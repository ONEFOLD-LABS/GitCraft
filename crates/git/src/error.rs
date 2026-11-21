use std::fmt;
use std::path::PathBuf;

use crate::runner::GitVersion;

/// Everything that can go wrong when talking to git. The UI turns each kind into its own
/// message; `Display` is the English text for logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// The git executable was not found.
    NotInstalled,
    /// git could not be started for another reason.
    Spawn(String),
    /// The installed git is older than [`crate::MIN_VERSION`].
    TooOld(GitVersion),
    /// The path is not inside a Git working tree.
    NotARepository(PathBuf),
    /// A bare repository (no working tree); not supported yet.
    Bare(PathBuf),
    /// git refuses the repository because another user owns it (`safe.directory`).
    DubiousOwnership(PathBuf),
    /// git ran and failed.
    Failed {
        /// The subcommand, e.g. `status`.
        command: String,
        code: Option<i32>,
        /// git's error output, trimmed and capped.
        stderr: String,
    },
    /// git wrote more output than GitCraft is willing to hold in memory.
    OutputTooLarge { command: String, limit: usize },
    /// git took longer than GitCraft allows for the call and was stopped.
    TimedOut { command: String, seconds: u64 },
    /// The repository's configuration names a program in a way GitCraft cannot switch off.
    UnsafeConfig(String),
    /// git's output did not have the expected form.
    Parse(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NotInstalled => write!(f, "git was not found"),
            GitError::Spawn(e) => write!(f, "git could not be started: {e}"),
            GitError::TooOld(v) => write!(
                f,
                "git {v} is too old, GitCraft needs {} or newer",
                crate::MIN_VERSION
            ),
            GitError::NotARepository(p) => write!(f, "{} is not a Git repository", p.display()),
            GitError::Bare(p) => write!(f, "{} is a bare repository", p.display()),
            GitError::DubiousOwnership(p) => write!(
                f,
                "git refuses {} because it is owned by another user",
                p.display()
            ),
            GitError::Failed {
                command,
                code,
                stderr,
            } => match code {
                Some(c) => write!(f, "git {command} failed with exit code {c}: {stderr}"),
                None => write!(f, "git {command} was terminated: {stderr}"),
            },
            GitError::OutputTooLarge { command, limit } => write!(
                f,
                "git {command} produced more than {} MiB of output",
                limit / (1024 * 1024)
            ),
            GitError::TimedOut { command, seconds } => {
                write!(
                    f,
                    "git {command} took longer than {seconds} s and was stopped"
                )
            }
            GitError::UnsafeConfig(key) => write!(
                f,
                "the repository configuration sets {key}, which GitCraft cannot switch off safely"
            ),
            GitError::Parse(e) => write!(f, "unexpected git output: {e}"),
        }
    }
}

impl std::error::Error for GitError {}
