//! Branches, remote branches, tags and stashes.

use crate::{GitError, Oid};

/// `for-each-ref` format: full name, object, peeled object (annotated tags), symbolic target,
/// upstream, upstream tracking, current-branch marker; fields separated by U+001F.
pub(crate) const REF_FORMAT: &str = "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(symref)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(HEAD)";

/// `stash list` format: id, selector (`stash@{0}`), time, message.
pub(crate) const STASH_FORMAT: &str = "--format=%H%x1f%gd%x1f%ct%x1f%gs";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

/// How a local branch relates to its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracking {
    /// Commits ahead and behind.
    AheadBehind(u32, u32),
    /// The upstream branch no longer exists.
    Gone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub kind: RefKind,
    /// Short name: `main`, `origin/main`, `v1.0`.
    pub name: String,
    /// Full name: `refs/heads/main`.
    pub full_name: String,
    /// The commit it points to (annotated tags peeled).
    pub target: Oid,
    /// The branch HEAD is on.
    pub is_head: bool,
    pub upstream: Option<String>,
    pub tracking: Option<Tracking>,
}

impl Ref {
    /// Parses the output of `for-each-ref` with [`REF_FORMAT`]. Symbolic refs (such as
    /// `refs/remotes/origin/HEAD`) are left out.
    pub fn parse_list(out: &[u8]) -> Result<Vec<Ref>, GitError> {
        let text = String::from_utf8_lossy(out);
        let mut refs = Vec::new();
        for line in text.lines().filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\u{1f}').collect();
            let bad = || GitError::Parse(format!("for-each-ref: {line:?}"));
            if f.len() != 7 {
                return Err(bad());
            }
            if !f[3].is_empty() {
                continue;
            }
            let (kind, name) = if let Some(n) = f[0].strip_prefix("refs/heads/") {
                (RefKind::Local, n)
            } else if let Some(n) = f[0].strip_prefix("refs/remotes/") {
                (RefKind::Remote, n)
            } else if let Some(n) = f[0].strip_prefix("refs/tags/") {
                (RefKind::Tag, n)
            } else {
                continue;
            };
            let target = if f[2].is_empty() { f[1] } else { f[2] };
            let Some(target) = Oid::parse(target) else {
                // A tag pointing at a tree or blob has no commit to show.
                if kind == RefKind::Tag {
                    continue;
                }
                return Err(bad());
            };
            let tracking = match (f[4], f[5]) {
                ("", _) => None,
                (_, "") => Some(Tracking::AheadBehind(0, 0)),
                (_, "gone") => Some(Tracking::Gone),
                (_, t) => Some(parse_track(t).ok_or_else(bad)?),
            };
            refs.push(Ref {
                kind,
                name: name.to_owned(),
                full_name: f[0].to_owned(),
                target,
                is_head: f[6] == "*",
                upstream: (!f[4].is_empty()).then(|| f[4].to_owned()),
                tracking,
            });
        }
        Ok(refs)
    }
}

/// `ahead 2, behind 1`, `ahead 2`, `behind 1`.
fn parse_track(t: &str) -> Option<Tracking> {
    let (mut ahead, mut behind) = (0, 0);
    for part in t.split(", ") {
        let (word, n) = part.split_once(' ')?;
        let n = n.parse().ok()?;
        match word {
            "ahead" => ahead = n,
            "behind" => behind = n,
            _ => return None,
        }
    }
    Some(Tracking::AheadBehind(ahead, behind))
}

/// One stash entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// `stash@{n}`.
    pub selector: String,
    pub id: Oid,
    pub time: i64,
    pub message: String,
}

impl Stash {
    /// Parses `stash list -z` with [`STASH_FORMAT`].
    pub fn parse_list(out: &[u8]) -> Result<Vec<Stash>, GitError> {
        let mut list = Vec::new();
        for rec in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let text = String::from_utf8_lossy(rec);
            let text = text.trim_start_matches('\n');
            let f: Vec<&str> = text.splitn(4, '\u{1f}').collect();
            let bad = || GitError::Parse(format!("stash list: {text:?}"));
            if f.len() != 4 {
                return Err(bad());
            }
            list.push(Stash {
                id: Oid::parse(f[0]).ok_or_else(bad)?,
                selector: f[1].to_owned(),
                time: f[2].parse().map_err(|_| bad())?,
                message: f[3].to_owned(),
            });
        }
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    fn line(f: [&str; 7]) -> String {
        f.join("\u{1f}") + "\n"
    }

    #[test]
    fn parses_branches_remotes_and_tags() {
        let out = [
            line([
                "refs/heads/main",
                A,
                "",
                "",
                "origin/main",
                "ahead 2, behind 1",
                "*",
            ]),
            line(["refs/heads/old", A, "", "", "origin/old", "gone", " "]),
            line(["refs/heads/solo", B, "", "", "", "", " "]),
            line([
                "refs/remotes/origin/HEAD",
                A,
                "",
                "refs/remotes/origin/main",
                "",
                "",
                " ",
            ]),
            line(["refs/remotes/origin/main", A, "", "", "", "", " "]),
            line(["refs/tags/v1", B, A, "", "", "", " "]),
        ]
        .concat();
        let refs = Ref::parse_list(out.as_bytes()).unwrap();
        assert_eq!(refs.len(), 5, "symbolic origin/HEAD left out");
        assert!(refs[0].is_head);
        assert_eq!(refs[0].tracking, Some(Tracking::AheadBehind(2, 1)));
        assert_eq!(refs[1].tracking, Some(Tracking::Gone));
        assert_eq!(refs[2].tracking, None);
        assert_eq!(refs[3].kind, RefKind::Remote);
        assert_eq!(refs[3].name, "origin/main");
        assert_eq!(refs[4].target.as_str(), A, "annotated tag peeled");
    }

    #[test]
    fn tracking_forms() {
        assert_eq!(parse_track("ahead 3"), Some(Tracking::AheadBehind(3, 0)));
        assert_eq!(parse_track("behind 4"), Some(Tracking::AheadBehind(0, 4)));
        assert_eq!(parse_track("sideways 1"), None);
    }

    #[test]
    fn stash_list() {
        let out = format!(
            "{A}\u{1f}stash@{{0}}\u{1f}1700000000\u{1f}WIP on main: abc\0\n{B}\u{1f}stash@{{1}}\u{1f}1\u{1f}On x: y\0"
        );
        let s = Stash::parse_list(out.as_bytes()).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].selector, "stash@{1}");
    }

    #[test]
    fn malformed_lists_are_errors() {
        assert!(Ref::parse_list(b"refs/heads/x\x1fnothex\x1f\x1f\x1f\x1f\x1f \n").is_err());
        assert!(Ref::parse_list(b"too\x1ffew\n").is_err());
        assert!(Stash::parse_list(b"x\x1fy\0").is_err());
    }
}
