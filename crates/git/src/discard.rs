//! Throwing work away with a way back (issues #24, #25, docs/security.md "Destructive
//! operations").
//!
//! Before GitCraft discards changes or deletes untracked files, it stores the files as they are
//! on disk: raw blobs (no filters; a symbolic link as its target), collected in a tree of a
//! commit outside every branch, kept reachable by a ref `refs/gitcraft/discarded/<time>` so that
//! `git gc` does not prune it. Restoring writes the files back through the checked-path
//! function. Old entries are removed after the retention time.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Cancel, FileDiff, GitError, Oid, RepoPath, Repository, Written};

/// Where the saved contents live.
pub const DISCARDED_REFS: &str = "refs/gitcraft/discarded/";
const TEMP_INDEX: &str = "gitcraft-discard.index";

/// One saved state: the files as they were before they were discarded or deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discarded {
    /// The full ref name.
    pub id: String,
    pub commit: Oid,
    /// Unix seconds.
    pub time: i64,
    /// Files that existed (their contents are saved).
    pub files: Vec<RepoPath>,
    /// Files that did not exist (a discarded deletion): restoring removes them again.
    pub absent: Vec<RepoPath>,
}

impl Repository {
    /// Discards the unstaged changes of tracked files (back to the index), after saving them.
    /// Returns the ref of the saved state.
    pub fn discard(&self, paths: &[RepoPath], cancel: &Cancel) -> Result<String, GitError> {
        if paths.is_empty() {
            return Err(GitError::InvalidValue("no file".into()));
        }
        let saved = self.save_contents(paths, cancel)?;
        self.write_paths(&["restore", "--worktree"], paths, cancel)?;
        Ok(saved)
    }

    /// Discards some lines of a file's unstaged diff: (hunk, line) positions in `shown`, the
    /// diff the user saw ([`GitError::Stale`] if the file changed since). The whole file is
    /// saved first.
    pub fn discard_lines(
        &self,
        path: &RepoPath,
        shown: &FileDiff,
        lines: &[(usize, usize)],
        cancel: &Cancel,
    ) -> Result<String, GitError> {
        let raw = self.worktree_diff_raw(path, None, false)?;
        let fresh = FileDiff::parse(&raw)?
            .into_iter()
            .next()
            .unwrap_or_default();
        if &fresh != shown {
            return Err(GitError::Stale);
        }
        if fresh.binary || fresh.truncated || fresh.submodule {
            return Err(GitError::InvalidValue(path.display()));
        }
        // Taking the selected changes out of the working tree is the unstage case of the
        // patch: what stays is context, what was not there stays away.
        let patch = crate::patch::build(&raw, lines, true)?;
        let saved = self.save_contents(std::slice::from_ref(path), cancel)?;
        self.write(
            ["apply", "--reverse", "--recount", "-"],
            Some(&patch),
            cancel,
        )?;
        Ok(saved)
    }

    /// Deletes untracked files (only files git lists as untracked and not ignored), after
    /// saving them. Directories left empty are removed too.
    pub fn delete_untracked(
        &self,
        paths: &[RepoPath],
        cancel: &Cancel,
    ) -> Result<String, GitError> {
        if paths.is_empty() {
            return Err(GitError::InvalidValue("no file".into()));
        }
        let listed = self.read(["ls-files", "-z", "--others", "--exclude-standard"])?;
        let untracked: std::collections::HashSet<&[u8]> = listed
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .collect();
        let mut targets = Vec::new();
        for p in paths {
            if !untracked.contains(p.as_bytes()) {
                return Err(GitError::UnsafePath(format!(
                    "{}: not untracked",
                    p.display()
                )));
            }
            targets.push(p.checked_path(self.workdir())?);
        }
        let saved = self.save_contents(paths, cancel)?;
        for full in &targets {
            remove_entry(full)?;
            remove_empty_parents(full, self.workdir());
        }
        Ok(saved)
    }

    /// The saved states, newest first.
    pub fn discarded(&self) -> Result<Vec<Discarded>, GitError> {
        let out = self.read([
            "for-each-ref",
            "--sort=-refname",
            "--format=%(refname) %(objectname) %(creatordate:unix)",
            DISCARDED_REFS,
        ])?;
        let mut list = Vec::new();
        for line in String::from_utf8_lossy(&out).lines() {
            let mut f = line.split(' ');
            let (Some(id), Some(commit), Some(time)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let commit = Oid::parse(commit)
                .ok_or_else(|| GitError::Parse(format!("discarded ref: {line}")))?;
            let files = self
                .read(["ls-tree", "-r", "-z", "--name-only", commit.as_str()])?
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(RepoPath::from_bytes)
                .collect();
            let message = self.read(["cat-file", "commit", commit.as_str()])?;
            list.push(Discarded {
                id: id.to_owned(),
                time: time.parse().unwrap_or_default(),
                commit,
                files,
                absent: absent_in(&message),
            });
        }
        Ok(list)
    }

    /// Writes a saved state back to disk (the files' current state is saved first, so this can
    /// be undone as well) and removes the entry. Returns the ref of the state saved before.
    pub fn restore_discarded(&self, id: &str, cancel: &Cancel) -> Result<String, GitError> {
        let entry = self
            .discarded()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| GitError::InvalidValue(id.to_owned()))?;
        let all: Vec<RepoPath> = entry.files.iter().chain(&entry.absent).cloned().collect();
        // Check every path before anything is touched.
        for p in &all {
            p.checked_path(self.workdir())?;
        }
        let before = self.save_contents(&all, cancel)?;
        let tree = self.read(["ls-tree", "-r", "-z", entry.commit.as_str()])?;
        for rec in tree.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let tab = rec
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| GitError::Parse("ls-tree".into()))?;
            let meta = String::from_utf8_lossy(&rec[..tab]);
            let mut m = meta.split(' ');
            let (Some(mode), Some(_), Some(sha)) = (m.next(), m.next(), m.next()) else {
                return Err(GitError::Parse(format!("ls-tree: {meta}")));
            };
            let path = RepoPath::from_bytes(&rec[tab + 1..]);
            let full = path.checked_path(self.workdir())?;
            let bytes = self.read(["cat-file", "blob", sha])?;
            write_entry(&full, mode, &bytes)?;
        }
        for p in &entry.absent {
            let full = p.checked_path(self.workdir())?;
            if std::fs::symlink_metadata(&full).is_ok() {
                remove_entry(&full)?;
                remove_empty_parents(&full, self.workdir());
            }
        }
        self.write(["update-ref", "-d", id], None, cancel)?;
        Ok(before)
    }

    /// Removes saved states older than `keep_secs` (relative to now).
    pub fn prune_discarded(&self, keep_secs: i64, cancel: &Cancel) -> Result<usize, GitError> {
        let now = now_secs();
        let mut removed = 0;
        for d in self.discarded()? {
            if now.saturating_sub(d.time) > keep_secs {
                self.write(["update-ref", "-d", d.id.as_str()], None, cancel)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Stores the files as they are on disk and returns the ref that keeps them.
    fn save_contents(&self, paths: &[RepoPath], cancel: &Cancel) -> Result<String, GitError> {
        let mut index_info = Vec::new();
        let mut absent = Vec::new();
        for p in paths {
            let full = p.checked_path(self.workdir())?;
            let (mode, bytes) = match std::fs::symlink_metadata(&full) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    absent.push(p.clone());
                    continue;
                }
                Err(e) => return Err(GitError::Spawn(format!("{}: {e}", p.display()))),
                Ok(m) if m.file_type().is_symlink() => {
                    let target = std::fs::read_link(&full)
                        .map_err(|e| GitError::Spawn(format!("{}: {e}", p.display())))?;
                    ("120000", path_bytes(&target))
                }
                Ok(m) if m.is_file() => {
                    let bytes = std::fs::read(&full)
                        .map_err(|e| GitError::Spawn(format!("{}: {e}", p.display())))?;
                    (if executable(&m) { "100755" } else { "100644" }, bytes)
                }
                Ok(_) => {
                    return Err(GitError::UnsafePath(format!("{}: not a file", p.display())));
                }
            };
            let out = self.write(
                ["hash-object", "-w", "--no-filters", "--stdin"],
                Some(&bytes),
                cancel,
            )?;
            let sha = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if Oid::parse(&sha).is_none() {
                return Err(GitError::Parse(format!("hash-object: {sha}")));
            }
            index_info.extend_from_slice(format!("{mode} {sha}\t").as_bytes());
            index_info.extend_from_slice(p.as_bytes());
            index_info.push(0);
        }
        let index = self.git_dir().join(TEMP_INDEX);
        let _ = std::fs::remove_file(&index);
        let env = [("GIT_INDEX_FILE", index.as_os_str())];
        let tree = (|| {
            if !index_info.is_empty() {
                self.write_env(
                    ["update-index", "-z", "--index-info"],
                    Some(&index_info),
                    cancel,
                    &env,
                )?;
            }
            self.write_env(["write-tree"], None, cancel, &env)
        })();
        let _ = std::fs::remove_file(&index);
        let tree = String::from_utf8_lossy(&tree?.stdout).trim().to_owned();
        let mut message = String::from("GitCraft: verworfene Inhalte\n\n");
        for p in &absent {
            message.push_str(&format!("absent {}\n", hex(p.as_bytes())));
        }
        let me: [(&str, &OsStr); 4] = [
            ("GIT_AUTHOR_NAME", OsStr::new("GitCraft")),
            ("GIT_AUTHOR_EMAIL", OsStr::new("gitcraft@localhost")),
            ("GIT_COMMITTER_NAME", OsStr::new("GitCraft")),
            ("GIT_COMMITTER_EMAIL", OsStr::new("gitcraft@localhost")),
        ];
        let commit = self.write_env(
            ["commit-tree", "--no-gpg-sign", "-F", "-", tree.as_str()],
            Some(message.as_bytes()),
            cancel,
            &me,
        )?;
        let commit = String::from_utf8_lossy(&commit.stdout).trim().to_owned();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let id = format!(
            "{DISCARDED_REFS}{}-{:09}",
            stamp.as_secs(),
            stamp.subsec_nanos()
        );
        self.write(["update-ref", id.as_str(), commit.as_str()], None, cancel)?;
        Ok(id)
    }

    fn write_env<I, S>(
        &self,
        args: I,
        input: Option<&[u8]>,
        cancel: &Cancel,
        env: &[(&str, &OsStr)],
    ) -> Result<Written, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.runner_ref()
            .write_env(self.workdir(), &[], args, input, cancel, env)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The `absent <hex>` lines of a saved state's commit.
fn absent_in(commit: &[u8]) -> Vec<RepoPath> {
    let text = String::from_utf8_lossy(commit);
    let body = text.split_once("\n\n").map_or("", |(_, b)| b);
    body.lines()
        .filter_map(|l| l.strip_prefix("absent "))
        .filter_map(unhex)
        .filter(|p| !p.is_empty())
        .map(|p| RepoPath::from_bytes(&p))
        .collect()
}

fn path_bytes(p: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        p.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        p.to_string_lossy().as_bytes().to_vec()
    }
}

#[cfg(unix)]
fn executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    false
}

/// Removes a file or symbolic link (never what a link points to); directories are refused.
fn remove_entry(full: &Path) -> Result<(), GitError> {
    let io = |e: std::io::Error| GitError::Spawn(format!("{}: {e}", full.display()));
    let meta = std::fs::symlink_metadata(full).map_err(io)?;
    if meta.is_dir() {
        return Err(GitError::UnsafePath(format!(
            "{}: a directory",
            full.display()
        )));
    }
    std::fs::remove_file(full).map_err(io)
}

/// Removes the parent directories of `full` that became empty, up to (not including) `top`.
fn remove_empty_parents(full: &Path, top: &Path) {
    let mut dir: Option<PathBuf> = full.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == top || !d.starts_with(top) || std::fs::remove_dir(&d).is_err() {
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
}

/// Writes one saved entry back: a file with its mode, or a symbolic link.
fn write_entry(full: &Path, mode: &str, bytes: &[u8]) -> Result<(), GitError> {
    let io = |e: std::io::Error| GitError::Spawn(format!("{}: {e}", full.display()));
    if let Ok(meta) = std::fs::symlink_metadata(full) {
        if meta.is_dir() {
            return Err(GitError::UnsafePath(format!(
                "{}: a directory",
                full.display()
            )));
        }
        std::fs::remove_file(full).map_err(io)?;
    }
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    if mode == "120000" {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            return std::os::unix::fs::symlink(OsStr::from_bytes(bytes), full).map_err(io);
        }
    }
    std::fs::write(full, bytes).map_err(io)?;
    #[cfg(unix)]
    if mode == "100755" {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(full, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_paths_round_trip_through_the_message() {
        let p = RepoPath::from_bytes(b"a\nb \xff");
        let msg = format!(
            "tree x\n\nGitCraft\n\nabsent {}\nabsent zz\n",
            hex(p.as_bytes())
        );
        assert_eq!(absent_in(msg.as_bytes()), [p]);
        assert_eq!(unhex("0g"), None);
    }
}
