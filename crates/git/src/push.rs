//! Push (issue #44): the current branch to its upstream (the first push sets it), tags, and a
//! forced push only with a lease on exactly the remote commit the user was shown, so that
//! commits that reached the remote since are never overwritten.
//!
//! `push --porcelain` reports every ref on stdout, also when the remote refuses some; those
//! lines become [`PushedRef`]s and refusals a [`GitError::PushRejected`].

use crate::network::network;
use crate::{Cancel, GitError, Loss, Oid, OnProgress, Repository};

/// What happened to one ref, as `push --porcelain` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushFlag {
    FastForward,
    Forced,
    Deleted,
    New,
    Rejected,
    UpToDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedRef {
    pub flag: PushFlag,
    /// The local side (`refs/heads/main`).
    pub from: String,
    /// The remote ref (`refs/heads/main`).
    pub to: String,
    /// git's summary (`1111111..2222222`, `[new branch]`, `[rejected]`).
    pub summary: String,
    /// Why, for rejected refs (`non-fast-forward`, `stale info`, a hook's message).
    pub reason: Option<String>,
}

/// Why the remote refused a ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The remote has commits the local branch lacks (pull first).
    NonFastForward,
    /// A forced push whose lease failed: the remote moved since it was shown.
    Stale,
    /// The remote's own refusal (a hook, branch protection), with its reason.
    Remote(String),
    Other(String),
}

/// Where the current branch goes and what a forced push there would overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub branch: String,
    pub remote: String,
    /// The remote branch (`refs/heads/…`).
    pub merge: String,
    /// The branch has no upstream yet (the push sets it).
    pub first: bool,
    /// The remote-tracking commit of the remote branch, as last fetched.
    pub tracking: Option<Oid>,
}

impl Repository {
    fn config_value(&self, key: &str) -> Result<Option<String>, GitError> {
        match self.read(["config", "--get", "--end-of-options", key]) {
            Ok(out) => Ok(Some(String::from_utf8_lossy(&out).trim().to_owned())),
            Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Where a push of the current branch goes: its upstream, or (`first`) the same name on
    /// `remote` (when given) or the only/`origin` remote.
    pub fn push_target(&self, remote: Option<&str>) -> Result<PushTarget, GitError> {
        let out = match self.read(["symbolic-ref", "--quiet", "--short", "HEAD"]) {
            Ok(out) => out,
            Err(GitError::Failed { code: Some(1), .. }) => return Err(GitError::NoUpstream),
            Err(e) => return Err(e),
        };
        let branch = String::from_utf8_lossy(&out).trim().to_owned();
        let configured = (
            self.config_value(&format!("branch.{branch}.remote"))?,
            self.config_value(&format!("branch.{branch}.merge"))?,
        );
        let (remote, merge, first) = match (configured, remote) {
            ((Some(r), Some(m)), None) if r != "." => (r, m, false),
            (_, Some(r)) => (r.to_owned(), format!("refs/heads/{branch}"), true),
            _ => {
                let remotes = self.remotes()?;
                let pick = remotes
                    .iter()
                    .find(|r| r.name == "origin")
                    .or(if remotes.len() == 1 {
                        remotes.first()
                    } else {
                        None
                    })
                    .ok_or(GitError::NoUpstream)?;
                (pick.name.clone(), format!("refs/heads/{branch}"), true)
            }
        };
        self.remote_name_ok(&remote)?;
        let tracking_ref = merge
            .strip_prefix("refs/heads/")
            .map(|b| format!("refs/remotes/{remote}/{b}"));
        let tracking = match &tracking_ref {
            Some(r) => self.resolve_commit(r).ok(),
            None => None,
        };
        Ok(PushTarget {
            branch,
            remote,
            merge,
            first,
            tracking,
        })
    }

    /// The commits a forced push to `target` would remove from the remote branch (as last
    /// fetched).
    pub fn force_loss(&self, target: &PushTarget) -> Result<Loss, GitError> {
        match &target.tracking {
            Some(tip) => self.loss(&[tip.as_str(), "--not", "HEAD"]),
            None => Ok(Loss::default()),
        }
    }

    /// Pushes the current branch to `target` (`push --progress --porcelain [--set-upstream]
    /// --end-of-options <remote> refs/heads/<branch>:<merge>`). With `lease` the push is forced,
    /// but only while the remote branch is still at that commit (`--force-with-lease=<merge>:
    /// <commit>`; `Some(None)`: only while it does not exist).
    pub fn push(
        &self,
        target: &PushTarget,
        lease: Option<Option<Oid>>,
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<Vec<PushedRef>, GitError> {
        self.remote_name_ok(&target.remote)?;
        if target.merge.starts_with('-') || target.branch.starts_with('-') {
            return Err(GitError::InvalidValue(target.merge.clone()));
        }
        let lease_arg = lease.map(|expect| {
            format!(
                "--force-with-lease={}:{}",
                target.merge,
                expect.as_ref().map_or("", Oid::as_str)
            )
        });
        let refspec = format!("refs/heads/{}:{}", target.branch, target.merge);
        let mut args = vec!["push", "--progress", "--porcelain"];
        if target.first {
            args.push("--set-upstream");
        }
        if let Some(l) = &lease_arg {
            args.push(l);
        }
        args.extend(["--end-of-options", target.remote.as_str(), refspec.as_str()]);
        self.push_args(&args, cancel, on_progress)
    }

    /// Pushes one tag, or all tags (`push --progress --porcelain [--tags] --end-of-options
    /// <remote> [refs/tags/<tag>]`).
    pub fn push_tags(
        &self,
        remote: &str,
        tag: Option<&str>,
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<Vec<PushedRef>, GitError> {
        self.remote_name_ok(remote)?;
        if let Some(t) = tag {
            self.tag_name_ok(t)?;
        }
        let refspec = tag.map(|t| format!("refs/tags/{t}"));
        let mut args = vec!["push", "--progress", "--porcelain"];
        if tag.is_none() {
            args.push("--tags");
        }
        args.extend(["--end-of-options", remote]);
        if let Some(r) = &refspec {
            args.push(r);
        }
        self.push_args(&args, cancel, on_progress)
    }

    fn push_args(
        &self,
        args: &[&str],
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<Vec<PushedRef>, GitError> {
        let (written, ok) =
            self.runner_ref()
                .write_progress_any(self.workdir(), args, cancel, on_progress)?;
        let refs = parse_porcelain(&String::from_utf8_lossy(&written.stdout));
        let rejected: Vec<(String, Rejection)> = refs
            .iter()
            .filter(|r| r.flag == PushFlag::Rejected)
            .map(|r| (r.to.clone(), rejection(r)))
            .collect();
        if !rejected.is_empty() {
            return Err(GitError::PushRejected(rejected));
        }
        if !ok {
            return Err(network(GitError::Failed {
                command: "push".into(),
                code: None,
                stderr: written.stderr,
            }));
        }
        Ok(refs)
    }
}

fn rejection(r: &PushedRef) -> Rejection {
    let reason = r.reason.clone().unwrap_or_default();
    if r.summary.contains("remote rejected") {
        return Rejection::Remote(reason);
    }
    match reason.as_str() {
        "non-fast-forward" | "fetch first" => Rejection::NonFastForward,
        "stale info" | "remote ref updated since checkout" => Rejection::Stale,
        _ => Rejection::Other(reason),
    }
}

/// Parses `push --porcelain` (`<flag>\t<from>:<to>\t<summary> (<reason>)` lines between
/// `To <url>` and `Done`).
fn parse_porcelain(out: &str) -> Vec<PushedRef> {
    let mut refs = Vec::new();
    for line in out.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(flag), Some(spec), Some(rest)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let flag = match flag {
            " " => PushFlag::FastForward,
            "+" => PushFlag::Forced,
            "-" => PushFlag::Deleted,
            "*" => PushFlag::New,
            "!" => PushFlag::Rejected,
            "=" => PushFlag::UpToDate,
            _ => continue,
        };
        let (from, to) = spec.split_once(':').unwrap_or(("", spec));
        let (summary, reason) = match rest.find(" (") {
            Some(i) if rest.ends_with(')') => (
                rest[..i].to_owned(),
                Some(rest[i + 2..rest.len() - 1].to_owned()),
            ),
            _ => (rest.to_owned(), None),
        };
        refs.push(PushedRef {
            flag,
            from: from.to_owned(),
            to: to.to_owned(),
            summary,
            reason,
        });
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_lines() {
        let out = "To /srv/up.git\n \trefs/heads/main:refs/heads/main\t1111111..2222222\n*\tHEAD:refs/heads/neu\t[new branch]\n!\trefs/heads/x:refs/heads/x\t[rejected] (non-fast-forward)\n!\trefs/heads/y:refs/heads/y\t[remote rejected] (hook declined: no (really))\n!\trefs/heads/z:refs/heads/z\t[rejected] (stale info)\n=\trefs/tags/v1:refs/tags/v1\t[up to date]\nDone\n";
        let refs = parse_porcelain(out);
        assert_eq!(refs.len(), 6);
        assert_eq!(refs[0].flag, PushFlag::FastForward);
        assert_eq!(refs[1].flag, PushFlag::New);
        assert_eq!(refs[1].from, "HEAD");
        assert_eq!(rejection(&refs[2]), Rejection::NonFastForward);
        assert_eq!(
            rejection(&refs[3]),
            Rejection::Remote("hook declined: no (really)".into())
        );
        assert_eq!(rejection(&refs[4]), Rejection::Stale);
        assert_eq!(refs[5].flag, PushFlag::UpToDate);
    }
}
