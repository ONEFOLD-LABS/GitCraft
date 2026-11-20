//! Git calls off the UI thread: each job runs on its own thread and sends its outcome back;
//! the window collects outcomes once per frame. The UI thread never waits for git.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use gitcraft_git::{GitError, GitRunner, GitVersion, Repository, Status};

/// Work for git.
pub(crate) enum Job {
    /// Check the installed git, open the repository at the path, read its status.
    Open(PathBuf),
    /// Read the status of an opened repository again.
    Refresh(Repository),
    /// Read the installed git version.
    Version,
}

/// What a job produced.
pub(crate) enum Outcome {
    Opened(Box<(Repository, Status)>),
    Refreshed(Status),
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
            let status = repo.status()?;
            Outcome::Opened(Box::new((repo, status)))
        }
        Job::Refresh(repo) => Outcome::Refreshed(repo.status()?),
        Job::Version => Outcome::Version(runner.version()?),
    })
}
