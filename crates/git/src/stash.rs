//! Stashes (issue #41): park changes, look at them, apply, pop, drop and restore a dropped
//! one by its id.

use crate::log::FORMAT;
use crate::{Cancel, Commit, GitError, Oid, Repository, Written};

impl Repository {
    /// Parks the working tree's changes. `message` is attached to its option (never read as
    /// one); `untracked` includes untracked files, `keep_index` leaves staged changes in place.
    /// The new stash's id; `None` when there was nothing to stash.
    pub fn stash_push(
        &self,
        message: &str,
        untracked: bool,
        keep_index: bool,
        cancel: &Cancel,
    ) -> Result<Option<Oid>, GitError> {
        let message_arg = format!("--message={}", message.trim());
        let mut args = vec!["stash", "push", message_arg.as_str()];
        if message.trim().is_empty() {
            args.pop();
        }
        if untracked {
            args.push("--include-untracked");
        }
        if keep_index {
            args.push("--keep-index");
        }
        let out = self.write(args, None, cancel)?;
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("No local changes to save") {
            return Ok(None);
        }
        let top = self.read(["rev-parse", "--verify", "--quiet", "refs/stash"])?;
        Ok(Oid::parse(String::from_utf8_lossy(&top).trim()))
    }

    /// Applies a stash; with `pop` it is dropped afterwards (git keeps it when the apply has
    /// conflicts). `selector` (`stash@{n}`) must still name the stash `id`.
    pub fn stash_apply(
        &self,
        selector: &str,
        id: &Oid,
        pop: bool,
        cancel: &Cancel,
    ) -> Result<Written, GitError> {
        self.check_stash(selector, id)?;
        let action = if pop { "pop" } else { "apply" };
        match self.write(
            ["stash", action, "--end-of-options", selector],
            None,
            cancel,
        ) {
            Err(GitError::Failed {
                stderr,
                code,
                command,
            }) => {
                // git reports conflicts on stdout; the working tree tells for sure.
                let conflicted: Vec<String> = self
                    .status()
                    .map(|s| s.conflicts().map(|(f, _)| f.path.display()).collect())
                    .unwrap_or_default();
                if !conflicted.is_empty() {
                    return Err(GitError::Conflicts(conflicted.join(", ")));
                }
                Err(crate::branch::overwrite(GitError::Failed {
                    command,
                    code,
                    stderr,
                }))
            }
            other => other,
        }
    }

    /// Drops a stash (its id can put it back with [`Repository::stash_store`]).
    pub fn stash_drop(&self, selector: &str, id: &Oid, cancel: &Cancel) -> Result<(), GitError> {
        self.check_stash(selector, id)?;
        self.write(
            ["stash", "drop", "--quiet", "--end-of-options", selector],
            None,
            cancel,
        )?;
        Ok(())
    }

    /// Puts a dropped stash back on the list.
    pub fn stash_store(&self, id: &Oid, message: &str, cancel: &Cancel) -> Result<(), GitError> {
        let message_arg = format!("--message={message}");
        self.write(
            // `stash store` counts its arguments itself (no `--end-of-options`); the id is
            // checked hex.
            ["stash", "store", message_arg.as_str(), id.as_str()],
            None,
            cancel,
        )?;
        Ok(())
    }

    /// A commit by id (a stash, or one not loaded in the history).
    pub fn commit_by_id(&self, id: &Oid) -> Result<Commit, GitError> {
        let out = self.read([
            "log",
            "-1",
            "-z",
            "--no-color",
            FORMAT,
            "--end-of-options",
            id.as_str(),
        ])?;
        let record = out.split(|b| *b == 0).next().unwrap_or_default();
        Commit::parse(record)
    }

    /// `selector` must be a `stash@{n}` that still points at `id` (the list may have changed).
    fn check_stash(&self, selector: &str, id: &Oid) -> Result<(), GitError> {
        let well_formed = selector
            .strip_prefix("stash@{")
            .and_then(|r| r.strip_suffix('}'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !well_formed {
            return Err(GitError::InvalidValue(selector.to_owned()));
        }
        let out = self.read([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            selector,
        ])?;
        if String::from_utf8_lossy(&out).trim() != id.as_str() {
            return Err(GitError::Stale);
        }
        Ok(())
    }
}
