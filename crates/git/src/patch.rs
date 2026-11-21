//! Partial patches for staging and unstaging hunks and lines (issues #22, #23).
//!
//! The patch is cut from git's own diff output, byte for byte (line endings, unusual file names
//! in the headers stay exactly as git wrote them). Line positions are the same as in the parsed
//! [`crate::FileDiff`]: hunk index, then line index within the hunk.

use std::collections::BTreeSet;

use crate::GitError;

const NO_NEWLINE: &[u8] = b"\\ No newline at end of file";

struct RawLine<'a> {
    marker: u8,
    body: &'a [u8],
    no_newline: bool,
}

/// Builds a patch containing only the `selected` changed lines.
///
/// Staging (`reverse == false`, applied to the index): unselected additions are dropped,
/// unselected removals become context. Unstaging (`reverse == true`, the staged diff applied in
/// reverse): unselected additions become context (they stay in the index), unselected removals
/// are dropped. Hunks without a selected change are left out. The hunk lengths are left to
/// `git apply --recount`.
pub(crate) fn build(
    raw: &[u8],
    selected: &[(usize, usize)],
    reverse: bool,
) -> Result<Vec<u8>, GitError> {
    let selected: BTreeSet<(usize, usize)> = selected.iter().copied().collect();
    let text = raw.strip_suffix(b"\n").unwrap_or(raw);
    let mut header: Vec<&[u8]> = Vec::new();
    let mut hunks: Vec<(&[u8], Vec<RawLine>)> = Vec::new();
    for line in text.split(|b| *b == b'\n') {
        if line.starts_with(b"@@") {
            hunks.push((line, Vec::new()));
            continue;
        }
        let Some((_, lines)) = hunks.last_mut() else {
            if line.starts_with(b"diff --git ") && !header.is_empty() {
                return Err(GitError::Parse("patch: more than one file".into()));
            }
            header.push(line);
            continue;
        };
        match line.first() {
            Some(m @ (b' ' | b'+' | b'-')) => lines.push(RawLine {
                marker: *m,
                body: &line[1..],
                no_newline: false,
            }),
            Some(b'\\') => {
                if let Some(last) = lines.last_mut() {
                    last.no_newline = true;
                }
            }
            None => lines.push(RawLine {
                marker: b' ',
                body: b"",
                no_newline: false,
            }),
            Some(_) => return Err(GitError::Parse("patch: unexpected line".into())),
        }
    }
    let mut out = Vec::new();
    for h in &header {
        out.extend_from_slice(h);
        out.push(b'\n');
    }
    let mut changes = 0usize;
    for (hi, (head, lines)) in hunks.iter().enumerate() {
        if !lines
            .iter()
            .enumerate()
            .any(|(li, l)| l.marker != b' ' && selected.contains(&(hi, li)))
        {
            continue;
        }
        out.extend_from_slice(head);
        out.push(b'\n');
        for (li, l) in lines.iter().enumerate() {
            let chosen = selected.contains(&(hi, li));
            let marker = match (l.marker, chosen, reverse) {
                (b' ', _, _) => Some(b' '),
                (m, true, _) => {
                    changes += 1;
                    Some(m)
                }
                (b'+', false, false) | (b'-', false, true) => None,
                _ => Some(b' '),
            };
            if let Some(m) = marker {
                out.push(m);
                out.extend_from_slice(l.body);
                out.push(b'\n');
                if l.no_newline {
                    out.extend_from_slice(NO_NEWLINE);
                    out.push(b'\n');
                }
            }
        }
    }
    if changes == 0 {
        return Err(GitError::InvalidValue("no changed line selected".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &[u8] = b"diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -10,2 +10,3 @@ fn x\n j\n+k\r\n l\n";

    #[test]
    fn a_whole_hunk() {
        let p = build(DIFF, &[(1, 1)], false).unwrap();
        assert_eq!(
            p,
            b"diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -10,2 +10,3 @@ fn x\n j\n+k\r\n l\n"
        );
    }

    #[test]
    fn unselected_lines_for_staging_and_unstaging() {
        // Only the removal: the addition is dropped when staging...
        let p = build(DIFF, &[(0, 1)], false).unwrap();
        assert!(
            p.ends_with(b"@@ -1,3 +1,3 @@\n a\n-b\n c\n"),
            "{}",
            String::from_utf8_lossy(&p)
        );
        // ...and becomes context when unstaging.
        let p = build(DIFF, &[(0, 1)], true).unwrap();
        assert!(
            p.ends_with(b"@@ -1,3 +1,3 @@\n a\n-b\n B\n c\n"),
            "{}",
            String::from_utf8_lossy(&p)
        );
        // Only the addition: the removal becomes context when staging.
        let p = build(DIFF, &[(0, 2)], false).unwrap();
        assert!(
            p.ends_with(b"@@ -1,3 +1,3 @@\n a\n b\n+B\n c\n"),
            "{}",
            String::from_utf8_lossy(&p)
        );
    }

    #[test]
    fn no_newline_markers_and_empty_selections() {
        let d = b"--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n";
        let p = build(d, &[(0, 0), (0, 1)], false).unwrap();
        assert!(
            p.ends_with(b"-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n")
        );
        assert!(
            matches!(
                build(DIFF, &[(0, 0)], false),
                Err(GitError::InvalidValue(_))
            ),
            "context only"
        );
        assert!(matches!(
            build(DIFF, &[], false),
            Err(GitError::InvalidValue(_))
        ));
    }
}
