# ADR-002: Git backend

Status: accepted, 2026-11-20

## Context

Git is large and subtle; its command line is the reference behaviour. A client that reimplements
Git risks being subtly incompatible (index extensions, config semantics, hooks, credential
helpers, LFS filters, worktrees, sparse checkout).

## Decision

GitCraft runs the installed `git` executable. All calls go through one crate (`crates/git`):

- `GitRunner` spawns git with an argument list (no shell), a fixed environment
  (`GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`, no pager, no optional locks for reads) and safety
  overrides on every call (`core.fsmonitor=false`, so repository config cannot start a
  program for a read).
- Output is machine readable (`--porcelain=v2 -z`, explicit `--format`s, NUL separators) and is
  parsed in the same crate into domain types (`Repository`, `Status`, `FileChange`, …). Raw output
  never reaches the UI.
- Minimum supported Git version: 2.30 (porcelain v2 with all fields used here).

The runner is behind a small interface so that a second backend (e.g. a pure-Rust implementation
for hot read paths such as history) can be added later without touching the UI.

## Alternatives

- **libgit2 bindings:** C dependency, its own subtle differences from Git (e.g. worktrees,
  config, filters), and still needs git for credential helpers and hooks.
- **Pure-Rust implementation (gitoxide):** promising and permissively licensed, but not complete
  for writes (merge, rebase, push). Candidate for read paths later.

## Consequences

- Users need Git installed (2.30+); GitCraft shows a clear message when it is missing or too old.
- Every operation pays a process start (a few ms); history and status must batch calls instead of
  running one per item.
- Tests create real temporary repositories with git; CI needs git, which all runners have.
- Git is GPL: GitCraft only executes it, never links or copies its code.
