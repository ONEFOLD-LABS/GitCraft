use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::GitError;
use crate::command_log::{CommandLog, LogEntry};
use crate::progress::{OnProgress, Progress};

/// Oldest git GitCraft supports (porcelain v2 status with all fields used here, `init -b`).
pub const MIN_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 30,
    patch: 0,
};

/// Largest stdout GitCraft reads from one git call (larger results come in streams or pages).
const MAX_STDOUT: usize = 64 * 1024 * 1024;
/// Largest stderr kept for an error message.
const MAX_STDERR: usize = 16 * 1024;

/// How long one read may take before GitCraft stops it (a hung filesystem, a stuck process).
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Environment variables that would point git at another repository, index, object store or
/// history, inject configuration, or write traces; never inherited.
const REDIRECTING_ENV: &[&str] = &[
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_ATTR_SOURCE",
    "GIT_REPLACE_REF_BASE",
    "GIT_GRAFT_FILE",
    "GIT_SHALLOW_FILE",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_EXTERNAL_DIFF",
];

/// A git version as `git version` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// Reads `git version 2.43.0`, `git version 2.39.2.windows.1`,
    /// `git version 2.39.3 (Apple Git-145)`.
    pub fn parse(text: &str) -> Option<GitVersion> {
        let rest = text.trim().strip_prefix("git version ")?;
        let number = rest.split_whitespace().next()?;
        let mut parts = number.split('.').map(|p| {
            let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        });
        let major = parts.next()??;
        let minor = parts.next()??;
        let patch = parts.next().flatten().unwrap_or(0);
        Some(GitVersion {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Stops a running git call from another thread (the user pressed Cancel). Cloning shares the
/// flag.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a write printed. Hooks and git's own notes go to stderr; the UI shows both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Written {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Read,
    Write,
}

/// Starts git. Every call gets an argument list (never a shell), runs without a terminal prompt
/// or editor, in the C locale, without a pager, in its own process group (so that cancelling
/// also ends hooks and helpers it started), and with `core.fsmonitor` switched off so that a
/// repository's own configuration cannot start a program while GitCraft only reads.
#[derive(Debug, Clone)]
pub struct GitRunner {
    program: PathBuf,
    env: Vec<(OsString, OsString)>,
    log: Option<Arc<CommandLog>>,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl GitRunner {
    /// Uses `git` from `PATH`.
    pub fn new() -> Self {
        Self::with_program("git")
    }

    /// Uses a specific git executable.
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        GitRunner {
            program: program.into(),
            env: Vec::new(),
            log: None,
        }
    }

    /// Sets an environment variable for every call (e.g. `GIT_CONFIG_GLOBAL` in tests).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Records every call in `log` (the command log the UI shows).
    pub fn with_log(mut self, log: Arc<CommandLog>) -> Self {
        self.log = Some(log);
        self
    }

    /// The installed git version; fails with [`GitError::TooOld`] below [`MIN_VERSION`].
    pub fn version(&self) -> Result<GitVersion, GitError> {
        // Outside any repository, so that no repository configuration is read.
        let out = self.read(&std::env::temp_dir(), ["version"])?;
        let text = String::from_utf8_lossy(&out);
        let version = GitVersion::parse(&text)
            .ok_or_else(|| GitError::Parse(format!("git version: {}", text.trim())))?;
        if version < MIN_VERSION {
            return Err(GitError::TooOld(version));
        }
        Ok(version)
    }

    /// Runs a git command that only reads (no optional locks, so it does not refresh the index
    /// behind the user's back) in `cwd` and returns its stdout.
    pub fn read<I, S>(&self, cwd: &Path, args: I) -> Result<Vec<u8>, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.read_with(cwd, &[], args)
    }

    /// Like [`GitRunner::read`], with configuration overrides (`key=value`, passed as `-c`)
    /// placed before the command; [`crate::Repository`] uses them to neutralise programs its
    /// configuration names.
    pub fn read_with<I, S>(
        &self,
        cwd: &Path,
        config: &[String],
        args: I,
    ) -> Result<Vec<u8>, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let (child, call) = self.spawn(cwd, config, args, Mode::Read, false, &[])?;
        collect(child, &call, None, Some(READ_TIMEOUT), None, None).map(|(out, _)| out)
    }

    /// Runs a git command that changes the repository. `input` is written to git's stdin (path
    /// lists, messages, patches: never temporary files). Writes take normal locks, run hooks
    /// where Git runs them and have no time limit; `cancel` stops git and everything it started.
    pub fn write<I, S>(
        &self,
        cwd: &Path,
        config: &[String],
        args: I,
        input: Option<&[u8]>,
        cancel: &Cancel,
    ) -> Result<Written, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.write_env(cwd, config, args, input, cancel, &[])
    }

    /// Like [`GitRunner::write`] with extra environment variables for this call (e.g. a
    /// temporary `GIT_INDEX_FILE`).
    pub(crate) fn write_env<I, S>(
        &self,
        cwd: &Path,
        config: &[String],
        args: I,
        input: Option<&[u8]>,
        cancel: &Cancel,
        env: &[(&str, &OsStr)],
    ) -> Result<Written, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if cancel.is_cancelled() {
            return Err(GitError::Cancelled);
        }
        let (child, call) = self.spawn(cwd, config, args, Mode::Write, input.is_some(), env)?;
        let (stdout, stderr) = collect(
            child,
            &call,
            input.map(<[u8]>::to_vec),
            None,
            Some(cancel.clone()),
            None,
        )?;
        Ok(Written {
            stdout,
            stderr: String::from_utf8_lossy(&stderr).trim_end().to_owned(),
        })
    }

    /// Like [`GitRunner::write`] for commands that report progress on stderr (`--progress`):
    /// each report goes to `on_progress` as it arrives; the other lines are kept as git's
    /// message.
    pub(crate) fn write_progress<I, S>(
        &self,
        cwd: &Path,
        config: &[String],
        args: I,
        cancel: &Cancel,
        on_progress: OnProgress,
        env: &[(&str, &OsStr)],
    ) -> Result<Written, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if cancel.is_cancelled() {
            return Err(GitError::Cancelled);
        }
        let (child, call) = self.spawn(cwd, config, args, Mode::Write, false, env)?;
        let (stdout, stderr) = collect(
            child,
            &call,
            None,
            None,
            Some(cancel.clone()),
            Some(on_progress),
        )?;
        Ok(Written {
            stdout,
            stderr: String::from_utf8_lossy(&stderr).trim_end().to_owned(),
        })
    }

    /// Like [`GitRunner::write_progress`], but a failed exit is not an error: returns the
    /// output and whether git succeeded (`push --porcelain` explains rejections on stdout).
    pub(crate) fn write_progress_any<I, S>(
        &self,
        cwd: &Path,
        args: I,
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<(Written, bool), GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if cancel.is_cancelled() {
            return Err(GitError::Cancelled);
        }
        let (child, call) = self.spawn(cwd, &[], args, Mode::Write, false, &[])?;
        let out = collect_any(
            child,
            &call,
            None,
            None,
            Some(cancel.clone()),
            Some(on_progress),
        )?;
        let written = Written {
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).trim_end().to_owned(),
        };
        Ok((written, out.success))
    }

    /// Starts a read whose output is consumed record by record ([`GitStream`]), e.g. a long
    /// history. No timeout: the consumer sets the pace; dropping the stream stops git.
    pub fn stream<I, S>(
        &self,
        cwd: &Path,
        config: &[String],
        args: I,
    ) -> Result<GitStream, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let (mut child, call) = self.spawn(cwd, config, args, Mode::Read, false, &[])?;
        let stdout = child.stdout.take().map(std::io::BufReader::new);
        let stderr = child
            .stderr
            .take()
            .map(|err| std::thread::spawn(move || drain(err)));
        Ok(GitStream {
            child,
            reaped: false,
            stdout,
            stderr,
            call,
        })
    }

    /// Starts git with GitCraft's fixed setup.
    fn spawn<I, S>(
        &self,
        cwd: &Path,
        config: &[String],
        args: I,
        mode: Mode,
        stdin: bool,
        extra_env: &[(&str, &OsStr)],
    ) -> Result<(Child, Call), GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let command = args
            .iter()
            .map(|a| a.to_string_lossy())
            .find(|a| !a.starts_with('-'))
            .map(|a| a.into_owned())
            .unwrap_or_default();
        let mut cmd = Command::new(&self.program);
        let mut fixed = vec!["-c".to_owned(), "core.fsmonitor=false".to_owned()];
        for c in config {
            fixed.push("-c".to_owned());
            fixed.push(c.clone());
        }
        cmd.current_dir(cwd)
            .args(&fixed)
            .args(&args)
            .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            // ":" is git's own "no editor": a command never waits for one.
            .env("GIT_EDITOR", ":")
            .env("GIT_SEQUENCE_EDITOR", ":")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C");
        if mode == Mode::Read {
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
        }
        for key in REDIRECTING_ENV {
            cmd.env_remove(key);
        }
        for (key, _) in std::env::vars_os() {
            let k = key.to_string_lossy();
            if k.starts_with("GIT_CONFIG_KEY_")
                || k.starts_with("GIT_CONFIG_VALUE_")
                || k.starts_with("GIT_TRACE")
            {
                cmd.env_remove(&key);
            }
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes up for each call.
            cmd.creation_flags(0x0800_0000);
        }
        let child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => GitError::NotInstalled,
            _ => GitError::Spawn(e.to_string()),
        })?;
        let shown = fixed
            .into_iter()
            .chain(args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect();
        Ok((
            child,
            Call {
                command,
                shown,
                cwd: cwd.to_path_buf(),
                write: mode == Mode::Write,
                started: Instant::now(),
                time: SystemTime::now(),
                log: self.log.clone(),
            },
        ))
    }
}

/// One started git call, for errors and the command log.
struct Call {
    /// The subcommand, e.g. `status`.
    command: String,
    /// All arguments after `git`.
    shown: Vec<String>,
    cwd: PathBuf,
    write: bool,
    started: Instant,
    time: SystemTime,
    log: Option<Arc<CommandLog>>,
}

impl Call {
    fn record(&self, code: Option<i32>, stderr: &[u8]) {
        if let Some(log) = &self.log {
            log.record(LogEntry::new(
                self.time,
                self.cwd.clone(),
                &self.shown,
                self.started.elapsed(),
                code,
                stderr,
                self.write,
            ));
        }
    }
}

/// Why the watchdog ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Ended,
    TimedOut,
    Cancelled,
}

/// A child process and whether it has been waited for (then its id may belong to another
/// process and must not be signalled).
struct Watched {
    child: Child,
    reaped: bool,
}

impl Watched {
    fn kill(&mut self) {
        if !self.reaped {
            kill(&mut self.child);
        }
    }
}

/// Stops git and, on Unix, its whole process group (hooks, ssh, helpers). Only for a child that
/// has not been waited for.
fn kill(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            // The group's id is git's pid (`process_group(0)` at spawn).
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
    let _ = child.kill();
}

/// Feeds `input` to stdin (on its own thread), reads stdout (capped) and stderr (capped, on its
/// own thread so that no pipe can block git), waits for git, records the call and maps
/// failures. A watchdog stops git after `timeout` or when `cancel` is set.
fn collect(
    child: Child,
    call: &Call,
    input: Option<Vec<u8>>,
    timeout: Option<Duration>,
    cancel: Option<Cancel>,
    on_progress: Option<OnProgress>,
) -> Result<(Vec<u8>, Vec<u8>), GitError> {
    let out = collect_any(child, call, input, timeout, cancel, on_progress)?;
    if out.success {
        return Ok((out.stdout, out.stderr));
    }
    Err(failure(out.code, &out.stderr, &call.command, &call.cwd))
}

/// Like [`collect`], but an unsuccessful exit is returned as output (for commands whose
/// stdout explains the failure, like `push --porcelain`).
fn collect_any(
    mut child: Child,
    call: &Call,
    input: Option<Vec<u8>>,
    timeout: Option<Duration>,
    cancel: Option<Cancel>,
    on_progress: Option<OnProgress>,
) -> Result<Collected, GitError> {
    let feeder = match (child.stdin.take(), input) {
        (Some(mut pipe), Some(bytes)) => Some(std::thread::spawn(move || {
            // git may exit without reading everything (an error); the broken pipe is fine.
            let _ = pipe.write_all(&bytes);
        })),
        _ => None,
    };
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let child = Arc::new(Mutex::new(Watched {
        child,
        reaped: false,
    }));
    let (done, wait) = mpsc::channel::<()>();
    let watched = Arc::clone(&child);
    let watchdog = std::thread::spawn(move || {
        let start = Instant::now();
        loop {
            if !matches!(
                wait.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                return Stop::Ended;
            }
            let stop = if cancel.as_ref().is_some_and(Cancel::is_cancelled) {
                Stop::Cancelled
            } else if timeout.is_some_and(|t| start.elapsed() >= t) {
                Stop::TimedOut
            } else {
                continue;
            };
            // The main thread then sees the pipes close.
            if let Ok(mut c) = watched.lock() {
                c.kill();
            }
            return stop;
        }
    });
    let result = collect_pipes(&child, stdout_pipe, stderr_pipe, on_progress);
    let _ = done.send(());
    let stop = watchdog.join().unwrap_or(Stop::Ended);
    if let Some(f) = feeder {
        let _ = f.join();
    }
    let out = result?;
    let code = if stop == Stop::Ended { out.code } else { None };
    call.record(code, &out.stderr);
    match stop {
        Stop::TimedOut => {
            return Err(GitError::TimedOut {
                command: call.command.clone(),
                seconds: timeout.map(|t| t.as_secs()).unwrap_or_default(),
            });
        }
        Stop::Cancelled => return Err(GitError::Cancelled),
        Stop::Ended => {}
    }
    if out.too_large {
        return Err(GitError::OutputTooLarge {
            command: call.command.clone(),
            limit: MAX_STDOUT,
        });
    }
    Ok(out)
}

struct Collected {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: Option<i32>,
    success: bool,
    too_large: bool,
}

fn collect_pipes(
    child: &Mutex<Watched>,
    stdout_pipe: Option<std::process::ChildStdout>,
    stderr_pipe: Option<std::process::ChildStderr>,
    on_progress: Option<OnProgress>,
) -> Result<Collected, GitError> {
    let kill = || {
        if let Ok(mut c) = child.lock() {
            c.kill();
            let _ = c.child.wait();
            c.reaped = true;
        }
    };
    let stderr_reader = stderr_pipe.map(|err| {
        std::thread::spawn(move || match on_progress {
            Some(report) => drain_progress(err, &report),
            None => drain(err),
        })
    });
    let mut stdout = Vec::new();
    let mut too_large = false;
    if let Some(out) = stdout_pipe {
        let limit = u64::try_from(MAX_STDOUT)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if let Err(e) = out.take(limit).read_to_end(&mut stdout) {
            kill();
            return Err(GitError::Spawn(e.to_string()));
        }
        too_large = stdout.len() > MAX_STDOUT;
    }
    if too_large {
        kill();
    }
    // Poll instead of `wait()`, so the watchdog can take the lock and kill a git that closed its
    // output but does not end.
    let status = loop {
        let mut c = child
            .lock()
            .map_err(|_| GitError::Spawn("git process lock poisoned".into()))?;
        let polled = c
            .child
            .try_wait()
            .map_err(|e| GitError::Spawn(e.to_string()))?;
        if let Some(s) = polled {
            c.reaped = true;
            break s;
        }
        drop(c);
        std::thread::sleep(Duration::from_millis(5));
    };
    let stderr = stderr_reader
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    Ok(Collected {
        stdout,
        stderr,
        code: status.code(),
        success: status.success(),
        too_large,
    })
}

/// Reads a pipe to its end, keeping at most [`MAX_STDERR`] bytes (reading on past the cap, so
/// git never blocks on a full pipe).
fn drain(mut pipe: impl Read) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = pipe.read(&mut buf) {
        if n == 0 {
            break;
        }
        let room = MAX_STDERR.saturating_sub(kept.len());
        kept.extend_from_slice(&buf[..n.min(room)]);
    }
    kept
}

/// Reads stderr line by line (lines end with `\r` or `\n`), hands progress reports to
/// `report` and keeps the other lines (at most [`MAX_STDERR`] bytes), so that a long progress
/// display never pushes git's actual message out.
fn drain_progress(mut pipe: impl Read, report: &OnProgress) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut line = Vec::new();
    let mut buf = [0u8; 4096];
    let take = |line: &mut Vec<u8>, kept: &mut Vec<u8>| {
        let text = String::from_utf8_lossy(line);
        match Progress::parse(&text) {
            Some(p) => report(p),
            None if !text.trim().is_empty() => {
                let room = MAX_STDERR.saturating_sub(kept.len());
                kept.extend_from_slice(&line[..line.len().min(room)]);
                if kept.len() < MAX_STDERR {
                    kept.push(b'\n');
                }
            }
            None => {}
        }
        line.clear();
    };
    while let Ok(n) = pipe.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\r' || b == b'\n' {
                take(&mut line, &mut kept);
            } else if line.len() < MAX_STDERR {
                line.push(b);
            }
        }
    }
    take(&mut line, &mut kept);
    kept
}

/// The error for a git that exited unsuccessfully.
fn failure(code: Option<i32>, stderr: &[u8], command: &str, cwd: &Path) -> GitError {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    if stderr.contains("not a git repository") {
        return GitError::NotARepository(cwd.to_path_buf());
    }
    if stderr.contains("detected dubious ownership") {
        return GitError::DubiousOwnership(cwd.to_path_buf());
    }
    GitError::Failed {
        command: command.to_owned(),
        code,
        stderr,
    }
}

/// A running git whose stdout is read record by record. Dropping it stops git.
pub struct GitStream {
    child: Child,
    reaped: bool,
    stdout: Option<std::io::BufReader<std::process::ChildStdout>>,
    stderr: Option<std::thread::JoinHandle<Vec<u8>>>,
    call: Call,
}

impl GitStream {
    /// The next record up to `separator` (not included), or `None` at the end of the output.
    /// A record longer than `max` bytes is an error (git is stopped).
    pub fn next_record(&mut self, separator: u8, max: usize) -> Result<Option<Vec<u8>>, GitError> {
        use std::io::BufRead;
        let Some(out) = self.stdout.as_mut() else {
            return Ok(None);
        };
        let mut record = Vec::new();
        let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
        let n = out
            .by_ref()
            .take(limit)
            .read_until(separator, &mut record)
            .map_err(|e| GitError::Spawn(e.to_string()))?;
        if n == 0 {
            self.stdout = None;
            return Ok(None);
        }
        if record.last() == Some(&separator) {
            record.pop();
        } else if record.len() > max {
            kill(&mut self.child);
            return Err(GitError::OutputTooLarge {
                command: self.call.command.clone(),
                limit: max,
            });
        }
        Ok(Some(record))
    }

    /// Waits for git after the output has been read; reports a failed exit.
    pub fn finish(mut self) -> Result<(), GitError> {
        self.stdout = None;
        let status = self
            .child
            .wait()
            .map_err(|e| GitError::Spawn(e.to_string()))?;
        self.reaped = true;
        let stderr = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        self.call.record(status.code(), &stderr);
        if status.success() {
            Ok(())
        } else {
            Err(failure(
                status.code(),
                &stderr,
                &self.call.command,
                &self.call.cwd,
            ))
        }
    }
}

impl Drop for GitStream {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // Unread output: git is no longer needed.
        kill(&mut self.child);
        let _ = self.child.wait();
        self.call.record(None, b"");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_of_all_platforms() {
        let v = |major, minor, patch| GitVersion {
            major,
            minor,
            patch,
        };
        assert_eq!(GitVersion::parse("git version 2.43.0\n"), Some(v(2, 43, 0)));
        assert_eq!(
            GitVersion::parse("git version 2.39.2.windows.1"),
            Some(v(2, 39, 2))
        );
        assert_eq!(
            GitVersion::parse("git version 2.39.3 (Apple Git-145)"),
            Some(v(2, 39, 3))
        );
        assert_eq!(GitVersion::parse("git version 2.45"), Some(v(2, 45, 0)));
        assert_eq!(
            GitVersion::parse("git version 2.50.0-rc1"),
            Some(v(2, 50, 0))
        );
        assert_eq!(GitVersion::parse("hg version 6"), None);
        assert_eq!(GitVersion::parse("git version x.y"), None);
        assert!(v(2, 29, 9) < MIN_VERSION && v(2, 30, 0) >= MIN_VERSION);
    }

    /// A git that hangs is stopped after the timeout.
    #[cfg(unix)]
    #[test]
    fn a_hanging_process_is_stopped() {
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = std::time::Instant::now();
        let r = collect(
            child,
            &Call::bare("sleep"),
            None,
            Some(Duration::from_millis(200)),
            None,
            None,
        );
        assert!(matches!(r, Err(GitError::TimedOut { .. })), "{r:?}");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    // Only the Unix tests below start a stand-in process.
    #[cfg(unix)]
    impl Call {
        fn bare(command: &str) -> Call {
            Call {
                command: command.to_owned(),
                shown: Vec::new(),
                cwd: PathBuf::from("."),
                write: false,
                started: Instant::now(),
                time: SystemTime::now(),
                log: None,
            }
        }
    }

    /// Cancelling ends the process and everything it started: the background `sleep` holds
    /// the output pipe open, so without the group kill this would wait 30 s.
    #[cfg(unix)]
    #[test]
    fn cancel_stops_the_whole_process_group() {
        use std::os::unix::process::CommandExt;
        let child = Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30; wait"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let cancel = Cancel::new();
        let later = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            later.cancel();
        });
        let start = std::time::Instant::now();
        let r = collect(child, &Call::bare("sh"), None, None, Some(cancel), None);
        assert_eq!(r, Err(GitError::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn progress_is_reported_and_kept_out_of_the_message() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let report: OnProgress = Arc::new(move |p| {
            if let Ok(mut s) = sink.lock() {
                s.push((p.phase, p.done));
            }
        });
        let mut input: Vec<u8> = Vec::new();
        for i in 0..5000 {
            input.extend_from_slice(
                format!("Receiving objects: {}% ({i}/5000)\r", i / 50).as_bytes(),
            );
        }
        input.extend_from_slice(
            b"Receiving objects: 100% (5000/5000), done.\nfatal: the real reason\n",
        );
        let kept = drain_progress(&input[..], &report);
        assert_eq!(String::from_utf8_lossy(&kept), "fatal: the real reason\n");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 5001);
        assert_eq!(seen.last(), Some(&("Receiving objects".to_owned(), 5000)));
    }

    #[test]
    fn missing_git_is_reported_as_not_installed() {
        let runner = GitRunner::with_program("/nonexistent/gitcraft-test/git");
        assert_eq!(runner.version(), Err(GitError::NotInstalled));
    }
}
