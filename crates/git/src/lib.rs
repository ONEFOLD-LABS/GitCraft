//! GitCraft's Git layer (ADR-002): runs the installed `git` executable and turns its
//! machine-readable output into domain types. Nothing outside this crate spawns git or sees its
//! raw output.
//!
//! - [`GitRunner`] spawns git safely (argument list, fixed environment, no prompts, repository
//!   configuration cannot start programs for reads).
//! - [`Repository`] is an opened working tree: [`Repository::status`], [`Repository::refs`],
//!   [`Repository::stashes`], and [`Repository::log`] for the history in pages.

mod diff;
mod display;
mod error;
mod log;
mod path;
mod refs;
mod repo;
mod runner;
mod status;

pub use diff::{ChangedFile, FileDiff, FileStatus, Hunk, Line, LineKind, MAX_LINES};
pub use display::{escape, escape_bytes};
pub use error::GitError;
pub use log::{Commit, LogReader, Signature};
pub use path::RepoPath;
pub use refs::{Ref, RefKind, Stash, Tracking};
pub use repo::{Repository, UNTRACKED_MAX};
pub use runner::{GitRunner, GitStream, GitVersion, MIN_VERSION};
pub use status::{Change, Conflict, FileChange, Head, Oid, State, Status, Upstream};
