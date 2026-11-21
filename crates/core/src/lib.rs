//! GitCraft's application layer (ADR-006, docs/architecture.md): one [`RepoSession`] per open
//! repository owns its state and the order of operations. The UI sends [`Command`]s and reads
//! [`Event`]s once per frame; it never starts git itself and never builds git arguments.
//!
//! - Writes run one at a time, in the order they were submitted, on the session's own thread;
//!   a refresh follows every write.
//! - Reads (refresh, commit details, diffs) run in parallel and never wait for writes. When a
//!   newer read of the same kind was submitted meanwhile, the older one's result is dropped.
//! - [`RepoSession::cancel`] stops a write (git and everything it started); a write still
//!   waiting in the queue does not start.
//! - Every git call lands in the session's [`CommandLog`].

mod session;
mod task;

pub use session::{
    Command, DiffKey, Event, OperationId, Outcome, Reader, RepoSession, Snapshot, Wake,
};
pub use task::Task;
