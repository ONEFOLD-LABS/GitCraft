//! The command log (issue #15): every git call GitCraft made, so that each action can be
//! understood and repeated on the command line (spec: Git first). Credentials in URLs are
//! redacted before anything is stored; the environment is not recorded.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// Entries kept; older ones are dropped.
const KEEP: usize = 1000;
/// Characters of git's error output kept per entry.
const STDERR_KEEP: usize = 2000;

/// One git call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub time: SystemTime,
    pub cwd: PathBuf,
    /// Arguments after `git`, credentials redacted.
    pub args: Vec<String>,
    pub duration: Duration,
    /// Exit code; `None` when git was stopped (cancelled, timed out, output not needed).
    pub code: Option<i32>,
    /// The start of git's error output, credentials redacted.
    pub stderr: String,
    /// The call could change the repository.
    pub write: bool,
}

impl LogEntry {
    pub(crate) fn new(
        time: SystemTime,
        cwd: PathBuf,
        args: &[String],
        duration: Duration,
        code: Option<i32>,
        stderr: &[u8],
        write: bool,
    ) -> LogEntry {
        let stderr: String = String::from_utf8_lossy(stderr)
            .trim()
            .chars()
            .take(STDERR_KEEP)
            .collect();
        LogEntry {
            time,
            cwd,
            args: args.iter().map(|a| redact(a)).collect(),
            duration,
            code,
            stderr: redact(&stderr),
            write,
        }
    }

    /// The call as a line for this platform's shell (POSIX shell, on Windows PowerShell).
    pub fn shell_line(&self) -> String {
        if cfg!(windows) {
            self.powershell_line()
        } else {
            self.posix_line()
        }
    }

    /// The call as a POSIX shell line: `git` and every argument single-quoted where needed.
    pub fn posix_line(&self) -> String {
        std::iter::once("git".to_owned())
            .chain(self.args.iter().map(|a| posix_quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The call with a change into its working directory first, for this platform's shell.
    pub fn shell_line_in_dir(&self) -> String {
        if cfg!(windows) {
            self.powershell_line_in_dir()
        } else {
            self.posix_line_in_dir()
        }
    }

    pub fn posix_line_in_dir(&self) -> String {
        format!(
            "cd {} && {}",
            posix_quote(&self.cwd.to_string_lossy()),
            self.posix_line()
        )
    }

    pub fn powershell_line_in_dir(&self) -> String {
        format!(
            "Set-Location -LiteralPath {}; {}",
            powershell_quote(&self.cwd.to_string_lossy()),
            self.powershell_line()
        )
    }

    /// The call as a PowerShell line.
    pub fn powershell_line(&self) -> String {
        std::iter::once("git".to_owned())
            .chain(self.args.iter().map(|a| powershell_quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The calls of one session, newest last. Shared between threads.
#[derive(Debug, Default)]
pub struct CommandLog {
    entries: Mutex<VecDeque<LogEntry>>,
    /// Calls recorded so far (also those already dropped): changes with every call.
    recorded: AtomicU64,
}

impl CommandLog {
    pub fn new() -> CommandLog {
        CommandLog::default()
    }

    pub(crate) fn record(&self, entry: LogEntry) {
        if let Ok(mut e) = self.entries.lock() {
            if e.len() >= KEEP {
                e.pop_front();
            }
            e.push_back(entry);
        }
        self.recorded.fetch_add(1, Ordering::SeqCst);
    }

    /// Number of calls recorded since the log was created; a viewer re-reads the entries when
    /// it changes.
    pub fn recorded(&self) -> u64 {
        self.recorded.load(Ordering::SeqCst)
    }

    /// A copy of the entries, oldest first.
    pub fn entries(&self) -> Vec<LogEntry> {
        self.entries
            .lock()
            .map(|e| e.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Replaces the user information of every URL in `text` (`scheme://user:password@host`) so that
/// no password or token is kept: `https://***@host`.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (before, after) = rest.split_at(i + 3);
        out.push_str(before);
        // The authority ends at the first `/`, `?`, `#` or whitespace.
        let end = after
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(after.len());
        let authority = &after[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn posix_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

fn powershell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:\\".contains(c));
    if plain {
        arg.to_owned()
    } else {
        // Inside single quotes only `'` is special, written twice. Typographic quotes count as
        // quotes in PowerShell too.
        let escaped: String = arg
            .chars()
            .flat_map(|c| match c {
                '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => vec![c, c],
                c => vec![c],
            })
            .collect();
        format!("'{escaped}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(args: &[&str]) -> LogEntry {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        LogEntry::new(
            SystemTime::UNIX_EPOCH,
            PathBuf::from("/repo"),
            &args,
            Duration::ZERO,
            Some(0),
            b"",
            false,
        )
    }

    #[test]
    fn credentials_in_urls_are_redacted() {
        assert_eq!(
            redact("https://user:s3cret@example.com/repo.git"),
            "https://***@example.com/repo.git"
        );
        assert_eq!(redact("https://ghp_token@host"), "https://***@host");
        assert_eq!(
            redact("fatal: could not read from 'ssh://git@host:22/x' and https://a:b@c/d"),
            "fatal: could not read from 'ssh://***@host:22/x' and https://***@c/d"
        );
        assert_eq!(redact("https://example.com/a@b"), "https://example.com/a@b");
        assert_eq!(redact("no url here"), "no url here");
        let e = entry(&["push", "https://me:pw@host/r.git"]);
        assert_eq!(e.args[1], "https://***@host/r.git");
    }

    #[test]
    fn posix_lines_run_in_a_shell() {
        let e = entry(&[
            "-c",
            "core.fsmonitor=false",
            "commit",
            "-m",
            "it's a \"test\" $HOME",
        ]);
        assert_eq!(
            e.posix_line(),
            r#"git -c core.fsmonitor=false commit -m 'it'\''s a "test" $HOME'"#
        );
        assert_eq!(entry(&["add", ""]).posix_line(), "git add ''");
        assert_eq!(entry(&["add", "a b\nc"]).posix_line(), "git add 'a b\nc'");
    }

    /// The quoted line, given to a real shell, arrives as the original arguments.
    #[cfg(unix)]
    #[test]
    fn posix_quoting_round_trips_through_sh() {
        let args = ["plain", "it's", "a b", "$x `y` \\z", "-", "ü ✓", "\n", "'"];
        let e = entry(&args);
        let line = e.posix_line().replacen("git", "printf '%s\\0'", 1);
        let out = std::process::Command::new("sh")
            .args(["-c", &line])
            .output()
            .unwrap();
        let got: Vec<String> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8(p.to_vec()).unwrap())
            .collect();
        assert_eq!(got, args);
    }

    #[test]
    fn lines_with_the_working_directory() {
        let mut e = entry(&["status"]);
        e.cwd = PathBuf::from("/home/a b/it's");
        assert_eq!(
            e.posix_line_in_dir(),
            r"cd '/home/a b/it'\''s' && git status"
        );
        assert_eq!(
            e.powershell_line_in_dir(),
            "Set-Location -LiteralPath '/home/a b/it''s'; git status"
        );
    }

    #[test]
    fn powershell_lines_double_single_quotes() {
        assert_eq!(
            entry(&["commit", "-m", "it's $x"]).powershell_line(),
            "git commit -m 'it''s $x'"
        );
        assert_eq!(
            entry(&["add", "C:\\a\\b.txt"]).powershell_line(),
            "git add C:\\a\\b.txt"
        );
    }

    #[test]
    fn the_log_keeps_the_newest_entries() {
        let log = CommandLog::new();
        for i in 0..KEEP + 5 {
            log.record(entry(&[&i.to_string()]));
        }
        let e = log.entries();
        assert_eq!(e.len(), KEEP);
        assert_eq!(log.recorded(), (KEEP + 5) as u64);
        assert_eq!(e[0].args[0], "5");
    }
}
