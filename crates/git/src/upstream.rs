//! Upstreams and shallow repositories (issue #47): set or remove what a branch tracks; find out
//! whether the repository is shallow, where its history is cut, and fetch more of it.

use crate::{Cancel, GitError, Oid, OnProgress, Repository, Written};

/// Boundary commits read at most (a shallow file lists one per cut line of history).
const MAX_BOUNDARY: usize = 10_000;

impl Repository {
    /// Lets local branch `branch` track `upstream` (a full ref name such as
    /// `refs/remotes/origin/main`) (`branch --set-upstream-to=<upstream> --end-of-options
    /// <branch>`).
    pub fn set_upstream(
        &self,
        branch: &str,
        upstream: &str,
        cancel: &Cancel,
    ) -> Result<Written, GitError> {
        self.check_branch_name(branch)
            .map_err(|_| GitError::InvalidValue(branch.to_owned()))?;
        if !(upstream.starts_with("refs/remotes/") || upstream.starts_with("refs/heads/")) {
            return Err(GitError::InvalidValue(upstream.to_owned()));
        }
        let to = format!("--set-upstream-to={upstream}");
        self.write(
            ["branch", to.as_str(), "--end-of-options", branch],
            None,
            cancel,
        )
    }

    /// Lets `branch` track nothing (`branch --unset-upstream --end-of-options <branch>`).
    pub fn unset_upstream(&self, branch: &str, cancel: &Cancel) -> Result<Written, GitError> {
        self.check_branch_name(branch)
            .map_err(|_| GitError::InvalidValue(branch.to_owned()))?;
        self.write(
            ["branch", "--unset-upstream", "--end-of-options", branch],
            None,
            cancel,
        )
    }

    /// The commits where a shallow repository's history is cut; empty when it is complete
    /// (`rev-parse --is-shallow-repository`, then the `shallow` file).
    pub fn shallow_boundary(&self) -> Result<Vec<Oid>, GitError> {
        let out = self.read(["rev-parse", "--is-shallow-repository"])?;
        if String::from_utf8_lossy(&out).trim() != "true" {
            return Ok(Vec::new());
        }
        let out = self.read(["rev-parse", "--git-path", "shallow"])?;
        let rel = String::from_utf8_lossy(&out).trim().to_owned();
        let path = self.workdir().join(rel);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Ok(text
            .lines()
            .take(MAX_BOUNDARY)
            .filter_map(|l| Oid::parse(l.trim()))
            .collect())
    }

    /// Fetches `by` more commits of history (`fetch --progress --deepen=<n>`), or all of it
    /// (`fetch --progress --unshallow`), from the current branch's remote.
    pub fn deepen(
        &self,
        by: Option<u32>,
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<Written, GitError> {
        let deepen = by.map(|n| format!("--deepen={}", n.max(1)));
        let mut args = vec!["fetch", "--progress"];
        match &deepen {
            Some(d) => args.push(d),
            None => args.push("--unshallow"),
        }
        self.runner_ref()
            .write_progress(self.workdir(), &[], args, cancel, on_progress, &[])
            .map_err(crate::network::network)
    }
}
