//! GitCraft's Git layer (ADR-002): runs the installed `git` executable and turns its
//! machine-readable output into domain types. Nothing outside this crate spawns git or sees its
//! raw output.
//!
//! - [`GitRunner`] spawns git safely (argument list, fixed environment, no prompts, repository
//!   configuration cannot start programs for reads).
//! - [`Repository`] is an opened working tree: [`Repository::status`], [`Repository::refs`],
//!   [`Repository::stashes`], and [`Repository::log`] for the history in pages.

mod command_log;
mod diff;
mod discard;
mod display;
mod error;
mod log;
mod patch;
mod path;
mod refs;
mod repo;
mod runner;
mod status;
mod trust;

pub use command_log::{CommandLog, LogEntry, redact};
pub use diff::{ChangedFile, FileDiff, FileStatus, Hunk, Line, LineKind, MAX_LINES};
pub use discard::{DISCARDED_REFS, Discarded};
pub use display::{escape, escape_bytes};
pub use error::GitError;
pub use log::{Commit, LogReader, Signature};
pub use path::RepoPath;
pub use refs::{Ref, RefKind, Stash, Tracking};
pub use repo::{Identity, NameError, Repository, SearchKind, SearchStream, UNTRACKED_MAX};
pub use runner::{Cancel, GitRunner, GitStream, GitVersion, MIN_VERSION, Written};
pub use status::{Change, Conflict, FileChange, Head, Oid, State, Status, Upstream};
pub use trust::{ProgramSetting, TrustReport};
