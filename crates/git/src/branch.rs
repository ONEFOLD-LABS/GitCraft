//! Branches (issues #37, #38): create, switch, check out a commit (detached HEAD), rename,
//! delete, and what would be lost by deleting a branch or leaving a detached HEAD.
//!
//! Names are checked before git sees them and passed after `--end-of-options`; a switch that
//! would overwrite local changes is reported with the files, never forced.

use crate::repo::NameError;
use crate::{Cancel, GitError, Oid, Repository, Written};

/// Commits listed at most for a loss warning.
const LOSS_LIMIT: usize = 50;

/// Why a name was refused (for [`GitError::InvalidName`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    LeadingDash,
    Whitespace,
    /// Breaks git's rules for ref names.
    Rules,
}

/// A commit that would become unreachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostCommit {
    pub id: Oid,
    pub subject: String,
}

/// Commits that only one ref (or a detached HEAD) holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loss {
    /// The newest of them, at most [`LOSS_LIMIT`].
    pub commits: Vec<LostCommit>,
    /// How many there are in all.
    pub total: usize,
}

impl Repository {
    fn branch_name_ok(&self, name: &str) -> Result<(), GitError> {
        self.check_branch_name(name).map_err(|e| match e {
            NameError::Git(e) => e,
            NameError::Empty => invalid(name, NameProblem::Empty),
            NameError::LeadingDash => invalid(name, NameProblem::LeadingDash),
            NameError::Whitespace => invalid(name, NameProblem::Whitespace),
            NameError::Rules => invalid(name, NameProblem::Rules),
        })
    }

    /// Creates branch `name` at `start` (a commit id or ref) and, with `checkout`, switches to
    /// it (local changes are carried along).
    pub fn create_branch(
        &self,
        name: &str,
        start: &str,
        checkout: bool,
        cancel: &Cancel,
    ) -> Result<Written, GitError> {
        self.branch_name_ok(name)?;
        let result = if checkout {
            self.write(
                [
                    "switch",
                    "--no-guess",
                    "-c",
                    name,
                    "--end-of-options",
                    start,
                ],
                None,
                cancel,
            )
        } else {
            self.write(["branch", "--end-of-options", name, start], None, cancel)
        };
        result.map_err(overwrite)
    }

    /// Switches to local branch `name`, carrying local changes when git can.
    pub fn switch_branch(&self, name: &str, cancel: &Cancel) -> Result<Written, GitError> {
        self.branch_name_ok(name)?;
        self.write(
            ["switch", "--no-guess", "--end-of-options", name],
            None,
            cancel,
        )
        .map_err(overwrite)
    }

    /// Checks out a commit without a branch (detached HEAD).
    pub fn switch_detached(&self, commit: &Oid, cancel: &Cancel) -> Result<Written, GitError> {
        self.write(
            ["switch", "--detach", "--end-of-options", commit.as_str()],
            None,
            cancel,
        )
        .map_err(overwrite)
    }

    /// Renames a local branch; git moves its upstream setting along.
    pub fn rename_branch(
        &self,
        old: &str,
        new: &str,
        cancel: &Cancel,
    ) -> Result<Written, GitError> {
        self.branch_name_ok(old)?;
        self.branch_name_ok(new)?;
        self.write(["branch", "-m", "--end-of-options", old, new], None, cancel)
    }

    /// Deletes a local branch, also when its commits are on no other branch (the caller has
    /// shown [`Repository::branch_loss`] and the user confirmed). Returns the commit the branch
    /// pointed to, to restore it. The current branch cannot be deleted.
    pub fn delete_branch(&self, name: &str, cancel: &Cancel) -> Result<Oid, GitError> {
        self.branch_name_ok(name)?;
        let tip = self.resolve(&format!("refs/heads/{name}"))?;
        self.write(["branch", "-D", "--end-of-options", name], None, cancel)?;
        Ok(tip)
    }

    /// The commits of branch `name` that no other branch, remote branch or tag (nor a
    /// detached HEAD) contains: deleting the branch would leave them unreachable.
    pub fn branch_loss(&self, name: &str) -> Result<Loss, GitError> {
        self.branch_name_ok(name)?;
        let full = format!("refs/heads/{name}");
        // For `--branches` the pattern is the name without `refs/heads/`; a valid name has no
        // glob characters.
        let exclude = format!("--exclude={name}");
        let mut args = vec![
            full.as_str(),
            "--not",
            exclude.as_str(),
            "--branches",
            "--remotes",
            "--tags",
        ];
        // A detached HEAD keeps commits reachable too.
        if self.head_is_detached()? {
            args.push("HEAD");
        }
        self.loss(&args)
    }

    /// While HEAD is detached: the commits only it holds (leaving would lose them).
    pub fn detached_loss(&self) -> Result<Loss, GitError> {
        if !self.head_is_detached()? {
            return Ok(Loss::default());
        }
        self.loss(&["HEAD", "--not", "--branches", "--remotes", "--tags"])
    }

    fn loss(&self, revs: &[&str]) -> Result<Loss, GitError> {
        // `revs` are built here (checked names as `refs/heads/…`, `HEAD`) and mix revisions
        // with `--not`/`--branches`, so no `--end-of-options`.
        let mut args: Vec<&str> = vec!["log", "--no-color", "--format=%H%x1f%s"];
        args.extend_from_slice(revs);
        let out = self.read(args)?;
        let mut loss = Loss::default();
        for line in String::from_utf8_lossy(&out).lines() {
            loss.total += 1;
            if loss.commits.len() < LOSS_LIMIT
                && let Some((id, subject)) = line.split_once('\u{1f}')
                && let Some(id) = Oid::parse(id)
            {
                loss.commits.push(LostCommit {
                    id,
                    subject: subject.to_owned(),
                });
            }
        }
        Ok(loss)
    }

    fn head_is_detached(&self) -> Result<bool, GitError> {
        match self.read(["symbolic-ref", "--quiet", "HEAD"]) {
            Ok(_) => Ok(false),
            Err(GitError::Failed { code: Some(1), .. }) => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn resolve(&self, rev: &str) -> Result<Oid, GitError> {
        let spec = format!("{rev}^{{commit}}");
        let out = self.read([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            spec.as_str(),
        ])?;
        let text = String::from_utf8_lossy(&out);
        Oid::parse(text.trim())
            .ok_or_else(|| GitError::Parse(format!("rev-parse: {}", text.trim())))
    }
}

fn invalid(name: &str, problem: NameProblem) -> GitError {
    GitError::InvalidName {
        name: name.to_owned(),
        problem,
    }
}

/// Turns git's refusal to overwrite local changes into [`GitError::WouldOverwrite`] with the
/// files it named.
fn overwrite(e: GitError) -> GitError {
    let GitError::Failed { stderr, .. } = &e else {
        return e;
    };
    if !stderr.contains("would be overwritten by checkout")
        && !stderr.contains("would be overwritten by switch")
    {
        return e;
    }
    let files = stderr
        .lines()
        .filter(|l| l.starts_with('\t'))
        .map(|l| l.trim().to_owned())
        .collect();
    GitError::WouldOverwrite(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrite_lists_the_files() {
        let e = GitError::Failed {
            command: "switch".into(),
            code: Some(1),
            stderr: "error: Your local changes to the following files would be overwritten by checkout:\n\ta.txt\n\tdir/b.txt\nPlease commit your changes or stash them before you switch branches.\nAborting".into(),
        };
        assert_eq!(
            overwrite(e),
            GitError::WouldOverwrite(vec!["a.txt".into(), "dir/b.txt".into()])
        );
        let other = GitError::Failed {
            command: "switch".into(),
            code: Some(128),
            stderr: "fatal: invalid reference: x".into(),
        };
        assert_eq!(overwrite(other.clone()), other);
    }
}
