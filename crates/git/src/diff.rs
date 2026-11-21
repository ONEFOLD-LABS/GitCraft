//! Diffs as domain types (issue #19): the files a commit changed with their line counts
//! (`--numstat -z`), and unified diffs (`-p`) parsed into hunks and lines.
//!
//! File names come from the NUL-separated numstat output, never from the patch headers (which
//! quote unusual names); the patch parser only reads modes, flags, hunks and lines.

use crate::{GitError, RepoPath};

/// Lines kept per file diff; the rest is cut and the diff marked as truncated.
pub const MAX_LINES: usize = 200_000;
/// Longest line kept, in bytes; longer lines are cut.
const MAX_LINE: usize = 16 * 1024;

/// What happened to a file in a commit or in the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// One changed file with its line counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: RepoPath,
    /// The previous path of a renamed file.
    pub old_path: Option<RepoPath>,
    /// Added and deleted lines; `None` for binary files.
    pub lines: Option<(u32, u32)>,
    pub status: FileStatus,
}

/// Parses `git diff-tree -r -z --numstat --find-renames` (with `--summary` not used): records
/// `added\tdeleted\tpath\0` or, for renames, `added\tdeleted\t\0old\0new\0`. Additions and
/// deletions of whole files are told apart with `kinds` from `--name-status -z`.
pub fn parse_numstat(
    out: &[u8],
    kinds: &[(RepoPath, FileStatus)],
) -> Result<Vec<ChangedFile>, GitError> {
    let mut files = Vec::new();
    let mut parts = out.split(|b| *b == 0);
    while let Some(rec) = parts.next() {
        let rec = rec.strip_prefix(b"\n").unwrap_or(rec);
        if rec.is_empty() {
            continue;
        }
        let bad = || GitError::Parse(format!("numstat: {:?}", String::from_utf8_lossy(rec)));
        let mut f = rec.splitn(3, |b| *b == b'\t');
        let (Some(a), Some(d), Some(path)) = (f.next(), f.next(), f.next()) else {
            return Err(bad());
        };
        let lines = match (a, d) {
            (b"-", b"-") => None,
            _ => {
                let n = |x: &[u8]| {
                    std::str::from_utf8(x)
                        .ok()
                        .and_then(|s| s.parse::<u32>().ok())
                };
                Some((n(a).ok_or_else(bad)?, n(d).ok_or_else(bad)?))
            }
        };
        let (path, old_path) = if path.is_empty() {
            let old = parts.next().ok_or_else(bad)?;
            let new = parts.next().ok_or_else(bad)?;
            (RepoPath::from_bytes(new), Some(RepoPath::from_bytes(old)))
        } else {
            (RepoPath::from_bytes(path), None)
        };
        let status = if old_path.is_some() {
            FileStatus::Renamed
        } else {
            kinds
                .iter()
                .find(|(p, _)| p == &path)
                .map_or(FileStatus::Modified, |(_, k)| *k)
        };
        files.push(ChangedFile {
            path,
            old_path,
            lines,
            status,
        });
    }
    Ok(files)
}

/// Parses `--name-status -z` into added/deleted/modified per path (renames come from numstat).
pub fn parse_name_status(out: &[u8]) -> Vec<(RepoPath, FileStatus)> {
    let mut kinds = Vec::new();
    let mut parts = out.split(|b| *b == 0).filter(|p| !p.is_empty());
    while let Some(code) = parts.next() {
        let code = code.strip_prefix(b"\n").unwrap_or(code);
        let status = match code.first() {
            Some(b'A') => FileStatus::Added,
            Some(b'D') => FileStatus::Deleted,
            Some(b'R') | Some(b'C') => {
                // Two paths follow.
                let _ = parts.next();
                if let Some(p) = parts.next() {
                    kinds.push((RepoPath::from_bytes(p), FileStatus::Renamed));
                }
                continue;
            }
            _ => FileStatus::Modified,
        };
        if let Some(p) = parts.next() {
            kinds.push((RepoPath::from_bytes(p), status));
        }
    }
    kinds
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// Line number in the old file (context and removed lines).
    pub old: Option<u32>,
    /// Line number in the new file (context and added lines).
    pub new: Option<u32>,
    /// The text without the leading marker and line ending; any bytes.
    pub text: Vec<u8>,
    /// The file has no newline after this line.
    pub no_newline: bool,
}

/// A hunk: `@@ -old_start,old_len +new_start,new_len @@ section`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    /// The text after the second `@@` (often the enclosing function).
    pub section: String,
    pub lines: Vec<Line>,
}

/// The diff of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub binary: bool,
    pub new_file: bool,
    pub deleted: bool,
    /// Old and new mode when the mode changed (e.g. `100644` → `100755`).
    pub mode_change: Option<(String, String)>,
    /// The file is a submodule (mode 160000).
    pub submodule: bool,
    pub hunks: Vec<Hunk>,
    /// More than [`MAX_LINES`] lines; the rest is not shown.
    pub truncated: bool,
}

impl FileDiff {
    pub fn added(&self) -> usize {
        self.count(LineKind::Added)
    }

    pub fn removed(&self) -> usize {
        self.count(LineKind::Removed)
    }

    fn count(&self, kind: LineKind) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == kind)
            .count()
    }

    /// Parses a unified diff (`git diff -p` / `git diff-tree -p`) of one or more files; returns
    /// one [`FileDiff`] per `diff --git` section.
    pub fn parse(out: &[u8]) -> Result<Vec<FileDiff>, GitError> {
        let mut files: Vec<FileDiff> = Vec::new();
        let mut lines_total = 0usize;
        let mut old_no = 0u32;
        let mut new_no = 0u32;
        let mut in_hunk = false;
        for raw in out.split(|b| *b == b'\n') {
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            if raw.starts_with(b"diff --git ") || raw.starts_with(b"diff --cc ") {
                files.push(FileDiff::default());
                lines_total = 0;
                in_hunk = false;
                continue;
            }
            let Some(file) = files.last_mut() else {
                continue;
            };
            if raw.starts_with(b"@@") {
                let hunk = parse_hunk_header(raw)?;
                old_no = hunk.old_start;
                new_no = hunk.new_start;
                file.hunks.push(hunk);
                in_hunk = true;
                continue;
            }
            if !in_hunk {
                header_line(file, raw);
                continue;
            }
            if file.truncated {
                continue;
            }
            let Some(hunk) = file.hunks.last_mut() else {
                continue;
            };
            let (kind, text) = match raw.first() {
                Some(b' ') => (LineKind::Context, &raw[1..]),
                Some(b'+') => (LineKind::Added, &raw[1..]),
                Some(b'-') => (LineKind::Removed, &raw[1..]),
                Some(b'\\') => {
                    if let Some(last) = hunk.lines.last_mut() {
                        last.no_newline = true;
                    }
                    continue;
                }
                // An empty context line (some tools strip the leading space) or the end.
                None => (LineKind::Context, &raw[..0]),
                Some(_) => {
                    in_hunk = false;
                    header_line(file, raw);
                    continue;
                }
            };
            let (old, new) = match kind {
                LineKind::Context => (Some(old_no), Some(new_no)),
                LineKind::Added => (None, Some(new_no)),
                LineKind::Removed => (Some(old_no), None),
            };
            if kind != LineKind::Added {
                old_no = old_no.saturating_add(1);
            }
            if kind != LineKind::Removed {
                new_no = new_no.saturating_add(1);
            }
            lines_total += 1;
            if lines_total > MAX_LINES {
                file.truncated = true;
                continue;
            }
            hunk.lines.push(Line {
                kind,
                old,
                new,
                text: text[..text.len().min(MAX_LINE)].to_vec(),
                no_newline: false,
            });
        }
        // The empty "line" after the final newline was taken as context: drop such trailing
        // empty context lines that lie outside the hunk's announced length.
        for f in &mut files {
            for h in &mut f.hunks {
                let announced = h.old_len.max(h.new_len) as usize;
                while h.lines.len() > announced
                    && h.lines
                        .last()
                        .is_some_and(|l| l.kind == LineKind::Context && l.text.is_empty())
                {
                    h.lines.pop();
                }
            }
        }
        Ok(files)
    }
}

fn header_line(file: &mut FileDiff, raw: &[u8]) {
    let text = String::from_utf8_lossy(raw);
    if text.starts_with("Binary files ") || text.starts_with("GIT binary patch") {
        file.binary = true;
    } else if let Some(mode) = text.strip_prefix("new file mode ") {
        file.new_file = true;
        file.submodule |= mode.trim() == "160000";
    } else if let Some(mode) = text.strip_prefix("deleted file mode ") {
        file.deleted = true;
        file.submodule |= mode.trim() == "160000";
    } else if let Some(m) = text.strip_prefix("old mode ") {
        file.mode_change = Some((m.trim().to_owned(), String::new()));
    } else if let Some(m) = text.strip_prefix("new mode ") {
        if let Some((_, new)) = file.mode_change.as_mut() {
            *new = m.trim().to_owned();
        }
    } else if let Some(rest) = text.strip_prefix("index ") {
        file.submodule |= rest.trim_end().ends_with(" 160000");
    }
}

/// `@@ -1,3 +1,4 @@ fn main()` (lengths default to 1).
fn parse_hunk_header(raw: &[u8]) -> Result<Hunk, GitError> {
    let text = String::from_utf8_lossy(raw);
    let bad = || {
        GitError::Parse(format!(
            "hunk header: {:?}",
            text.chars().take(80).collect::<String>()
        ))
    };
    let rest = text.strip_prefix("@@ ").ok_or_else(bad)?;
    let (ranges, section) = rest.split_once(" @@").ok_or_else(bad)?;
    let (old, new) = ranges.split_once(' ').ok_or_else(bad)?;
    let range = |r: &str, sign: char| -> Option<(u32, u32)> {
        let r = r.strip_prefix(sign)?;
        match r.split_once(',') {
            Some((s, l)) => Some((s.parse().ok()?, l.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (old_start, old_len) = range(old, '-').ok_or_else(bad)?;
    let (new_start, new_len) = range(new, '+').ok_or_else(bad)?;
    Ok(Hunk {
        old_start,
        old_len,
        new_start,
        new_len,
        section: section.trim().to_owned(),
        lines: Vec::new(),
    })
}

/// A diff showing a new file's whole content as added (untracked files, read directly).
pub fn whole_file_added(content: &[u8]) -> FileDiff {
    let binary = content.iter().take(8000).any(|b| *b == 0);
    let mut diff = FileDiff {
        binary,
        new_file: true,
        ..FileDiff::default()
    };
    if binary || content.is_empty() {
        return diff;
    }
    let mut lines: Vec<&[u8]> = content.split(|b| *b == b'\n').collect();
    let ends_with_newline = content.ends_with(b"\n");
    if ends_with_newline {
        lines.pop();
    }
    let n = lines.len();
    let mut hunk = Hunk {
        old_start: 0,
        old_len: 0,
        new_start: 1,
        new_len: u32::try_from(n).unwrap_or(u32::MAX),
        section: String::new(),
        lines: Vec::new(),
    };
    for (i, l) in lines.into_iter().enumerate() {
        if i >= MAX_LINES {
            diff.truncated = true;
            break;
        }
        let l = l.strip_suffix(b"\r").unwrap_or(l);
        hunk.lines.push(Line {
            kind: LineKind::Added,
            old: None,
            new: Some(u32::try_from(i + 1).unwrap_or(u32::MAX)),
            text: l[..l.len().min(MAX_LINE)].to_vec(),
            no_newline: false,
        });
    }
    if !ends_with_newline && let Some(last) = hunk.lines.last_mut() {
        last.no_newline = true;
    }
    diff.hunks.push(hunk);
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunks_modes_and_no_newline() {
        let patch = [
            "diff --git a/x b/x",
            "index 1111111..2222222 100644",
            "--- a/x",
            "+++ b/x",
            "@@ -1,3 +1,3 @@ fn main()",
            " keep",
            "-old",
            "+new",
            " end",
            "\\ No newline at end of file",
            "diff --git a/run.sh b/run.sh",
            "old mode 100644",
            "new mode 100755",
            "diff --git a/img.png b/img.png",
            "new file mode 100644",
            "index 0000000..3333333",
            "Binary files /dev/null and b/img.png differ",
            "",
        ]
        .join("\n");
        let patch = patch.as_bytes();
        let files = FileDiff::parse(patch).unwrap();
        assert_eq!(files.len(), 3);
        let x = &files[0];
        assert_eq!(x.hunks.len(), 1);
        let h = &x.hunks[0];
        assert_eq!(
            (h.old_start, h.old_len, h.new_start, h.new_len),
            (1, 3, 1, 3)
        );
        assert_eq!(h.section, "fn main()");
        let kinds: Vec<LineKind> = h.lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(kinds, [Context, Removed, Added, Context]);
        assert_eq!(h.lines[1].old, Some(2));
        assert_eq!(h.lines[2].new, Some(2));
        assert_eq!(h.lines[3].old, Some(3));
        assert!(h.lines[3].no_newline);
        assert_eq!((x.added(), x.removed()), (1, 1));
        assert_eq!(
            files[1].mode_change,
            Some(("100644".into(), "100755".into()))
        );
        assert!(files[2].binary && files[2].new_file);
    }

    #[test]
    fn short_hunk_headers_and_crlf() {
        let patch = b"diff --git a/a b/a\n@@ -5 +5,2 @@\r\n-x\r\n+y\r\n+z\r\n";
        let f = &FileDiff::parse(patch).unwrap()[0];
        let h = &f.hunks[0];
        assert_eq!((h.old_len, h.new_len), (1, 2));
        assert_eq!(h.lines[0].text, b"x");
        assert_eq!(h.lines[2].new, Some(6));
    }

    #[test]
    fn malformed_hunk_headers_are_errors() {
        for p in [
            &b"diff --git a/a b/a\n@@ nonsense @@\n"[..],
            b"diff --git a/a b/a\n@@ -x +1 @@\n",
            b"diff --git a/a b/a\n@@ -1 +1\n",
        ] {
            assert!(
                FileDiff::parse(p).is_err(),
                "{:?}",
                String::from_utf8_lossy(p)
            );
        }
    }

    #[test]
    fn numstat_with_renames_and_binaries() {
        let out = b"3\t1\tsrc/a.rs\x00-\t-\timg.png\x000\t0\t\x00old name.txt\x00new name.txt\x00";
        let kinds = parse_name_status(
            b"M\x00src/a.rs\x00A\x00img.png\x00R100\x00old name.txt\x00new name.txt\x00",
        );
        let files = parse_numstat(out, &kinds).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].lines, Some((3, 1)));
        assert_eq!(files[1].lines, None);
        assert_eq!(files[1].status, FileStatus::Added);
        assert_eq!(files[2].status, FileStatus::Renamed);
        assert_eq!(
            files[2].old_path.as_ref().map(RepoPath::display),
            Some("old name.txt".into())
        );
        assert!(parse_numstat(b"x\ty\tz\x00", &[]).is_err());
    }

    #[test]
    fn whole_file() {
        let d = whole_file_added(b"a\nb");
        assert_eq!(d.added(), 2);
        assert!(d.hunks[0].lines[1].no_newline);
        assert!(whole_file_added(b"\x00\x01").binary);
        assert!(whole_file_added(b"").hunks.is_empty());
    }
}
