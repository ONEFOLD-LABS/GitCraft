//! Git calls off the UI thread: each job runs on its own thread and sends its outcome back;
//! the window collects outcomes once per frame. The UI thread never waits for git.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use gitcraft_git::{
    ChangedFile, Commit, FileDiff, GitError, GitRunner, GitVersion, Ref, RepoPath, Repository,
    Stash, Status,
};

/// What the window shows of a repository besides the history: read together so that status,
/// refs and stashes always belong to the same moment.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub status: Status,
    pub refs: Vec<Ref>,
    pub stashes: Vec<Stash>,
}

impl Snapshot {
    pub(crate) fn read(repo: &Repository) -> Result<Snapshot, GitError> {
        Ok(Snapshot {
            status: repo.status()?,
            refs: repo.refs()?,
            stashes: repo.stashes()?,
        })
    }
}

/// Which diff to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffKey {
    Commit {
        commit: Box<Commit>,
        file: ChangedFile,
    },
    Worktree {
        path: RepoPath,
        old: Option<RepoPath>,
        staged: bool,
    },
    Untracked {
        path: RepoPath,
    },
}

/// Work for git.
pub(crate) enum Job {
    /// Message and changed files of a commit.
    CommitDetails(Repository, Box<Commit>),
    /// The diff of one file.
    Diff(Repository, DiffKey),
    /// Check the installed git, open the repository at the path, read its snapshot.
    Open(PathBuf),
    /// Read the snapshot of an opened repository again.
    Refresh(Repository),
    /// Read the installed git version.
    Version,
}

/// What a job produced.
pub(crate) enum Outcome {
    CommitDetails {
        id: gitcraft_git::Oid,
        result: Result<(String, Vec<ChangedFile>), GitError>,
    },
    Diff {
        key: DiffKey,
        result: Result<FileDiff, GitError>,
    },
    Opened(Box<(Repository, Snapshot)>),
    Refreshed(Box<Snapshot>),
    Version(GitVersion),
    Failed(GitError),
}

pub(crate) struct Worker {
    tx: Sender<(u64, Outcome)>,
    rx: Receiver<(u64, Outcome)>,
}

impl Worker {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        Worker { tx, rx }
    }

    /// Starts `job`; its outcome arrives tagged with `generation`, and `ctx` is asked to repaint.
    pub(crate) fn start(
        &self,
        generation: u64,
        job: Job,
        runner: GitRunner,
        ctx: egui::Context,
    ) -> Result<(), String> {
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("gitcraft-git".into())
            .spawn(move || {
                let outcome = run(job, &runner).unwrap_or_else(Outcome::Failed);
                // The window may be gone; then nobody needs the result.
                let _ = tx.send((generation, outcome));
                ctx.request_repaint();
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Outcomes that arrived since the last call.
    pub(crate) fn poll(&self) -> Vec<(u64, Outcome)> {
        self.rx.try_iter().collect()
    }
}

fn run(job: Job, runner: &GitRunner) -> Result<Outcome, GitError> {
    Ok(match job {
        Job::Open(path) => {
            runner.version()?;
            let repo = Repository::open(runner, &path)?;
            let snapshot = Snapshot::read(&repo)?;
            Outcome::Opened(Box::new((repo, snapshot)))
        }
        Job::Refresh(repo) => Outcome::Refreshed(Box::new(Snapshot::read(&repo)?)),
        Job::Version => Outcome::Version(runner.version()?),
        Job::CommitDetails(repo, commit) => Outcome::CommitDetails {
            id: commit.id.clone(),
            result: repo
                .commit_message(&commit)
                .and_then(|m| Ok((m, repo.commit_files(&commit)?))),
        },
        Job::Diff(repo, key) => {
            let result = match &key {
                DiffKey::Commit { commit, file } => repo.commit_diff(commit, file),
                DiffKey::Worktree { path, old, staged } => {
                    repo.worktree_diff(path, old.as_ref(), *staged)
                }
                DiffKey::Untracked { path } => repo.untracked_diff(path),
            };
            Outcome::Diff { key, result }
        }
    })
}
