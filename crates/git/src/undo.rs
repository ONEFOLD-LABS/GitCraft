//! Building blocks for undo and redo (issue #84): where HEAD is, moving the current branch
//! without touching the index or working tree, and removing a ref only if it still points
//! where GitCraft left it. Every step checks that nothing moved in between and never discards
//! uncommitted work.

use crate::{Cancel, GitError, Oid, Repository};

/// Where HEAD is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadPos {
    /// On a branch with commits.
    Branch(String),
    /// On a branch without commits yet.
    Unborn(String),
    Detached(Oid),
}

impl Repository {
    /// The commit HEAD points at; `None` on an unborn branch.
    pub fn head_commit(&self) -> Result<Option<Oid>, GitError> {
        match self.read(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]) {
            Ok(out) => Ok(Oid::parse(String::from_utf8_lossy(&out).trim())),
            Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn head_pos(&self) -> Result<HeadPos, GitError> {
        let commit = self.head_commit()?;
        match self.read(["symbolic-ref", "--quiet", "--short", "HEAD"]) {
            Ok(out) => {
                let name = String::from_utf8_lossy(&out).trim().to_owned();
                Ok(match commit {
                    Some(_) => HeadPos::Branch(name),
                    None => HeadPos::Unborn(name),
                })
            }
            Err(GitError::Failed { code: Some(1), .. }) => commit
                .map(HeadPos::Detached)
                .ok_or_else(|| GitError::Parse("detached HEAD without commit".into())),
            Err(e) => Err(e),
        }
    }

    /// Whether a remote-tracking branch contains the commit (then rewriting it needs a force
    /// push).
    pub fn on_remote(&self, id: &Oid) -> Result<bool, GitError> {
        let contains = format!("--contains={}", id.as_str());
        let out = self.read([
            "for-each-ref",
            "--count=1",
            contains.as_str(),
            "--format=%(refname)",
            "refs/remotes",
        ])?;
        Ok(!out.is_empty())
    }

    /// Moves the current branch from `expected` to `to` (`None`: back to no commit) with
    /// `reset --soft`: index and working tree stay as they are, so nothing uncommitted is lost.
    /// Refuses when HEAD is not at `expected`, and with `refuse_pushed` when a remote already
    /// has `expected`.
    pub fn move_head(
        &self,
        expected: Option<&Oid>,
        to: Option<&Oid>,
        refuse_pushed: bool,
        cancel: &Cancel,
    ) -> Result<(), GitError> {
        if self.head_commit()?.as_ref() != expected {
            return Err(GitError::Stale);
        }
        if refuse_pushed
            && let Some(e) = expected
            && self.on_remote(e)?
        {
            return Err(GitError::Pushed);
        }
        match to {
            Some(to) => {
                self.write(
                    ["reset", "--soft", "--end-of-options", to.as_str()],
                    None,
                    cancel,
                )?;
            }
            None => {
                let HeadPos::Branch(name) = self.head_pos()? else {
                    return Err(GitError::InvalidValue("HEAD".into()));
                };
                let full = format!("refs/heads/{name}");
                let old = expected.map(Oid::as_str).unwrap_or_default();
                self.write(["update-ref", "-d", full.as_str(), old], None, cancel)?;
            }
        }
        Ok(())
    }

    /// Removes a branch or tag ref only while it still points at `expected`.
    pub fn delete_ref_if(
        &self,
        full: &str,
        expected: &Oid,
        cancel: &Cancel,
    ) -> Result<(), GitError> {
        let allowed = full
            .strip_prefix("refs/heads/")
            .or_else(|| full.strip_prefix("refs/tags/"))
            .is_some_and(|n| !n.is_empty() && !n.starts_with('-'));
        if !allowed {
            return Err(GitError::InvalidValue(full.to_owned()));
        }
        match self.write(["update-ref", "-d", full, expected.as_str()], None, cancel) {
            Err(GitError::Failed { .. }) => Err(GitError::Stale),
            other => other.map(|_| ()),
        }
    }
}
