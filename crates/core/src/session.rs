//! [`RepoSession`]: commands, the write queue and events of one open repository.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use gitcraft_git::{
    Cancel, ChangedFile, CommandLog, Commit, Discarded, FileDiff, GitError, GitRunner, Identity,
    LogReader, Ref, RepoPath, Repository, SearchKind, SearchStream, Stash, Status, TrustReport,
    Written,
};

/// Called from worker threads when an event is waiting (the UI asks for a repaint).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Identifies one submitted command; newer commands have larger ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(u64);

/// What the window shows of a repository besides the history: read together so that status,
/// refs and stashes always belong to the same moment.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub status: Status,
    pub refs: Vec<Ref>,
    pub stashes: Vec<Stash>,
}

impl Snapshot {
    fn read(repo: &Repository) -> Result<Snapshot, GitError> {
        Ok(Snapshot {
            status: repo.status()?,
            refs: repo.refs()?,
            stashes: repo.stashes()?,
        })
    }
}

/// Which diff to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKey {
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

/// Everything the UI can ask of a repository. Each variant names the git commands it runs.
#[derive(Debug, Clone)]
pub enum Command {
    /// Read status, refs and stashes (`status`, `for-each-ref`, `stash list`).
    Refresh,
    /// Message and changed files of a commit (`show -s`, `diff-tree`).
    CommitDetails(Box<Commit>),
    /// The diff of one file (`diff-tree -p`, `diff`, or the untracked file itself).
    Diff(DiffKey),
    /// Stage paths (`add --all --pathspec-from-file=-`).
    Stage(Vec<RepoPath>),
    /// Stage every change (`add --all`).
    StageAll,
    /// Unstage paths (`restore --staged --pathspec-from-file=-`; `rm --cached` before the
    /// first commit).
    Unstage(Vec<RepoPath>),
    /// Unstage everything (`reset`; `rm --cached -r .` before the first commit).
    UnstageAll,
    /// Stage (or with `staged` unstage) some lines of a working-tree diff: (hunk, line)
    /// positions in `shown`, the diff the user saw (`apply --cached --recount [--reverse] -`,
    /// refused with [`GitError::Stale`] when the file changed since).
    StageLines {
        path: RepoPath,
        old: Option<RepoPath>,
        staged: bool,
        shown: Box<FileDiff>,
        lines: Vec<(usize, usize)>,
    },
    /// Discard unstaged changes of tracked files, after saving them (`hash-object`,
    /// `commit-tree`, `update-ref refs/gitcraft/discarded/…`, `restore --worktree`).
    Discard(Vec<RepoPath>),
    /// Discard some lines of a file's unstaged diff, after saving the file (`apply --reverse`).
    DiscardLines {
        path: RepoPath,
        shown: Box<FileDiff>,
        lines: Vec<(usize, usize)>,
    },
    /// Delete untracked files, after saving them.
    DeleteUntracked(Vec<RepoPath>),
    /// The saved states (`for-each-ref refs/gitcraft/discarded`).
    ListDiscarded,
    /// Write a saved state back (the current state is saved first).
    RestoreDiscarded(String),
    /// Remove saved states older than this many seconds.
    PruneDiscarded(i64),
    /// Added plus removed unstaged lines of files (`diff --numstat`), for a confirmation.
    CountLines(Vec<RepoPath>),
    /// Commit the staged changes (`commit --file=- --cleanup=whitespace [--amend]
    /// [--no-verify]`, message on stdin).
    Commit {
        message: String,
        amend: bool,
        no_verify: bool,
    },
    /// Read `user.name` and `user.email` (`config --get`).
    Identity,
    /// Set `user.name` and `user.email` (`config --global|--local`).
    SetIdentity {
        name: String,
        email: String,
        global: bool,
    },
    /// Message of `HEAD` and whether it is already on a remote (`log -1`, `for-each-ref
    /// --contains`), for amending.
    AmendInfo,
    /// Blocks the write queue until the barrier is passed (tests of ordering and cancellation).
    #[cfg(test)]
    Hold(Arc<std::sync::Barrier>),
}

/// Kinds of reads whose older results are dropped when a newer one was submitted.
const REFRESH: usize = 0;
const DETAILS: usize = 1;
const DIFF: usize = 2;
const IDENTITY: usize = 3;
const AMEND: usize = 4;
const DISCARDED: usize = 5;
const COUNT: usize = 6;
const SLOTS: usize = 7;

impl Command {
    /// Changes the repository (runs in the write queue).
    pub fn is_write(&self) -> bool {
        match self {
            Command::Refresh
            | Command::CommitDetails(_)
            | Command::Diff(_)
            | Command::Identity
            | Command::AmendInfo
            | Command::ListDiscarded
            | Command::CountLines(_) => false,
            Command::Stage(_)
            | Command::StageAll
            | Command::Unstage(_)
            | Command::UnstageAll
            | Command::StageLines { .. }
            | Command::Discard(_)
            | Command::DiscardLines { .. }
            | Command::DeleteUntracked(_)
            | Command::RestoreDiscarded(_)
            | Command::PruneDiscarded(_)
            | Command::Commit { .. }
            | Command::SetIdentity { .. } => true,
            #[cfg(test)]
            Command::Hold(_) => true,
        }
    }

    fn slot(&self) -> Option<usize> {
        match self {
            Command::Refresh => Some(REFRESH),
            Command::CommitDetails(_) => Some(DETAILS),
            Command::Diff(_) => Some(DIFF),
            Command::Identity => Some(IDENTITY),
            Command::AmendInfo => Some(AMEND),
            Command::ListDiscarded => Some(DISCARDED),
            Command::CountLines(_) => Some(COUNT),
            _ => None,
        }
    }
}

/// What a command produced.
#[derive(Debug, Clone)]
pub enum Outcome {
    Refreshed(Arc<Snapshot>),
    CommitDetails {
        message: String,
        files: Vec<ChangedFile>,
    },
    Diff(FileDiff),
    Identity(Identity),
    /// The message of `HEAD` and whether a remote has it; `None` before the first commit.
    AmendInfo(Option<(String, bool)>),
    /// Contents were saved before a destructive command; the ref that keeps them.
    Saved(String),
    Discarded(Vec<Discarded>),
    Lines(u64),
    /// A write finished; git's output (hooks, notes).
    Written(Written),
}

/// News from the session, collected with [`RepoSession::poll`].
#[derive(Debug, Clone)]
pub enum Event {
    /// A command ended. Results of superseded reads never arrive.
    Finished {
        id: OperationId,
        command: Command,
        result: Result<Outcome, GitError>,
    },
}

/// State shared with the worker threads.
struct Shared {
    next: AtomicU64,
    /// Newest submitted id per read kind.
    latest: [AtomicU64; SLOTS],
    in_flight: AtomicUsize,
    cancels: Mutex<HashMap<OperationId, Cancel>>,
    events: Sender<Event>,
    wake: Wake,
    /// Writes may run (docs/security.md T1). Checked when a write is about to start.
    trusted: AtomicBool,
}

impl Shared {
    fn begin(&self, command: &Command) -> OperationId {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(slot) = command.slot() {
            // Ids are taken on several threads: keep the largest.
            self.latest[slot].fetch_max(id, Ordering::SeqCst);
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        OperationId(id)
    }

    fn finish(&self, id: OperationId, command: Command, result: Result<Outcome, GitError>) {
        if let Ok(mut c) = self.cancels.lock() {
            c.remove(&id);
        }
        let current = command
            .slot()
            .is_none_or(|slot| self.latest[slot].load(Ordering::SeqCst) == id.0);
        if current {
            // The session may be gone; then nobody needs the result.
            let _ = self.events.send(Event::Finished {
                id,
                command,
                result,
            });
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        (self.wake)();
    }
}

/// One open repository: its commands, write queue, events and command log.
pub struct RepoSession {
    repo: Repository,
    trust: TrustReport,
    log: Arc<CommandLog>,
    shared: Arc<Shared>,
    events: Receiver<Event>,
    writes: Sender<(OperationId, Command, Cancel)>,
}

impl RepoSession {
    /// Checks the installed git, opens the working tree containing `path` and reads its first
    /// snapshot. Blocks: call it off the UI thread (e.g. in a [`crate::Task`]).
    pub fn open(
        runner: GitRunner,
        path: &Path,
        wake: Wake,
    ) -> Result<(RepoSession, Arc<Snapshot>), GitError> {
        let log = Arc::new(CommandLog::new());
        let runner = runner.with_log(Arc::clone(&log));
        runner.version()?;
        let repo = Repository::open(&runner, path)?;
        let trust = repo.trust_report()?;
        let snapshot = Arc::new(Snapshot::read(&repo)?);
        Ok((RepoSession::start(repo, trust, log, wake)?, snapshot))
    }

    fn start(
        repo: Repository,
        trust: TrustReport,
        log: Arc<CommandLog>,
        wake: Wake,
    ) -> Result<RepoSession, GitError> {
        let (events_tx, events) = channel();
        let shared = Arc::new(Shared {
            next: AtomicU64::new(0),
            latest: Default::default(),
            in_flight: AtomicUsize::new(0),
            cancels: Mutex::new(HashMap::new()),
            events: events_tx,
            wake,
            // Nothing that could run: no reason to ask.
            trusted: AtomicBool::new(trust.is_empty()),
        });
        let (writes, queue) = channel::<(OperationId, Command, Cancel)>();
        let worker_repo = repo.clone();
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("gitcraft-writes".into())
            .spawn(move || {
                // Ends when the session is dropped (the sender goes away).
                while let Ok((id, command, cancel)) = queue.recv() {
                    let result = if worker.trusted.load(Ordering::SeqCst) {
                        run(&worker_repo, &command, &cancel)
                    } else {
                        Err(GitError::NotTrusted)
                    };
                    // A refresh after every write, so the UI shows what git left. Begun before
                    // the write is reported, so the session never looks idle in between.
                    let refresh = Command::Refresh;
                    let rid = worker.begin(&refresh);
                    worker.finish(id, command, result);
                    let result = run(&worker_repo, &refresh, &Cancel::new());
                    worker.finish(rid, refresh, result);
                }
            })
            .map_err(|e| GitError::Spawn(e.to_string()))?;
        Ok(RepoSession {
            repo,
            trust,
            log,
            shared,
            events,
            writes,
        })
    }

    /// Top level of the working tree.
    pub fn workdir(&self) -> &Path {
        self.repo.workdir()
    }

    /// The repository's git directory.
    pub fn git_dir(&self) -> &Path {
        self.repo.git_dir()
    }

    /// What writes in this repository could run besides git (read when it was opened).
    pub fn trust_report(&self) -> &TrustReport {
        &self.trust
    }

    /// Writes may run. True from the start when the repository names no programs and has no
    /// hooks.
    pub fn is_trusted(&self) -> bool {
        self.shared.trusted.load(Ordering::SeqCst)
    }

    /// The user trusts the repository (or withdraws trust). Untrusted, every write ends with
    /// [`GitError::NotTrusted`] without running git.
    pub fn set_trusted(&self, trusted: bool) {
        self.shared.trusted.store(trusted, Ordering::SeqCst);
    }

    /// Queues `command`; its outcome arrives as an [`Event`]. Writes run one after another in
    /// submission order; reads start at once.
    pub fn submit(&self, command: Command) -> OperationId {
        let id = self.shared.begin(&command);
        if command.is_write() {
            let cancel = Cancel::new();
            if let Ok(mut c) = self.shared.cancels.lock() {
                c.insert(id, cancel.clone());
            }
            if let Err(e) = self.writes.send((id, command, cancel)) {
                let (id, command, _) = e.0;
                let stopped = GitError::Spawn("the write queue has stopped".into());
                self.shared.finish(id, command, Err(stopped));
            }
            return id;
        }
        let repo = self.repo.clone();
        let shared = Arc::clone(&self.shared);
        let job = command.clone();
        let started = std::thread::Builder::new()
            .name("gitcraft-read".into())
            .spawn(move || {
                let result = run(&repo, &job, &Cancel::new());
                shared.finish(id, job, result);
            });
        if let Err(e) = started {
            self.shared
                .finish(id, command, Err(GitError::Spawn(e.to_string())));
        }
        id
    }

    /// Stops a write: a running git is ended with everything it started, a queued one does not
    /// start. Its event reports [`GitError::Cancelled`]. Reads are not cancelled (their result
    /// is dropped when a newer read replaces them).
    pub fn cancel(&self, id: OperationId) {
        if let Ok(c) = self.shared.cancels.lock()
            && let Some(cancel) = c.get(&id)
        {
            cancel.cancel();
        }
    }

    /// Events since the last call.
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Commands submitted and not yet finished.
    pub fn busy(&self) -> bool {
        self.shared.in_flight.load(Ordering::SeqCst) > 0
    }

    /// Every git call of this session (credentials redacted).
    pub fn command_log(&self) -> Arc<CommandLog> {
        Arc::clone(&self.log)
    }

    /// Streaming reads the UI consumes at its own pace (history pages, search).
    pub fn reader(&self) -> Reader {
        Reader(self.repo.clone())
    }
}

/// Read-only access for streams that the UI pulls page by page on its own threads.
#[derive(Debug, Clone)]
pub struct Reader(Repository);

impl Reader {
    /// The history, newest first, in pages.
    pub fn log(&self) -> Result<LogReader, GitError> {
        self.0.log()
    }

    /// Commits matching a search, as they are found.
    pub fn search(&self, kind: SearchKind, text: &str) -> Result<SearchStream, GitError> {
        self.0.search(kind, text)
    }
}

/// Runs one command (on the calling thread).
fn run(repo: &Repository, command: &Command, cancel: &Cancel) -> Result<Outcome, GitError> {
    Ok(match command {
        Command::Refresh => Outcome::Refreshed(Arc::new(Snapshot::read(repo)?)),
        Command::CommitDetails(commit) => Outcome::CommitDetails {
            message: repo.commit_message(commit)?,
            files: repo.commit_files(commit)?,
        },
        Command::Diff(key) => Outcome::Diff(match key {
            DiffKey::Commit { commit, file } => repo.commit_diff(commit, file)?,
            DiffKey::Worktree { path, old, staged } => {
                repo.worktree_diff(path, old.as_ref(), *staged)?
            }
            DiffKey::Untracked { path } => repo.untracked_diff(path)?,
        }),
        Command::Stage(paths) => Outcome::Written(repo.stage(paths, cancel)?),
        Command::StageAll => Outcome::Written(repo.stage_all(cancel)?),
        Command::Unstage(paths) => Outcome::Written(repo.unstage(paths, cancel)?),
        Command::UnstageAll => Outcome::Written(repo.unstage_all(cancel)?),
        Command::StageLines {
            path,
            old,
            staged,
            shown,
            lines,
        } => {
            Outcome::Written(repo.stage_lines(path, old.as_ref(), *staged, shown, lines, cancel)?)
        }
        Command::Discard(paths) => Outcome::Saved(repo.discard(paths, cancel)?),
        Command::DiscardLines { path, shown, lines } => {
            Outcome::Saved(repo.discard_lines(path, shown, lines, cancel)?)
        }
        Command::DeleteUntracked(paths) => Outcome::Saved(repo.delete_untracked(paths, cancel)?),
        Command::ListDiscarded => Outcome::Discarded(repo.discarded()?),
        Command::RestoreDiscarded(id) => Outcome::Saved(repo.restore_discarded(id, cancel)?),
        Command::PruneDiscarded(keep) => {
            repo.prune_discarded(*keep, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::CountLines(paths) => Outcome::Lines(repo.unstaged_lines(paths)?),
        Command::Commit {
            message,
            amend,
            no_verify,
        } => Outcome::Written(repo.commit(message, *amend, *no_verify, cancel)?),
        Command::Identity => Outcome::Identity(repo.identity()?),
        Command::SetIdentity {
            name,
            email,
            global,
        } => {
            repo.set_identity(name, email, *global, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::AmendInfo => Outcome::AmendInfo(repo.head_for_amend()?),
        #[cfg(test)]
        Command::Hold(barrier) => {
            barrier.wait();
            Outcome::Written(Written::default())
        }
    })
}

#[cfg(test)]
mod tests;
