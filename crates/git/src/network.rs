//! Talking to remotes (issue #43): fetch with progress and the new commits per branch; pull
//! fast-forward only by default, merge or rebase only when chosen.
//!
//! These calls may take long and can be cancelled: git and everything it started (ssh, remote
//! helpers) is stopped; git updates refs only after it has received everything, so a cancelled
//! fetch leaves the repository as it was.

use std::collections::BTreeMap;

use crate::command_log::redact;
use crate::{Cancel, GitError, Oid, OnProgress, Repository, Written};

/// Changed remote-tracking branches counted at most (each needs one `rev-list`).
const COUNTED: usize = 50;

/// How a remote-tracking branch or tag changed in a fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    /// Short name (`origin/main`, `v1.0` for tags).
    pub name: String,
    pub tag: bool,
    /// `None`: new.
    pub old: Option<Oid>,
    /// `None`: removed (pruned).
    pub new: Option<Oid>,
    /// Commits that came in (`old..new`); `None` for new or removed refs and beyond the
    /// counted ones.
    pub commits: Option<u32>,
    /// The branch was rewritten (its old tip is not in the new history).
    pub forced: bool,
}

/// How to bring in the upstream's commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    /// Only when the local branch has no commits of its own (`--ff-only`).
    FastForward,
    /// Fast-forward, or a merge commit when both sides have commits (`--no-rebase --ff`).
    Merge,
    /// Replay the local commits on top of the upstream (`--rebase`).
    Rebase,
}

impl Repository {
    /// Remote-tracking branches and tags with their objects.
    fn fetched_refs(&self) -> Result<BTreeMap<String, Oid>, GitError> {
        let out = self.read([
            "for-each-ref",
            "--format=%(refname)%1f%(objectname)",
            "refs/remotes",
            "refs/tags",
        ])?;
        let text = String::from_utf8_lossy(&out);
        let mut refs = BTreeMap::new();
        for line in text.lines() {
            if let Some((name, id)) = line.split_once('\u{1f}')
                && let Some(id) = Oid::parse(id)
            {
                refs.insert(name.to_owned(), id);
            }
        }
        Ok(refs)
    }

    /// Fetches `remote` (or all remotes), with `prune` removing remote-tracking branches whose
    /// branch is gone (`fetch --progress [--prune] (--all | --end-of-options <remote>)`).
    /// Returns what changed.
    pub fn fetch(
        &self,
        remote: Option<&str>,
        prune: bool,
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<Vec<RefUpdate>, GitError> {
        if let Some(r) = remote {
            self.remote_name_ok(r)?;
        }
        let before = self.fetched_refs()?;
        let mut args = vec!["fetch", "--progress"];
        if prune {
            args.push("--prune");
        }
        match remote {
            Some(r) => args.extend(["--end-of-options", r]),
            None => args.push("--all"),
        }
        self.runner_ref()
            .write_progress(self.workdir(), &[], args, cancel, on_progress, &[])
            .map_err(network)?;
        let after = self.fetched_refs()?;
        self.ref_updates(&before, &after)
    }

    fn ref_updates(
        &self,
        before: &BTreeMap<String, Oid>,
        after: &BTreeMap<String, Oid>,
    ) -> Result<Vec<RefUpdate>, GitError> {
        let mut updates = Vec::new();
        let names = before
            .keys()
            .chain(after.keys().filter(|k| !before.contains_key(*k)));
        for full in names {
            let (old, new) = (before.get(full), after.get(full));
            if old == new {
                continue;
            }
            let (name, tag) = match full.strip_prefix("refs/remotes/") {
                Some(n) => (n, false),
                None => (full.strip_prefix("refs/tags/").unwrap_or(full), true),
            };
            // `origin/HEAD` follows the default branch; nothing to report about it.
            if !tag && name.ends_with("/HEAD") {
                continue;
            }
            updates.push(RefUpdate {
                name: name.to_owned(),
                tag,
                old: old.cloned(),
                new: new.cloned(),
                commits: None,
                forced: false,
            });
        }
        for u in updates.iter_mut().filter(|u| !u.tag).take(COUNTED) {
            if let (Some(old), Some(new)) = (&u.old, &u.new) {
                let range = format!("{}..{}", old.as_str(), new.as_str());
                let out = self.read(["rev-list", "--count", range.as_str()])?;
                u.commits = String::from_utf8_lossy(&out).trim().parse().ok();
                let base = self.read(["merge-base", "--is-ancestor", old.as_str(), new.as_str()]);
                u.forced = matches!(base, Err(GitError::Failed { code: Some(1), .. }));
            }
        }
        Ok(updates)
    }

    /// Commits the current branch has that its upstream lacks, and the other way round
    /// (`rev-list --left-right --count HEAD...@{upstream}`); fails with
    /// [`GitError::NoUpstream`] when the branch tracks nothing.
    pub fn divergence(&self) -> Result<(u32, u32), GitError> {
        let out = match self.read(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
            Ok(out) => out,
            Err(GitError::Failed { .. }) => return Err(GitError::NoUpstream),
            Err(e) => return Err(e),
        };
        let text = String::from_utf8_lossy(&out);
        let mut parts = text.split_whitespace().map(str::parse::<u32>);
        match (parts.next(), parts.next()) {
            (Some(Ok(ahead)), Some(Ok(behind))) => Ok((ahead, behind)),
            _ => Err(GitError::Parse(format!("rev-list: {}", text.trim()))),
        }
    }

    /// Pulls the upstream of the current branch the chosen way (`pull --progress
    /// --no-rebase --ff-only`, `--no-rebase --ff --no-edit`, or `--rebase`), never by what the
    /// configuration happens to say. A fast-forward that is not possible fails with
    /// [`GitError::Diverged`] before anything changed; conflicts with
    /// [`GitError::MergeConflicts`], leaving the merge or rebase in progress.
    pub fn pull(
        &self,
        mode: PullMode,
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<Written, GitError> {
        let (ahead, behind) = self.divergence()?;
        if mode == PullMode::FastForward && ahead > 0 && behind > 0 {
            return Err(GitError::Diverged { ahead, behind });
        }
        let mut args = vec!["pull", "--progress"];
        args.extend(match mode {
            PullMode::FastForward => &["--no-rebase", "--ff-only"][..],
            PullMode::Merge => &["--no-rebase", "--ff", "--no-edit"][..],
            PullMode::Rebase => &["--rebase"][..],
        });
        match self
            .runner_ref()
            .write_progress(self.workdir(), &[], args, cancel, on_progress, &[])
        {
            Ok(w) => Ok(w),
            Err(GitError::Failed { stderr, .. })
                if self.status().is_ok_and(|s| s.conflicts().next().is_some()) =>
            {
                Err(GitError::MergeConflicts(redact(&stderr)))
            }
            Err(e) => match (mode, network(e)) {
                // The upstream moved on during the fetch: report it like the check above.
                (PullMode::FastForward, GitError::Failed { stderr, .. })
                    if stderr.contains("fast-forward") =>
                {
                    let (ahead, behind) = self.divergence()?;
                    Err(GitError::Diverged { ahead, behind })
                }
                (_, e) => Err(e),
            },
        }
    }
}

/// Recognises a refused login and hides credentials in git's message.
pub(crate) fn network(e: GitError) -> GitError {
    match e {
        GitError::Failed {
            command,
            code,
            stderr,
        } => {
            let stderr = redact(&stderr);
            let auth = [
                "terminal prompts disabled",
                "Authentication failed",
                "could not read Username",
                "could not read Password",
                "Permission denied (publickey",
                "Host key verification failed",
            ]
            .iter()
            .any(|m| stderr.contains(m));
            if auth {
                GitError::Auth(stderr)
            } else {
                GitError::Failed {
                    command,
                    code,
                    stderr,
                }
            }
        }
        e => e,
    }
}
