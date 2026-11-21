//! A clone running in the background (issue #46): progress to show, a way to cancel, and the
//! new working tree when it is done.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gitcraft_git::{Cancel, CloneOptions, GitError, GitRunner, OnProgress, Progress};

use crate::{Task, Wake};

pub struct CloneJob {
    cancel: Cancel,
    progress: Arc<Mutex<Option<Progress>>>,
    task: Task<Result<PathBuf, GitError>>,
}

impl CloneJob {
    /// Starts `git clone` on its own thread; `wake` is called on progress (a few times per
    /// second) and at the end.
    pub fn start(
        runner: GitRunner,
        options: CloneOptions,
        wake: Wake,
    ) -> Result<CloneJob, GitError> {
        let cancel = Cancel::new();
        let progress = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&progress);
        let waker = wake.clone();
        let last = Mutex::new(Instant::now());
        let on_progress: OnProgress = Arc::new(move |p: Progress| {
            if let Ok(mut slot) = latest.lock() {
                *slot = Some(p);
            }
            if let Ok(mut at) = last.lock()
                && at.elapsed() >= Duration::from_millis(50)
            {
                *at = Instant::now();
                waker();
            }
        });
        let stop = cancel.clone();
        let task = Task::spawn("gitcraft-clone", wake, move || {
            runner.clone_repository(&options, &stop, on_progress)
        })?;
        Ok(CloneJob {
            cancel,
            progress,
            task,
        })
    }

    /// The latest progress report.
    pub fn progress(&self) -> Option<Progress> {
        self.progress.lock().ok().and_then(|p| p.clone())
    }

    /// Stops git; the result then reports [`GitError::Cancelled`] after cleaning up.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The result once the clone has ended (exactly once).
    pub fn poll(&mut self) -> Option<Result<PathBuf, GitError>> {
        self.task.poll()
    }
}
