# Architecture and internal API

How GitCraft is built and how its layers talk to each other. The product behaviour is in
[spec.md](spec.md), the order of work in [roadmap.md](roadmap.md), the security rules in
[security.md](security.md). Decisions with alternatives are recorded in [decisions/](decisions/).

## Layers

```text
apps/gitcraft          eframe app: window, command line, platform glue
      │
crates/ui              egui views: status, diff, history, sidebar, dialogs, command palette
      │  reads snapshots, sends Commands, receives Events
crates/core            application layer: RepoSession, operation queue, cache, command log
      │  calls typed functions
crates/git             Git layer: GitRunner, parsers, domain types (Status, Diff, Commit, …)
      │  argument lists, machine-readable output
git executable         the reference implementation (ADR-002)
```

Rules:

- Only `crates/git` starts git. Only `crates/core` calls `crates/git` for writes; the UI never
  builds git arguments and never sees raw git output.
- `crates/git` is synchronous and has no UI or threading knowledge. Every call takes what it
  needs (paths, refs) as typed values and returns domain types or a `GitError`.
- `crates/core` owns concurrency: reads may run in parallel, writes run one at a time per
  repository (a git index lock is per repository), each with progress and cancellation.
- `crates/ui` holds no repository state of its own beyond view state (selection, scroll,
  expanded sections). It renders the latest snapshot from `crates/core`.

The UI reaches git only through `crates/core`: commands and events of a `RepoSession`, plus a
read-only `Reader` for the history and search streams it pulls page by page.

## crates/git – the Git layer

### Runner

`GitRunner` is the only way to start git (see [security.md](security.md) for why each setting
exists):

| Function | Use | Locks | Hooks |
|---|---|---|---|
| `read(cwd, args)` | status, diff, log, show, rev-parse, for-each-ref | no optional locks (`GIT_OPTIONAL_LOCKS=0`) | never run |
| `write(cwd, config, args, input, cancel)` | add, restore, commit, branch, apply, … | normal | run where Git runs them |
| `stream(cwd, config, args)` | log pages, search results (fetch/push progress later) | as read | never run |

Each call runs in its own process group; cancelling a write ends git and everything it started
(on Windows only git itself so far). Every call is recorded in the session's command log.

Every call: argument list, `-c core.fsmonitor=false`, `GIT_TERMINAL_PROMPT=0`, C locale, no
pager, redirecting variables (`GIT_DIR`, …) removed, stdout and stderr capped, user-provided
positional values after `--end-of-options` or `--`, input on stdin instead of temporary files
where git allows it (`--pathspec-from-file=- --pathspec-file-nul`, `apply --cached -`).

### Parsers and domain types

One module per git output format, each with unit tests on handwritten input and integration
tests on real repositories; malformed input is a `GitError::Parse`, never a panic. Planned:

| Module | Git command | Types |
|---|---|---|
| `status` (done) | `status --porcelain=v2 --branch -z` | `Status`, `Head`, `Upstream`, `FileChange` |
| `diff` | `diff`/`show` with `--no-ext-diff --no-textconv --no-color -z` | `Diff`, `FileDiff`, `Hunk`, `Line` |
| `log` | `log -z --format=<fields>` | `Commit`, `Signature`, refs |
| `refs` | `for-each-ref --format=…` | `Branch`, `Tag`, `RemoteBranch` |
| `stash` | `stash list -z --format=…` | `Stash` |
| `remote` (done) | `config -z --get-regexp ^remote\..*\.(url\|pushurl)$` | `Remote`; URL check (`check_url`) and redaction (`redact_url`) |
| `worktree` | `worktree list --porcelain` (`-z` from git 2.36) | `Worktree` |
| `state` | files in the git dir (`MERGE_HEAD`, `rebase-merge/`, …) | `RepoState` |
| `progress` | `--progress` lines on stderr | `Progress { phase, done, total }` |

### Paths

`RepoPath` keeps file names as bytes (any encoding git stores). Display escapes control and
invisible characters. Turning a `RepoPath` into a file-system path for a destructive operation
goes through one checked function that rejects absolute paths, `..` components and paths that
leave the working tree through a symlink.

## crates/core – the application layer

### RepoSession

One per open repository (and per worktree window):

```rust
pub struct RepoSession { /* repository, trust report, queue, log */ }

impl RepoSession {
    /// Checks git, opens the working tree, reads the first snapshot (blocking; run in a Task).
    pub fn open(runner: GitRunner, path: &Path, wake: Wake)
        -> Result<(RepoSession, Arc<Snapshot>), GitError>;
    /// Queues a command; its outcome arrives as an Event (a refresh follows every write).
    pub fn submit(&self, command: Command) -> OperationId;
    pub fn cancel(&self, id: OperationId);
    /// Events since the last call; results of superseded reads never arrive.
    pub fn poll(&self) -> Vec<Event>;
    /// Every git call with arguments, duration and exit code (credentials redacted).
    pub fn command_log(&self) -> Arc<CommandLog>;
    /// Read-only access for streams (history pages, search).
    pub fn reader(&self) -> Reader;
}
```

### Commands and events

`Command` is a closed enum of everything the user can do (`Refresh`, `Stage(paths)`,
`StageHunk(..)`, `Commit { message, amend }`, `Checkout(..)`, `Fetch(..)`, …). Each command
documents the git commands it runs; the command log shows them so that every action can be
reproduced on the command line (spec: "Git first").

```rust
pub enum Event {
    Started { id: OperationId, title: String },
    Progress { id: OperationId, progress: Progress },
    Finished { id: OperationId, result: Result<Outcome, GitError> },
    Snapshot(Arc<Snapshot>),
    /// git asks for input (password, passphrase, host key) through the askpass bridge.
    Prompt { id: OperationId, prompt: Prompt },
}
```

- Writes are queued per repository and run in order; a read refresh follows every write.
- Cancellation kills the git process (and its process group/job object, so ssh and remote
  helpers end too) and refreshes the state; the UI then shows what Git left behind (for
  example a merge in progress) instead of guessing.
- Destructive commands carry a `Confirmation` built by core (what exactly will be lost), which
  the UI shows; the command runs only with the confirmation token. See
  [security.md](security.md#destructive-operations).

### Cache and refresh

The snapshot holds status, refs, repository state and the visible part of history. A file
watcher (debounced) and window focus trigger refreshes; history pages are loaded on demand.

## crates/ui – views

The main window's layout and operation are specified in [ux/layout.md](ux/layout.md).
View modules (`status`, `diff`, `history`, `sidebar`, `commit`, `dialogs`, `palette`) render
from the snapshot and send `Command`s. Conventions from AGENTS.md: theme tokens, `tr()`,
virtualised lists, accessible labels, render tests with screenshots.

## Public interfaces

- **Command line:** `gitcraft [path]`; later `gitcraft --diff <a> <b>` style entry points only
  where they help other tools (e.g. as `git difftool`).
- **Automation API (later):** an MCP server like StudioCraft's (`gitcraft-cli mcp`), exposing
  read tools and confirmed write tools over the same `Command` enum, with the same confirmation
  rules as the UI. Not before the UI covers the operations itself.
- **Extensions (later):** not designed yet; the `Command`/`Event` boundary is meant to be the
  surface they would use.

No stable Rust API is promised; the crates are internal to GitCraft.
