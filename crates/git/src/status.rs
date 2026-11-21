//! `git status --porcelain=v2 --branch -z` as domain types.

use std::fmt;

use crate::{GitError, RepoPath};

/// A commit id (SHA-1 or SHA-256, lowercase hex).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Oid(String);

impl Oid {
    pub fn parse(text: &str) -> Option<Oid> {
        let ok = matches!(text.len(), 40 | 64)
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        ok.then(|| Oid(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first seven characters, as commonly shown.
    pub fn short(&self) -> &str {
        self.0.get(..7).unwrap_or(&self.0)
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

/// Where HEAD points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// On a branch. `commit` is `None` while the branch has no commit yet (unborn HEAD, e.g. a
    /// fresh repository).
    Branch { name: String, commit: Option<Oid> },
    /// Detached HEAD at a commit.
    Detached { commit: Oid },
}

/// The upstream of the current branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// e.g. `origin/main`.
    pub name: String,
    /// Commits (ahead, behind); `None` when the upstream branch no longer exists.
    pub ahead_behind: Option<(u32, u32)>,
}

/// How a file differs between two of HEAD, index and working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Modified,
    /// File type changed (e.g. file ↔ symlink).
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
}

impl Change {
    /// The status letter git uses.
    pub fn letter(self) -> char {
        match self {
            Change::Modified => 'M',
            Change::TypeChanged => 'T',
            Change::Added => 'A',
            Change::Deleted => 'D',
            Change::Renamed => 'R',
            Change::Copied => 'C',
        }
    }
}

/// An unmerged path during a merge, rebase, cherry-pick or stash apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    BothModified,
    BothAdded,
    BothDeleted,
    AddedByUs,
    AddedByThem,
    DeletedByUs,
    DeletedByThem,
}

/// The state of one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Tracked, with a change staged (HEAD → index), unstaged (index → working tree) or both.
    Tracked {
        staged: Option<Change>,
        unstaged: Option<Change>,
    },
    Conflicted(Conflict),
    Untracked,
    Ignored,
}

/// One entry of the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: RepoPath,
    /// For renames and copies: the path the content came from.
    pub orig_path: Option<RepoPath>,
    pub state: State,
    /// The path is a submodule.
    pub submodule: bool,
}

/// The status of a working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub head: Head,
    pub upstream: Option<Upstream>,
    /// In git's order (sorted by path).
    pub files: Vec<FileChange>,
}

impl Status {
    /// Entries with a staged change.
    pub fn staged(&self) -> impl Iterator<Item = (&FileChange, Change)> {
        self.files.iter().filter_map(|f| match f.state {
            State::Tracked {
                staged: Some(c), ..
            } => Some((f, c)),
            _ => None,
        })
    }

    /// Entries with an unstaged change in the working tree.
    pub fn unstaged(&self) -> impl Iterator<Item = (&FileChange, Change)> {
        self.files.iter().filter_map(|f| match f.state {
            State::Tracked {
                unstaged: Some(c), ..
            } => Some((f, c)),
            _ => None,
        })
    }

    pub fn untracked(&self) -> impl Iterator<Item = &FileChange> {
        self.files.iter().filter(|f| f.state == State::Untracked)
    }

    pub fn conflicts(&self) -> impl Iterator<Item = (&FileChange, Conflict)> {
        self.files.iter().filter_map(|f| match f.state {
            State::Conflicted(c) => Some((f, c)),
            _ => None,
        })
    }

    /// Nothing staged, changed, conflicted or untracked.
    pub fn is_clean(&self) -> bool {
        self.files.iter().all(|f| f.state == State::Ignored)
    }

    /// Parses the output of
    /// `git status --porcelain=v2 --branch -z`. Malformed input is an error, never a panic.
    pub fn parse(out: &[u8]) -> Result<Status, GitError> {
        let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
        let mut oid: Option<Option<Oid>> = None;
        let mut head_name: Option<String> = None;
        let mut upstream: Option<Upstream> = None;
        let mut files = Vec::new();
        while let Some(rec) = records.next() {
            match rec.first() {
                Some(b'#') => header(rec, &mut oid, &mut head_name, &mut upstream)?,
                Some(b'1') => {
                    let f = fields(rec, 9)?;
                    files.push(tracked(&f, None)?);
                }
                Some(b'2') => {
                    let f = fields(rec, 10)?;
                    let orig = records
                        .next()
                        .ok_or_else(|| bad("rename without original path", rec))?;
                    files.push(tracked(&f, Some(RepoPath::from_bytes(orig)))?);
                }
                Some(b'u') => {
                    let f = fields(rec, 11)?;
                    files.push(FileChange {
                        path: RepoPath::from_bytes(f[10]),
                        orig_path: None,
                        state: State::Conflicted(conflict(f[1]).ok_or_else(|| bad("XY", rec))?),
                        submodule: f[2].first() == Some(&b'S'),
                    });
                }
                Some(b'?') | Some(b'!') => {
                    let path = rec
                        .get(2..)
                        .filter(|p| !p.is_empty() && rec.get(1) == Some(&b' '));
                    let path = path.ok_or_else(|| bad("path", rec))?;
                    files.push(FileChange {
                        path: RepoPath::from_bytes(path),
                        orig_path: None,
                        state: if rec[0] == b'?' {
                            State::Untracked
                        } else {
                            State::Ignored
                        },
                        submodule: false,
                    });
                }
                _ => return Err(bad("unknown entry", rec)),
            }
        }
        let name = head_name.ok_or_else(|| GitError::Parse("no branch.head".into()))?;
        let oid = oid.ok_or_else(|| GitError::Parse("no branch.oid".into()))?;
        let head = if name == "(detached)" {
            Head::Detached {
                commit: oid
                    .ok_or_else(|| GitError::Parse("detached HEAD without commit".into()))?,
            }
        } else {
            Head::Branch { name, commit: oid }
        };
        Ok(Status {
            head,
            upstream,
            files,
        })
    }
}

fn bad(what: &str, rec: &[u8]) -> GitError {
    let shown: String = String::from_utf8_lossy(rec).chars().take(120).collect();
    GitError::Parse(format!("{what}: {shown:?}"))
}

/// Splits a record into exactly `n` space-separated fields; the last one (the path) keeps its
/// spaces.
fn fields(rec: &[u8], n: usize) -> Result<Vec<&[u8]>, GitError> {
    let f: Vec<&[u8]> = rec.splitn(n, |b| *b == b' ').collect();
    if f.len() != n || f.last().is_none_or(|p| p.is_empty()) {
        return Err(bad("fields", rec));
    }
    Ok(f)
}

fn header(
    rec: &[u8],
    oid: &mut Option<Option<Oid>>,
    head: &mut Option<String>,
    upstream: &mut Option<Upstream>,
) -> Result<(), GitError> {
    let text = String::from_utf8_lossy(rec);
    let Some(rest) = text.strip_prefix("# ") else {
        return Err(bad("header", rec));
    };
    let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
    match key {
        "branch.oid" => {
            *oid = Some(if value == "(initial)" {
                None
            } else {
                Some(Oid::parse(value).ok_or_else(|| bad("branch.oid", rec))?)
            });
        }
        "branch.head" if !value.is_empty() => *head = Some(value.to_owned()),
        "branch.upstream" if !value.is_empty() => {
            *upstream = Some(Upstream {
                name: value.to_owned(),
                ahead_behind: None,
            });
        }
        "branch.ab" => {
            let (a, b) = value.split_once(' ').ok_or_else(|| bad("branch.ab", rec))?;
            let ahead = a.strip_prefix('+').and_then(|n| n.parse::<u32>().ok());
            let behind = b.strip_prefix('-').and_then(|n| n.parse::<u32>().ok());
            let (Some(ahead), Some(behind)) = (ahead, behind) else {
                return Err(bad("branch.ab", rec));
            };
            match upstream {
                Some(u) => u.ahead_behind = Some((ahead, behind)),
                None => return Err(bad("branch.ab without upstream", rec)),
            }
        }
        // Other headers (e.g. stash counts) are not used yet.
        _ => {}
    }
    Ok(())
}

/// An ordinary (`1`) or renamed/copied (`2`) entry.
fn tracked(f: &[&[u8]], orig: Option<RepoPath>) -> Result<FileChange, GitError> {
    let xy = f[1];
    let (Some(&x), Some(&y), 2) = (xy.first(), xy.get(1), xy.len()) else {
        return Err(bad("XY", xy));
    };
    let staged = change(x).ok_or_else(|| bad("X", xy))?;
    let unstaged = change(y).ok_or_else(|| bad("Y", xy))?;
    let path = f.last().copied().unwrap_or_default();
    Ok(FileChange {
        path: RepoPath::from_bytes(path),
        orig_path: orig,
        state: State::Tracked { staged, unstaged },
        submodule: f[2].first() == Some(&b'S'),
    })
}

/// One status letter: `Some(None)` for “unchanged”, `None` if unknown.
fn change(c: u8) -> Option<Option<Change>> {
    Some(match c {
        b'.' => None,
        b'M' => Some(Change::Modified),
        b'T' => Some(Change::TypeChanged),
        b'A' => Some(Change::Added),
        b'D' => Some(Change::Deleted),
        b'R' => Some(Change::Renamed),
        b'C' => Some(Change::Copied),
        _ => return None,
    })
}

fn conflict(xy: &[u8]) -> Option<Conflict> {
    Some(match xy {
        b"UU" => Conflict::BothModified,
        b"AA" => Conflict::BothAdded,
        b"DD" => Conflict::BothDeleted,
        b"AU" => Conflict::AddedByUs,
        b"UA" => Conflict::AddedByThem,
        b"DU" => Conflict::DeletedByUs,
        b"UD" => Conflict::DeletedByThem,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    fn parse(text: &str) -> Result<Status, GitError> {
        Status::parse(text.replace('|', "\0").as_bytes())
    }

    #[test]
    fn parses_every_entry_kind() {
        let s = parse(&format!(
            "# branch.oid {A}|# branch.head main|# branch.upstream origin/main|# branch.ab +2 -1|\
             1 .M N... 100644 100644 100644 {A} {A} file with spaces.txt|\
             1 MM N... 100644 100644 100644 {A} {B} both.txt|\
             2 R. N... 100644 100644 100644 {A} {A} R100 new name.txt|old name.txt|\
             u UU N... 100644 100644 100644 100644 {A} {B} {A} conflict.txt|\
             1 .M S.M. 160000 160000 160000 {A} {A} sub|\
             ? dir/untracked.txt|! target|"
        ))
        .unwrap();
        assert_eq!(
            s.head,
            Head::Branch {
                name: "main".into(),
                commit: Oid::parse(A)
            }
        );
        assert_eq!(
            s.upstream,
            Some(Upstream {
                name: "origin/main".into(),
                ahead_behind: Some((2, 1))
            })
        );
        assert_eq!(s.files.len(), 7);
        let staged: Vec<_> = s.staged().map(|(f, c)| (f.path.display(), c)).collect();
        assert_eq!(
            staged,
            [
                ("both.txt".into(), Change::Modified),
                ("new name.txt".into(), Change::Renamed)
            ]
        );
        assert_eq!(
            s.files[2].orig_path.as_ref().map(RepoPath::display),
            Some("old name.txt".into())
        );
        assert_eq!(s.unstaged().count(), 3);
        assert!(s.files[4].submodule);
        assert_eq!(
            s.conflicts().map(|(_, c)| c).collect::<Vec<_>>(),
            [Conflict::BothModified]
        );
        assert_eq!(s.untracked().count(), 1);
        assert!(!s.is_clean());
    }

    #[test]
    fn unborn_detached_and_gone_upstream() {
        let s = parse("# branch.oid (initial)|# branch.head main|").unwrap();
        assert_eq!(
            s.head,
            Head::Branch {
                name: "main".into(),
                commit: None
            }
        );
        assert!(s.is_clean());
        let s = parse(&format!("# branch.oid {A}|# branch.head (detached)|")).unwrap();
        assert_eq!(
            s.head,
            Head::Detached {
                commit: Oid::parse(A).unwrap()
            }
        );
        let s = parse(&format!(
            "# branch.oid {A}|# branch.head x|# branch.upstream origin/x|"
        ))
        .unwrap();
        assert_eq!(s.upstream.unwrap().ahead_behind, None);
    }

    #[test]
    fn malformed_output_is_an_error_not_a_panic() {
        for text in [
            "",
            "# branch.head main|",
            "# branch.oid nothex|# branch.head main|",
            "# branch.oid (initial)|# branch.head (detached)|",
            &format!("# branch.oid {A}|# branch.head main|1 .M N...|"),
            &format!("# branch.oid {A}|# branch.head main|1 XY N... 1 1 1 {A} {A} f|"),
            &format!("# branch.oid {A}|# branch.head main|1 .MM N... 1 1 1 {A} {A} f|"),
            &format!("# branch.oid {A}|# branch.head main|2 R. N... 1 1 1 {A} {A} R1 f|"),
            &format!("# branch.oid {A}|# branch.head main|u ZZ N... 1 1 1 1 {A} {A} {A} f|"),
            &format!("# branch.oid {A}|# branch.head main|# branch.ab +x -1|"),
            &format!("# branch.oid {A}|# branch.head main|# branch.ab +1 -1|"),
            &format!("# branch.oid {A}|# branch.head main|? |"),
            &format!("# branch.oid {A}|# branch.head main|?x|"),
            &format!("# branch.oid {A}|# branch.head main|z what|"),
            "garbage",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn oid_validation() {
        assert!(Oid::parse(A).is_some());
        assert!(Oid::parse(&"a".repeat(64)).is_some());
        assert!(Oid::parse(&"A".repeat(40)).is_none());
        assert!(Oid::parse("abc").is_none());
        assert_eq!(Oid::parse(A).unwrap().short(), "1111111");
    }
}
