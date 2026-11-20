# ADR-006: Operations, progress and cancellation

Status: accepted, 2026-11-20

## Context

GitCraft must never block the UI, must show what is running, must let the user cancel long
operations, and must not run two writes on the same repository at once (they would collide on
git's index lock). The first milestone uses a simple worker that runs each read on its own
thread. Writes, progress (fetch, push, clone) and prompts (askpass) need more structure.

## Decision

An application layer (`crates/core`, see [architecture.md](../architecture.md)) owns all
operations:

- Every user action is a variant of a closed `Command` enum; core turns it into git calls.
- Per repository, writes run one at a time in submission order on a worker thread; reads
  (refresh, history pages, diffs) run in parallel and never wait for writes, but a refresh is
  queued after each write.
- Progress and outcomes come back as `Event`s polled by the UI once per frame; the worker
  asks the UI to repaint.
- Cancellation kills the git process together with its children (process group on Unix, job
  object on Windows), then refreshes; the UI shows the state Git left (e.g. a merge in
  progress) rather than assuming a rollback.
- Results of superseded reads (e.g. a refresh started before a newer one) are dropped.
- Every git call is recorded in a command log (arguments, duration, exit code; redacted).

## Alternatives

- **async runtime (tokio):** adds a large dependency for what is a handful of threads and
  blocking child processes.
- **UI calls git directly on a thread per action:** no ordering of writes, no central log, no
  shared cancellation.

## Consequences

- One place to test ordering, cancellation and error handling.
- The UI becomes a pure view of snapshots and events.
- The worker in `crates/ui/src/worker.rs` moves into core when it is introduced (roadmap 0.2).
