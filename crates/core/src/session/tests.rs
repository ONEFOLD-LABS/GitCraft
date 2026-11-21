//! The session against real temporary repositories, isolated from the user's git
//! configuration.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use super::*;

struct Fixture {
    tmp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let f = Fixture {
            tmp: tempfile::tempdir().unwrap(),
        };
        std::fs::write(f.config(), "").unwrap();
        std::fs::create_dir(f.repo()).unwrap();
        f.git(&["init", "-q", "-b", "main"]);
        f
    }

    fn repo(&self) -> PathBuf {
        self.tmp.path().join("repo")
    }

    fn config(&self) -> PathBuf {
        self.tmp.path().join("gitconfig")
    }

    fn git(&self, args: &[&str]) {
        let out = Process::new("git")
            .current_dir(self.repo())
            .env("GIT_CONFIG_GLOBAL", self.config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write(&self, name: &str) {
        std::fs::write(self.repo().join(name), name).unwrap();
    }

    fn runner(&self) -> GitRunner {
        GitRunner::new()
            .env("GIT_CONFIG_GLOBAL", self.config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", self.tmp.path())
    }

    fn open(&self) -> RepoSession {
        RepoSession::open(self.runner(), &self.repo(), Arc::new(|| {}))
            .unwrap()
            .0
    }
}

/// Polls until `n` events arrived (at most 20 s).
fn events(session: &RepoSession, n: usize) -> Vec<Event> {
    let start = Instant::now();
    let mut got = Vec::new();
    while got.len() < n {
        got.extend(session.poll());
        assert!(start.elapsed() < Duration::from_secs(20), "only {got:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    got
}

/// Waits until nothing is running and returns what arrived.
fn settle(session: &RepoSession) -> Vec<Event> {
    let start = Instant::now();
    while session.busy() {
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(2));
    }
    session.poll()
}

fn staged(snapshot: &Snapshot) -> Vec<String> {
    snapshot
        .status
        .staged()
        .map(|(f, _)| f.path.display())
        .collect()
}

fn last_snapshot(events: &[Event]) -> Arc<Snapshot> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Finished {
                result: Ok(Outcome::Refreshed(s)),
                ..
            } => Some(Arc::clone(s)),
            _ => None,
        })
        .unwrap()
}

fn paths(names: &[&str]) -> Vec<RepoPath> {
    names
        .iter()
        .map(|n| RepoPath::from_bytes(n.as_bytes()))
        .collect()
}

#[test]
fn opening_reads_the_first_snapshot() {
    let f = Fixture::new();
    f.write("a.txt");
    let (session, snapshot) = RepoSession::open(f.runner(), &f.repo(), Arc::new(|| {})).unwrap();
    assert_eq!(snapshot.status.untracked().count(), 1);
    // Canonical on both sides: on Windows canonicalize adds a `\\?\` prefix, git does not.
    assert_eq!(
        session.workdir().canonicalize().unwrap(),
        f.repo().canonicalize().unwrap()
    );
    assert!(
        !session.command_log().is_empty(),
        "the open calls are logged"
    );
    assert!(matches!(
        RepoSession::open(f.runner(), f.tmp.path(), Arc::new(|| {})),
        Err(GitError::NotARepository(_))
    ));
}

/// Writes run in submission order, each followed by a refresh; the last snapshot shows the
/// state after all of them.
#[test]
fn writes_run_in_order_and_are_followed_by_a_refresh() {
    let f = Fixture::new();
    for n in ["a", "b", "c"] {
        f.write(n);
    }
    let session = f.open();
    let ids = [
        session.submit(Command::Stage(paths(&["a", "b"]))),
        session.submit(Command::Unstage(paths(&["a"]))),
        session.submit(Command::Stage(paths(&["c"]))),
    ];
    let got = settle(&session);
    let writes: Vec<OperationId> = got
        .iter()
        .filter_map(|e| match e {
            Event::Finished {
                id,
                command,
                result,
            } if command.is_write() => {
                assert!(result.is_ok(), "{result:?}");
                Some(*id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(writes, ids);
    assert_eq!(staged(&last_snapshot(&got)), ["b", "c"]);
}

/// A queued write that is cancelled never runs; the queue goes on with the next one.
#[test]
fn a_cancelled_queued_write_does_not_run() {
    let f = Fixture::new();
    f.write("a");
    f.write("b");
    let session = f.open();
    let gate = Arc::new(Barrier::new(2));
    session.submit(Command::Hold(Arc::clone(&gate)));
    let doomed = session.submit(Command::Stage(paths(&["a"])));
    let next = session.submit(Command::Stage(paths(&["b"])));
    session.cancel(doomed);
    gate.wait();
    let got = settle(&session);
    let result = |want: OperationId| {
        got.iter()
            .find_map(|e| match e {
                Event::Finished { id, result, .. } if *id == want => Some(result.clone()),
                _ => None,
            })
            .unwrap()
    };
    assert!(matches!(result(doomed), Err(GitError::Cancelled)));
    assert!(result(next).is_ok());
    assert_eq!(staged(&last_snapshot(&got)), ["b"]);
}

/// Reads do not wait for a write that is still running.
#[test]
fn reads_do_not_wait_for_writes() {
    let f = Fixture::new();
    f.write("a");
    let session = f.open();
    let gate = Arc::new(Barrier::new(2));
    session.submit(Command::Hold(Arc::clone(&gate)));
    let read = session.submit(Command::Refresh);
    let got = events(&session, 1);
    assert!(matches!(&got[0], Event::Finished { id, .. } if *id == read));
    gate.wait();
    settle(&session);
}

/// Of two refreshes submitted together, only the newer one's result arrives.
#[test]
fn superseded_reads_are_dropped() {
    let f = Fixture::new();
    let session = f.open();
    let _older = session.submit(Command::Refresh);
    let newer = session.submit(Command::Refresh);
    let got = settle(&session);
    let ids: Vec<OperationId> = got
        .iter()
        .filter_map(|e| match e {
            Event::Finished { id, .. } => Some(*id),
            Event::Progress { .. } => None,
        })
        .collect();
    assert_eq!(ids, [newer]);
}

#[test]
fn reads_of_different_kinds_do_not_supersede_each_other() {
    let shared = Shared {
        next: AtomicU64::new(0),
        latest: Default::default(),
        in_flight: AtomicUsize::new(0),
        cancels: Mutex::new(HashMap::new()),
        events: channel().0,
        wake: Arc::new(|| {}),
        trusted: AtomicBool::new(true),
    };
    let r = shared.begin(&Command::Refresh);
    let d = shared.begin(&Command::Diff(DiffKey::Untracked {
        path: RepoPath::from_bytes(b"x"),
    }));
    assert_eq!(shared.latest[REFRESH].load(Ordering::SeqCst), r.0);
    assert_eq!(shared.latest[DIFF].load(Ordering::SeqCst), d.0);
    assert_eq!(shared.latest[DETAILS].load(Ordering::SeqCst), 0);
    assert_eq!(shared.in_flight.load(Ordering::SeqCst), 2);
}

#[test]
fn every_call_is_in_the_command_log() {
    let f = Fixture::new();
    f.write("a");
    let session = f.open();
    session.submit(Command::StageAll);
    settle(&session);
    let log = session.command_log().entries();
    let add = log.iter().find(|e| e.write).unwrap();
    assert_eq!(add.posix_line(), "git -c core.fsmonitor=false add --all");
    assert_eq!(add.code, Some(0));
    assert_eq!(Path::new(&add.cwd), session.workdir());
}

/// A repository whose configuration names a program is not trusted until the user says so;
/// until then writes do not run.
#[test]
fn untrusted_repositories_refuse_writes() {
    let f = Fixture::new();
    f.write("a");
    f.git(&["config", "core.hooksPath", "hooks"]);
    let session = f.open();
    assert!(!session.is_trusted());
    assert_eq!(session.trust_report().settings[0].key, "core.hookspath");
    let refused = session.submit(Command::StageAll);
    let got = settle(&session);
    assert!(got.iter().any(|e| matches!(
        e,
        Event::Finished { id, result: Err(GitError::NotTrusted), .. } if *id == refused
    )));
    assert!(staged(&last_snapshot(&got)).is_empty());
    assert!(
        !session.command_log().entries().iter().any(|e| e.write),
        "git never ran"
    );

    session.set_trusted(true);
    session.submit(Command::StageAll);
    assert_eq!(staged(&last_snapshot(&settle(&session))), ["a"]);
}

#[test]
fn plain_repositories_are_trusted() {
    let f = Fixture::new();
    assert!(f.open().is_trusted());
}

#[test]
fn commit_through_the_queue() {
    let f = Fixture::new();
    f.write("a");
    let session = f.open();
    session.submit(Command::SetIdentity {
        name: "Test".into(),
        email: "t@invalid".into(),
        global: false,
    });
    session.submit(Command::StageAll);
    let commit = session.submit(Command::Commit {
        message: "Erster".into(),
        amend: false,
        no_verify: false,
    });
    let got = settle(&session);
    assert!(got.iter().any(|e| matches!(
        e,
        Event::Finished {
            id,
            result: Ok(Outcome::Committed { before: None, after: Some(_), .. }),
            ..
        } if *id == commit
    )));
    assert!(last_snapshot(&got).status.is_clean());
    session.submit(Command::AmendInfo);
    session.submit(Command::Identity);
    let got = settle(&session);
    assert!(got.iter().any(|e| matches!(
        e,
        Event::Finished { result: Ok(Outcome::AmendInfo(Some((m, false)))), .. } if m == "Erster"
    )));
    assert!(got.iter().any(|e| matches!(
        e,
        Event::Finished { result: Ok(Outcome::Identity(i)), .. } if i.is_complete()
    )));
}

/// A fetch reports progress while it runs and what changed at the end.
#[test]
fn fetch_reports_progress_and_changes() {
    let f = Fixture::new();
    // The remote: another repository with a few commits.
    let up = f.tmp.path().join("up");
    std::fs::create_dir(&up).unwrap();
    let git_up = |args: &[&str]| {
        let out = Process::new("git")
            .current_dir(&up)
            .env("GIT_CONFIG_GLOBAL", f.config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@invalid")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@invalid")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
    };
    git_up(&["init", "-q", "-b", "main"]);
    for i in 0..20 {
        std::fs::write(up.join(format!("f{i}")), format!("{i}").repeat(1000)).unwrap();
        git_up(&["add", "-A"]);
        git_up(&["commit", "-q", "-m", &format!("c{i}")]);
    }
    f.git(&["remote", "add", "origin", up.to_str().unwrap()]);
    let session = f.open();
    session.submit(Command::Fetch {
        remote: Some("origin".into()),
        prune: false,
    });
    let got = settle(&session);
    assert!(
        got.iter().any(|e| matches!(e, Event::Progress { .. })),
        "{got:?}"
    );
    let fetched = got.iter().find_map(|e| match e {
        Event::Finished {
            result: Ok(Outcome::Fetched(u)),
            ..
        } => Some(u.clone()),
        _ => None,
    });
    let updates = fetched.expect("fetch finished");
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].name, "origin/main");
}
