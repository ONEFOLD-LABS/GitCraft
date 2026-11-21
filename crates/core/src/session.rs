//! [`RepoSession`]: commands, the write queue and events of one open repository.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gitcraft_git::{
    Cancel, ChangedFile, CommandLog, Commit, Comparison, Discarded, FileDiff, GitError, GitRunner,
    HeadPos, Identity, LogReader, Loss, Oid, OnProgress, Progress, PullMode, PushTarget, PushedRef,
    Ref, RefUpdate, Remote, RepoPath, Repository, SearchKind, SearchStream, Stash, Status,
    TrustReport, Written,
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
    pub remotes: Vec<Remote>,
    /// Where a shallow repository's history is cut; empty when it is complete.
    pub shallow: Vec<Oid>,
}

impl Snapshot {
    fn read(repo: &Repository) -> Result<Snapshot, GitError> {
        Ok(Snapshot {
            status: repo.status()?,
            refs: repo.refs()?,
            stashes: repo.stashes()?,
            remotes: repo.remotes()?,
            shallow: repo.shallow_boundary()?,
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
    /// One file between two commits (e.g. merge base and a branch, for a comparison).
    Range {
        from: String,
        to: String,
        file: ChangedFile,
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
    /// Create a branch at a commit or ref, optionally switching to it (`branch` /
    /// `switch -c`).
    CreateBranch {
        name: String,
        start: String,
        checkout: bool,
    },
    /// Switch to a local branch (`switch --no-guess`).
    SwitchBranch(String),
    /// Check out a commit without a branch (`switch --detach`).
    SwitchDetached(Oid),
    RenameBranch {
        old: String,
        new: String,
    },
    /// Delete a local branch after the user saw what it alone holds (`branch -D`).
    DeleteBranch(String),
    /// Commits only this branch holds (`log <branch> --not …`).
    BranchLoss(String),
    /// Commits only the detached HEAD holds.
    DetachedLoss,
    /// Park the changes (`stash push [--message=…] [--include-untracked] [--keep-index]`).
    StashPush {
        message: String,
        untracked: bool,
        keep_index: bool,
    },
    /// Apply a stash, with `pop` dropping it afterwards (`stash apply|pop stash@{n}`).
    StashApply {
        selector: String,
        id: Oid,
        pop: bool,
    },
    /// Drop a stash (`stash drop stash@{n}`).
    StashDrop {
        selector: String,
        id: Oid,
        message: String,
    },
    /// Put a dropped stash back (`stash store`).
    StashStore {
        id: Oid,
        message: String,
    },
    /// Create a tag; with a message annotated, with `sign` signed (`tag [-a|-s -F -]`).
    CreateTag {
        name: String,
        target: String,
        message: Option<String>,
        sign: bool,
    },
    /// Delete a tag (`tag -d`); the outcome is the object it pointed to.
    DeleteTag(String),
    /// Put a deleted tag back (`update-ref refs/tags/<name> <object> ""`).
    RestoreTag {
        name: String,
        object: Oid,
    },
    /// Compare two refs (`merge-base`, `log b..a`, `log a..b`, `diff base b`).
    Compare {
        a: String,
        b: String,
    },
    /// Move the current branch from `expected` to `to` without touching index or working tree
    /// (`reset --soft`, or `update-ref -d` back to no commit); for undoing and redoing commits.
    MoveHead {
        expected: Option<Oid>,
        to: Option<Oid>,
        refuse_pushed: bool,
    },
    /// Remove a branch or tag ref only while it points at `expected` (`update-ref -d`).
    DeleteRefIf {
        full: String,
        expected: Oid,
    },
    /// Add a remote (`remote add`, `remote set-url --push`).
    AddRemote {
        name: String,
        url: String,
        push_url: Option<String>,
    },
    /// Rename a remote with its remote-tracking branches (`remote rename`).
    RenameRemote {
        old: String,
        new: String,
    },
    /// Remove a remote and its remote-tracking branches (`remote remove`); the outcome is
    /// what it was.
    RemoveRemote(String),
    /// Change the fetch URL and the push URL (`remote set-url [--push]`, `config --unset-all
    /// remote.<name>.pushurl`).
    SetRemoteUrls {
        name: String,
        url: String,
        push_url: Option<String>,
    },
    /// Hand passwords in the remote's URLs to the credential helper and take them out of the
    /// URLs (`credential approve`, `remote set-url`).
    MoveCredentials(String),
    /// Fetch one remote or all (`fetch --progress [--prune] (--all | <remote>)`); progress
    /// arrives as [`Event::Progress`].
    Fetch {
        remote: Option<String>,
        prune: bool,
    },
    /// Pull the upstream of the current branch the chosen way (`pull --progress
    /// --no-rebase --ff-only | --no-rebase --ff --no-edit | --rebase`).
    Pull(PullMode),
    /// Where a push of the current branch would go (`remote` for a first push, else the
    /// upstream) and what a forced push there would overwrite (`config --get`, `log`).
    PushInfo(Option<String>),
    /// Push the current branch (`push --progress --porcelain [--set-upstream]
    /// [--force-with-lease=<ref>:<commit>] <remote> <branch>:<ref>`); with `lease` forced, but
    /// only while the remote branch is still at that commit.
    Push {
        remote: Option<String>,
        lease: Option<Option<Oid>>,
    },
    /// Push one tag or all (`push --progress --porcelain [--tags] <remote> [refs/tags/<t>]`).
    PushTags {
        remote: String,
        tag: Option<String>,
    },
    /// Let a local branch track a remote (or local) branch, given as full ref name
    /// (`branch --set-upstream-to`).
    SetUpstream {
        branch: String,
        upstream: String,
    },
    /// Let a branch track nothing (`branch --unset-upstream`).
    UnsetUpstream(String),
    /// Fetch more history of a shallow repository: `n` commits (`fetch --deepen=<n>`), or all
    /// (`fetch --unshallow`).
    Deepen(Option<u32>),
    /// A commit by id, e.g. a stash (`log -1`).
    LoadCommit(Oid),
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
const LOSS: usize = 7;
const LOAD: usize = 8;
const COMPARE: usize = 9;
const PUSH_INFO: usize = 10;
const SLOTS: usize = 11;

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
            | Command::CountLines(_)
            | Command::BranchLoss(_)
            | Command::DetachedLoss
            | Command::LoadCommit(_)
            | Command::PushInfo(_)
            | Command::Compare { .. } => false,
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
            | Command::CreateBranch { .. }
            | Command::SwitchBranch(_)
            | Command::SwitchDetached(_)
            | Command::RenameBranch { .. }
            | Command::DeleteBranch(_)
            | Command::StashPush { .. }
            | Command::StashApply { .. }
            | Command::StashDrop { .. }
            | Command::StashStore { .. }
            | Command::CreateTag { .. }
            | Command::DeleteTag(_)
            | Command::RestoreTag { .. }
            | Command::MoveHead { .. }
            | Command::DeleteRefIf { .. }
            | Command::AddRemote { .. }
            | Command::RenameRemote { .. }
            | Command::RemoveRemote(_)
            | Command::SetRemoteUrls { .. }
            | Command::MoveCredentials(_)
            | Command::Fetch { .. }
            | Command::Pull(_)
            | Command::Push { .. }
            | Command::PushTags { .. }
            | Command::SetUpstream { .. }
            | Command::UnsetUpstream(_)
            | Command::Deepen(_)
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
            Command::BranchLoss(_) | Command::DetachedLoss => Some(LOSS),
            Command::LoadCommit(_) => Some(LOAD),
            Command::Compare { .. } => Some(COMPARE),
            Command::PushInfo(_) => Some(PUSH_INFO),
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
    Loss(Loss),
    /// A branch was deleted; the commit it pointed to.
    Deleted(Oid),
    /// The new stash; `None` when there was nothing to stash.
    Stashed(Option<Oid>),
    /// A commit was made: HEAD before (`None` on an unborn branch) and after.
    Committed {
        output: Written,
        before: Option<Oid>,
        after: Option<Oid>,
    },
    /// HEAD moved to another branch or commit; where it was.
    Switched(HeadPos),
    /// A branch was created at `tip`; `from` is where HEAD was when it was also checked out.
    BranchCreated {
        tip: Oid,
        from: Option<HeadPos>,
    },
    /// A tag was created; the object its ref points to.
    Tagged(Oid),
    Commit(Box<Commit>),
    /// What a fetch changed.
    Fetched(Vec<RefUpdate>),
    /// A pull ended: HEAD before and after.
    Pulled {
        output: Written,
        before: Option<Oid>,
        after: Option<Oid>,
    },
    /// Where a push goes and what forcing it would overwrite.
    PushInfo(Box<PushTarget>, Loss),
    /// What a push did per ref.
    Pushed(Vec<PushedRef>),
    /// A remote was removed; what it was.
    RemoteRemoved(Remote),
    Comparison(Box<Comparison>),
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
    /// A running command reports how far it is (fetch, pull); at most a few per second.
    Progress { id: OperationId, progress: Progress },
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
    /// Sends progress of `id` as events, thinned out: a new phase and the end of a phase
    /// always, otherwise at most every 50 ms.
    fn progress(self: &Arc<Self>, id: OperationId) -> OnProgress {
        let shared = Arc::clone(self);
        let last: Mutex<Option<(String, Instant)>> = Mutex::new(None);
        Arc::new(move |progress: Progress| {
            let Ok(mut last) = last.lock() else {
                return;
            };
            let now = Instant::now();
            let send = match &*last {
                Some((phase, at)) => {
                    *phase != progress.phase
                        || Some(progress.done) == progress.total
                        || now.duration_since(*at) >= Duration::from_millis(50)
                }
                None => true,
            };
            if send {
                *last = Some((progress.phase.clone(), now));
                let _ = shared.events.send(Event::Progress { id, progress });
                (shared.wake)();
            }
        })
    }

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
                        run(&worker_repo, &command, &cancel, worker.progress(id))
                    } else {
                        Err(GitError::NotTrusted)
                    };
                    // A refresh after every write, so the UI shows what git left. Begun before
                    // the write is reported, so the session never looks idle in between.
                    let refresh = Command::Refresh;
                    let rid = worker.begin(&refresh);
                    worker.finish(id, command, result);
                    let result = run(&worker_repo, &refresh, &Cancel::new(), quiet());
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
                let result = run(&repo, &job, &Cancel::new(), quiet());
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

/// For commands that report no progress.
fn quiet() -> OnProgress {
    Arc::new(|_| {})
}

/// Runs one command (on the calling thread).
fn run(
    repo: &Repository,
    command: &Command,
    cancel: &Cancel,
    on_progress: OnProgress,
) -> Result<Outcome, GitError> {
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
            DiffKey::Range { from, to, file } => repo.range_diff(from, to, file)?,
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
        Command::CreateBranch {
            name,
            start,
            checkout,
        } => {
            let from = if *checkout {
                Some(repo.head_pos()?)
            } else {
                None
            };
            repo.create_branch(name, start, *checkout, cancel)?;
            let full = format!("refs/heads/{name}");
            let tip = repo.resolve_commit(&full)?;
            Outcome::BranchCreated { tip, from }
        }
        Command::SwitchBranch(name) => {
            let from = repo.head_pos()?;
            repo.switch_branch(name, cancel)?;
            Outcome::Switched(from)
        }
        Command::SwitchDetached(id) => {
            let from = repo.head_pos()?;
            repo.switch_detached(id, cancel)?;
            Outcome::Switched(from)
        }
        Command::RenameBranch { old, new } => {
            Outcome::Written(repo.rename_branch(old, new, cancel)?)
        }
        Command::DeleteBranch(name) => Outcome::Deleted(repo.delete_branch(name, cancel)?),
        Command::BranchLoss(name) => Outcome::Loss(repo.branch_loss(name)?),
        Command::DetachedLoss => Outcome::Loss(repo.detached_loss()?),
        Command::StashPush {
            message,
            untracked,
            keep_index,
        } => Outcome::Stashed(repo.stash_push(message, *untracked, *keep_index, cancel)?),
        Command::StashApply { selector, id, pop } => {
            Outcome::Written(repo.stash_apply(selector, id, *pop, cancel)?)
        }
        Command::StashDrop { selector, id, .. } => {
            repo.stash_drop(selector, id, cancel)?;
            Outcome::Deleted(id.clone())
        }
        Command::StashStore { id, message } => {
            repo.stash_store(id, message, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::CreateTag {
            name,
            target,
            message,
            sign,
        } => Outcome::Tagged(repo.create_tag(name, target, message.as_deref(), *sign, cancel)?),
        Command::DeleteTag(name) => Outcome::Deleted(repo.delete_tag(name, cancel)?),
        Command::RestoreTag { name, object } => {
            repo.restore_tag(name, object, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::Compare { a, b } => Outcome::Comparison(Box::new(repo.compare(a, b)?)),
        Command::MoveHead {
            expected,
            to,
            refuse_pushed,
        } => {
            repo.move_head(expected.as_ref(), to.as_ref(), *refuse_pushed, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::DeleteRefIf { full, expected } => {
            repo.delete_ref_if(full, expected, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::AddRemote {
            name,
            url,
            push_url,
        } => {
            repo.add_remote(name, url, push_url.as_deref(), cancel)?;
            Outcome::Written(Written::default())
        }
        Command::RenameRemote { old, new } => {
            repo.rename_remote(old, new, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::RemoveRemote(name) => Outcome::RemoteRemoved(repo.remove_remote(name, cancel)?),
        Command::SetRemoteUrls {
            name,
            url,
            push_url,
        } => {
            repo.set_remote_urls(name, url, push_url.as_deref(), cancel)?;
            Outcome::Written(Written::default())
        }
        Command::MoveCredentials(name) => {
            repo.move_credentials(name, cancel)?;
            Outcome::Written(Written::default())
        }
        Command::Fetch { remote, prune } => {
            Outcome::Fetched(repo.fetch(remote.as_deref(), *prune, cancel, on_progress)?)
        }
        Command::Pull(mode) => {
            let before = repo.head_commit()?;
            let output = repo.pull(*mode, cancel, on_progress)?;
            Outcome::Pulled {
                output,
                before,
                after: repo.head_commit()?,
            }
        }
        Command::PushInfo(remote) => {
            let target = repo.push_target(remote.as_deref())?;
            let loss = repo.force_loss(&target)?;
            Outcome::PushInfo(Box::new(target), loss)
        }
        Command::Push { remote, lease } => {
            let target = repo.push_target(remote.as_deref())?;
            Outcome::Pushed(repo.push(&target, lease.clone(), cancel, on_progress)?)
        }
        Command::PushTags { remote, tag } => {
            Outcome::Pushed(repo.push_tags(remote, tag.as_deref(), cancel, on_progress)?)
        }
        Command::SetUpstream { branch, upstream } => {
            Outcome::Written(repo.set_upstream(branch, upstream, cancel)?)
        }
        Command::UnsetUpstream(branch) => Outcome::Written(repo.unset_upstream(branch, cancel)?),
        Command::Deepen(by) => Outcome::Written(repo.deepen(*by, cancel, on_progress)?),
        Command::LoadCommit(id) => Outcome::Commit(Box::new(repo.commit_by_id(id)?)),
        Command::Commit {
            message,
            amend,
            no_verify,
        } => {
            let before = repo.head_commit()?;
            let output = repo.commit(message, *amend, *no_verify, cancel)?;
            Outcome::Committed {
                output,
                before,
                after: repo.head_commit()?,
            }
        }
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
