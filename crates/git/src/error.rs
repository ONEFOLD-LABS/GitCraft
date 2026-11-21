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
    /// The repository's program settings are not trusted (yet): GitCraft refuses to write.
    NotTrusted,
    /// A value GitCraft refuses to hand to git (empty, or with control characters).
    InvalidValue(String),
    /// A path GitCraft refuses to change on disk (outside the working tree, through a
    /// symbolic link, reserved name).
    UnsafePath(String),
    /// The file changed since its diff was shown; nothing was applied.
    Stale,
    /// A branch or tag name GitCraft or git refuses.
    InvalidName {
        name: String,
        problem: crate::NameProblem,
    },
    /// A remote URL GitCraft refuses (credentials already hidden).
    InvalidUrl {
        url: String,
        problem: crate::UrlProblem,
    },
    /// The current branch has no upstream to pull from or compare with.
    NoUpstream,
    /// A fast-forward is not possible: both sides have commits of their own.
    Diverged { ahead: u32, behind: u32 },
    /// A pull stopped with conflicts; the merge or rebase is in progress (git's report,
    /// credentials hidden).
    MergeConflicts(String),
    /// The remote refused these refs (remote ref name, why); nothing else went wrong.
    PushRejected(Vec<(String, crate::Rejection)>),
    /// The remote refused the login or none could be given (git's report, credentials
    /// hidden).
    Auth(String),
    /// No credential helper is configured to hand credentials to.
    NoCredentialHelper,
    /// A switch would overwrite local changes in these files; nothing was changed.
    WouldOverwrite(Vec<String>),
    /// Applying left conflicts in the working tree (git's report).
    Conflicts(String),
    /// The commit is already on a remote; undoing it would need a force push.
    Pushed,
    /// The user cancelled the call; git and what it started were stopped.
    Cancelled,
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
            GitError::Cancelled => write!(f, "cancelled"),
            GitError::Pushed => write!(f, "the commit is already on a remote"),
            GitError::Conflicts(_) => write!(f, "applying left conflicts"),
            GitError::InvalidName { name, problem } => {
                write!(f, "invalid name {name:?} ({problem:?})")
            }
            GitError::WouldOverwrite(files) => write!(
                f,
                "switching would overwrite local changes in {}",
                files.join(", ")
            ),
            GitError::InvalidUrl { url, problem } => {
                write!(f, "invalid remote URL {url:?} ({problem:?})")
            }
            GitError::NoUpstream => write!(f, "the current branch has no upstream"),
            GitError::Diverged { ahead, behind } => write!(
                f,
                "the branch and its upstream have diverged ({ahead} ahead, {behind} behind)"
            ),
            GitError::MergeConflicts(_) => write!(f, "the pull stopped with conflicts"),
            GitError::PushRejected(refs) => write!(f, "the remote rejected {refs:?}"),
            GitError::Auth(e) => write!(f, "authentication failed: {e}"),
            GitError::NoCredentialHelper => write!(f, "no credential helper is configured"),
            GitError::UnsafePath(p) => write!(f, "refused path {p}"),
            GitError::Stale => write!(f, "the file changed since its diff was shown"),
            GitError::InvalidValue(v) => write!(f, "invalid value {v:?}"),
            GitError::NotTrusted => write!(
                f,
                "the repository runs programs from its own configuration and is not trusted"
            ),
            GitError::UnsafeConfig(key) => write!(
                f,
                "the repository configuration sets {key}, which GitCraft cannot switch off safely"
            ),
            GitError::Parse(e) => write!(f, "unexpected git output: {e}"),
        }
    }
}

impl std::error::Error for GitError {}
