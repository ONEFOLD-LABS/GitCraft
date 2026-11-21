//! One piece of work on its own thread whose result is collected later without waiting (opening
//! a repository, reading the git version).

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use gitcraft_git::GitError;

use crate::Wake;

/// Work running on its own thread.
pub struct Task<T> {
    rx: Option<Receiver<T>>,
}

impl<T: Send + 'static> Task<T> {
    /// Runs `work` on a thread named `name`; `wake` is called when the result is there.
    pub fn spawn(
        name: &str,
        wake: Wake,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<Task<T>, GitError> {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                // Nobody waits any more when the task was dropped.
                let _ = tx.send(work());
                wake();
            })
            .map_err(|e| GitError::Spawn(e.to_string()))?;
        Ok(Task { rx: Some(rx) })
    }

    /// The result once it is there (exactly once).
    pub fn poll(&mut self) -> Option<T> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(v) => {
                self.rx = None;
                Some(v)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                None
            }
        }
    }

    /// Still running.
    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn delivers_the_result_once() {
        let mut t = Task::spawn("test", Arc::new(|| {}), || 41 + 1).unwrap();
        let start = std::time::Instant::now();
        let v = loop {
            if let Some(v) = t.poll() {
                break v;
            }
            assert!(start.elapsed().as_secs() < 10);
            std::thread::yield_now();
        };
        assert_eq!(v, 42);
        assert!(!t.is_running());
        assert_eq!(t.poll(), None);
    }
}
