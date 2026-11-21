//! The commit history as a stream of [`Commit`]s, read page by page from one running
//! `git log` (no `--skip`, which would recompute the graph order for every page).

use crate::{GitError, GitStream, Oid};

/// Field separator in the log format (ASCII unit separator); commits are separated by NUL.
const FIELD: char = '\u{1f}';
/// Largest single commit record (subject lines can be long, but not this long).
const MAX_RECORD: usize = 64 * 1024;

/// The `--format` GitCraft reads: id, parents, author name/email/time, committer
/// name/email/time, subject.
pub(crate) const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%s";

/// Who and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
}

/// One commit as the history shows it (the full message comes with the details).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: Oid,
    /// First parent first; empty for a root commit.
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    pub subject: String,
}

impl Commit {
    /// Parses one record of [`FORMAT`].
    pub fn parse(record: &[u8]) -> Result<Commit, GitError> {
        let text = String::from_utf8_lossy(record);
        let f: Vec<&str> = text.splitn(9, FIELD).collect();
        let bad = |what: &str| GitError::Parse(format!("log {what}: {:?}", truncate(&text)));
        if f.len() != 9 {
            return Err(bad("fields"));
        }
        let id = Oid::parse(f[0]).ok_or_else(|| bad("id"))?;
        let parents = f[1]
            .split(' ')
            .filter(|p| !p.is_empty())
            .map(|p| Oid::parse(p).ok_or_else(|| bad("parent")))
            .collect::<Result<Vec<_>, _>>()?;
        let time = |s: &str| s.parse::<i64>().map_err(|_| bad("time"));
        Ok(Commit {
            id,
            parents,
            author: Signature {
                name: f[2].to_owned(),
                email: f[3].to_owned(),
                time: time(f[4])?,
            },
            committer: Signature {
                name: f[5].to_owned(),
                email: f[6].to_owned(),
                time: time(f[7])?,
            },
            subject: f[8].to_owned(),
        })
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(120).collect()
}

/// A running `git log`, read in pages. Dropping it stops git.
pub struct LogReader {
    stream: Option<GitStream>,
}

impl LogReader {
    pub(crate) fn new(stream: Option<GitStream>) -> LogReader {
        LogReader { stream }
    }

    /// Up to `n` further commits; fewer (possibly none) at the end of the history.
    pub fn next_page(&mut self, n: usize) -> Result<Vec<Commit>, GitError> {
        let mut page = Vec::with_capacity(n.min(4096));
        while page.len() < n {
            let Some(stream) = self.stream.as_mut() else {
                break;
            };
            match stream.next_record(0, MAX_RECORD)? {
                Some(record) => {
                    // `git log -z` puts a newline before each record after the first.
                    let record = record.strip_prefix(b"\n").unwrap_or(&record);
                    if !record.is_empty() {
                        page.push(Commit::parse(record)?);
                    }
                }
                None => {
                    if let Some(stream) = self.stream.take() {
                        stream.finish()?;
                    }
                }
            }
        }
        Ok(page)
    }

    /// True once the whole history has been read.
    pub fn is_done(&self) -> bool {
        self.stream.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";

    fn rec(fields: &[&str]) -> Vec<u8> {
        fields.join("\u{1f}").into_bytes()
    }

    #[test]
    fn parses_a_merge_commit() {
        let c = Commit::parse(&rec(&[
            A,
            &format!("{B} {C}"),
            "Ann",
            "ann@invalid",
            "1700000000",
            "Bob",
            "bob@invalid",
            "1700000100",
            "Merge branch 'x' \u{1f} with a separator in the subject",
        ]))
        .unwrap();
        assert_eq!(c.parents.len(), 2);
        assert_eq!(c.author.time, 1_700_000_000);
        assert_eq!(c.committer.name, "Bob");
        assert!(c.subject.ends_with("in the subject"));
    }

    #[test]
    fn root_commit_and_empty_subject() {
        let c = Commit::parse(&rec(&[A, "", "a", "e", "0", "c", "e", "0", ""])).unwrap();
        assert!(c.parents.is_empty());
        assert_eq!(c.subject, "");
    }

    #[test]
    fn malformed_records_are_errors() {
        for r in [
            rec(&[A]),
            rec(&["xyz", "", "a", "e", "0", "c", "e", "0", "s"]),
            rec(&[A, "nothex", "a", "e", "0", "c", "e", "0", "s"]),
            rec(&[A, "", "a", "e", "soon", "c", "e", "0", "s"]),
            Vec::new(),
        ] {
            assert!(
                Commit::parse(&r).is_err(),
                "{:?}",
                String::from_utf8_lossy(&r)
            );
        }
    }
}
