//! Comparing two refs (issue #39): the commits only one side has, and what changed from their
//! merge base to the second side (what a merge of it would bring).

use std::ffi::OsString;

use crate::diff::{ChangedFile, FileDiff, parse_name_status, parse_numstat};
use crate::log::FORMAT;
use crate::{Commit, GitError, Oid, Repository};

/// Commits listed per side at most.
pub const COMPARE_LIMIT: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// Commits in `a` that `b` lacks, newest first (at most [`COMPARE_LIMIT`]).
    pub only_a: Vec<Commit>,
    /// Commits in `b` that `a` lacks.
    pub only_b: Vec<Commit>,
    /// The merge base; `None` when the two have no common history.
    pub base: Option<Oid>,
    /// Files changed from the merge base to `b`.
    pub files: Vec<ChangedFile>,
}

impl Repository {
    /// Compares `a` with `b` (refs or commit ids as the repository lists them).
    pub fn compare(&self, a: &str, b: &str) -> Result<Comparison, GitError> {
        let only = |from: &str, not: &str| -> Result<Vec<Commit>, GitError> {
            let range = format!("{not}..{from}");
            let max = format!("--max-count={COMPARE_LIMIT}");
            let out = self.read([
                "log",
                "-z",
                "--no-color",
                "--decorate=no",
                max.as_str(),
                FORMAT,
                "--end-of-options",
                range.as_str(),
            ])?;
            out.split(|b| *b == 0)
                .filter(|r| !r.is_empty())
                .map(|r| Commit::parse(r.strip_prefix(b"\n").unwrap_or(r)))
                .collect()
        };
        let base = match self.read(["merge-base", "--end-of-options", a, b]) {
            Ok(out) => Oid::parse(String::from_utf8_lossy(&out).trim()),
            Err(GitError::Failed { code: Some(1), .. }) => None,
            Err(e) => return Err(e),
        };
        let files = match &base {
            Some(base) => self.range_files(base.as_str(), b)?,
            None => Vec::new(),
        };
        Ok(Comparison {
            only_a: only(a, b)?,
            only_b: only(b, a)?,
            base,
            files,
        })
    }

    /// Files changed between two commits.
    pub fn range_files(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>, GitError> {
        let run = |format: &str| {
            self.read([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "-z",
                "--find-renames",
                format,
                "--end-of-options",
                from,
                to,
            ])
        };
        let kinds = parse_name_status(&run("--name-status")?);
        parse_numstat(&run("--numstat")?, &kinds)
    }

    /// The diff of one file between two commits.
    pub fn range_diff(
        &self,
        from: &str,
        to: &str,
        file: &ChangedFile,
    ) -> Result<FileDiff, GitError> {
        let mut args: Vec<OsString> = [
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--end-of-options",
            from,
            to,
            "--",
        ]
        .map(OsString::from)
        .to_vec();
        args.push(crate::path::bytes_to_path(file.path.as_bytes()).into_os_string());
        if let Some(old) = &file.old_path {
            args.push(crate::path::bytes_to_path(old.as_bytes()).into_os_string());
        }
        let out = self.read(args)?;
        Ok(FileDiff::parse(&out)?
            .into_iter()
            .next()
            .unwrap_or_default())
    }
}
