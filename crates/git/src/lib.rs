//! GitCraft's Git layer (ADR-002): runs the installed `git` executable and turns its
//! machine-readable output into domain types. Nothing outside this crate spawns git or sees its
//! raw output.
//!
//! - [`GitRunner`] spawns git safely (argument list, fixed environment, no prompts, repository
//!   configuration cannot start programs for reads).
//! - [`Repository`] is an opened working tree; [`Repository::status`] reads its [`Status`].

mod error;
mod path;
mod repo;
mod runner;
mod status;

pub use error::GitError;
pub use path::RepoPath;
pub use repo::Repository;
pub use runner::{GitRunner, GitVersion, MIN_VERSION};
pub use status::{Change, Conflict, FileChange, Head, Oid, State, Status, Upstream};
