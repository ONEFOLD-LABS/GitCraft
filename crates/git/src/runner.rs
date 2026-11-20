use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::GitError;

/// Oldest git GitCraft supports (porcelain v2 status with all fields used here, `init -b`).
pub const MIN_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 30,
    patch: 0,
};

/// Largest stdout GitCraft reads from one git call.
const MAX_STDOUT: usize = 256 * 1024 * 1024;
/// Largest stderr kept for an error message.
const MAX_STDERR: usize = 16 * 1024;

/// Environment variables that would point git at another repository, index or object store
/// than the one GitCraft opened; never inherited.
const REDIRECTING_ENV: &[&str] = &[
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

/// Starts git. Every call gets an argument list (never a shell), runs without a terminal prompt,
/// in the C locale, without a pager, and with `core.fsmonitor` switched off so that a
/// repository's own configuration cannot start a program while GitCraft only reads.
#[derive(Debug, Clone)]
pub struct GitRunner {
    program: PathBuf,
    env: Vec<(OsString, OsString)>,
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
        }
    }

    /// Sets an environment variable for every call (e.g. `GIT_CONFIG_GLOBAL` in tests).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
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
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let command = args
            .first()
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut cmd = Command::new(&self.program);
        cmd.current_dir(cwd)
            .args(["-c", "core.fsmonitor=false"])
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C");
        for key in REDIRECTING_ENV {
            cmd.env_remove(key);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
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
        collect(child, &command, cwd)
    }
}

/// Reads stdout (capped) and stderr (capped, on its own thread so neither pipe can block git),
/// waits for git and maps failures.
fn collect(mut child: Child, command: &str, cwd: &Path) -> Result<Vec<u8>, GitError> {
    let stderr_reader = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buf = [0u8; 4096];
            // Keep reading past the cap so that git never blocks on a full pipe.
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let room = MAX_STDERR.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
            kept
        })
    });
    let mut stdout = Vec::new();
    let mut too_large = false;
    if let Some(out) = child.stdout.take() {
        let limit = u64::try_from(MAX_STDOUT)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if let Err(e) = out.take(limit).read_to_end(&mut stdout) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitError::Spawn(e.to_string()));
        }
        too_large = stdout.len() > MAX_STDOUT;
    }
    if too_large {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| GitError::Spawn(e.to_string()))?;
    let stderr = stderr_reader
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    if too_large {
        return Err(GitError::OutputTooLarge {
            command: command.to_owned(),
            limit: MAX_STDOUT,
        });
    }
    if status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
    if stderr.contains("not a git repository") {
        return Err(GitError::NotARepository(cwd.to_path_buf()));
    }
    if stderr.contains("detected dubious ownership") {
        return Err(GitError::DubiousOwnership(cwd.to_path_buf()));
    }
    Err(GitError::Failed {
        command: command.to_owned(),
        code: status.code(),
        stderr,
    })
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

    #[test]
    fn missing_git_is_reported_as_not_installed() {
        let runner = GitRunner::with_program("/nonexistent/gitcraft-test/git");
        assert_eq!(runner.version(), Err(GitError::NotInstalled));
    }
}
