# GitCraft – product specification

This is the behaviour GitCraft aims for. Working rules for contributors and agents are in
`AGENTS.md`.

## Project Mission

GitCraft is an open-source, cross-platform desktop Git client.

GitCraft is part of ONEFOLD LABS: focused, polished, accessible open-source applications.

GitCraft's goal is **not** to clone any existing Git client feature-for-feature. It is designed clean-room from Git's own behaviour and public documentation.

The goal is to create a Git client that makes Git powerful, understandable, fast, and pleasant to use.

The underlying Git model must remain visible and predictable. GitCraft should simplify the workflow, not hide Git behind opaque abstractions.

---

# Core Principles

## 1. Git first

GitCraft is a Git client, not a general developer platform.

Git is the source of truth.

Do not invent a parallel version-control model.

Whenever possible, GitCraft should expose concepts that correspond directly to real Git concepts:

- repository
- working tree
- staging area
- commit
- branch
- tag
- remote
- upstream
- merge
- rebase
- stash
- worktree
- HEAD

Users should be able to understand what GitCraft does and, if necessary, reproduce it using Git CLI commands.

---

## 2. Local first

GitCraft must work without an account.

A user should be able to:

- open a local repository
- create a repository
- clone a repository
- commit
- branch
- merge
- rebase
- inspect history
- work with remotes

without creating a GitCraft account.

There should be no mandatory cloud backend.

---

## 3. Open source

GitCraft is intended to be open-source software.

Do not introduce proprietary infrastructure or mandatory hosted services unless explicitly approved.

Avoid unnecessary telemetry.

Privacy should be a deliberate design principle rather than an afterthought.

---

## 4. Cross-platform

Target platforms:

Linux (x86_64, aarch64, riscv64), Windows, macOS (universal) and FreeBSD.

There is no web build: GitCraft drives the installed `git` executable, which a browser cannot run.

Platform-specific functionality must be isolated behind appropriate abstractions.

Do not build the application around assumptions that only work on one operating system.

---

## 5. Keyboard-first

GitCraft should be highly usable without a mouse.

Core operations should have keyboard shortcuts.

A command palette should eventually provide access to most important actions.

---

## 6. Safe by default

Git can destroy user work.

Operations such as:

- reset --hard
- clean
- discard changes
- branch deletion
- force push
- stash deletion

must be clearly communicated.

Do not use generic confirmation dialogs such as:

> Are you sure?

Explain the actual consequence.

Example:

> This will permanently discard changes in 3 files.

---

# Product Positioning

GitCraft should feel like a carefully crafted developer tool rather than an enterprise management platform.

The product should prioritize:

1. clarity
2. speed
3. information density
4. predictable behavior
5. keyboard accessibility
6. excellent diffs
7. excellent history visualization
8. worktree support
9. strong error handling
10. polished UX

Do not add features simply because competing Git clients have them.

Every feature should have a clear reason to exist.

---

# Initial Product Scope

## MVP / v0.1

The first usable release should focus on:

### Repository

- Open repository
- Clone repository
- Initialize repository
- Recent repositories
- Repository status
- Git configuration

### Changes

- Modified files
- Added files
- Deleted files
- Renamed files
- Untracked files
- Stage file
- Unstage file
- Stage hunk
- Unstage hunk
- Discard changes
- Diff viewer

### Commits

- Create commit
- Amend commit
- Commit details
- Commit diff
- Commit history

### Branches

- List branches
- Create branch
- Checkout branch
- Delete branch
- Rename branch
- Compare branches

### Remotes

- Fetch
- Pull
- Push
- Add/remove remote
- Upstream tracking

### Merge

- Merge
- Detect conflicts
- Abort merge
- Continue merge
- Basic conflict resolution

### Stash

- Create stash
- List stashes
- Apply stash
- Pop stash
- Drop stash

### Tags

- List tags
- Create tag
- Delete tag
- Push tag

### History

- Visual commit graph
- Commit selection
- Commit details
- Commit diff
- Branch visualization
- Search

---

# Explicitly Out of Scope for v0.1

Do NOT implement these unless explicitly requested:

- issue tracking of hosting platforms
- external issue trackers
- project management
- CI/CD management
- integrated IDE
- integrated code editor
- integrated terminal emulator
- cloud synchronization
- GitCraft accounts
- mandatory telemetry
- plugin marketplace
- mobile application
- AI agent
- autonomous Git operations

These may be considered later.

---

# Important Future Features

The architecture should not prevent:

- Git worktrees
- interactive rebase
- cherry-pick
- revert
- advanced conflict resolution
- Git LFS
- submodules
- sparse checkout
- GitHub integration
- GitLab integration
- Forgejo integration
- AI-assisted commit messages
- AI-assisted conflict explanations
- history exploration
- command palette
- extensions/plugins

However, do not prematurely implement these features.

---

# Architecture

The architecture should separate:

```text
UI
 ↓
Application layer
 ↓
Domain model
 ↓
Git abstraction
 ↓
Git implementation
```

The UI must never directly construct arbitrary Git CLI commands.

Use a dedicated Git abstraction layer.

For example:

```text
UI
 ↓
RepositoryService
 ↓
GitRepository
 ↓
GitCommandRunner
 ↓
git executable
```

The exact architecture may evolve.

---

# Git Implementation Strategy

Initially, prefer using the installed Git executable rather than immediately implementing Git functionality ourselves.

Reasons:

- Git is extremely complex.
- Git CLI behavior is the reference behavior.
- It reduces implementation risk.
- It avoids accidentally creating a subtly incompatible Git implementation.

The architecture should allow the Git backend to be replaced or supplemented later.

Potential future backends may include:

- libgit2
- another native Git implementation
- specialized optimized implementations

Do not make this decision prematurely.

Create an abstraction first.

---

# Domain Model

The application should have explicit domain models for concepts such as:

```text
Repository
WorkingTree
Branch
Remote
Commit
Tag
Stash
Worktree
FileChange
Diff
Conflict
```

Do not allow raw Git command output to leak throughout the application.

Parsing and normalization belong in the Git layer.

---

# Git Edge Cases

GitCraft must be designed with Git's actual complexity in mind.

Important cases include:

- empty repository
- unborn HEAD
- detached HEAD
- merge commits
- rebases
- conflicts
- deleted files
- renamed files
- binary files
- Unicode filenames
- spaces in filenames
- unusual filenames
- CRLF/LF differences
- symlinks
- shallow repositories
- large repositories
- large diffs
- worktrees
- submodules
- Git LFS
- signed commits
- hooks
- non-fast-forward pushes
- force pushes

Not every case must be fully supported in v0.1.

However, the architecture must not make future support unnecessarily difficult.

---

# Performance

GitCraft must remain responsive on large repositories.

Never assume that repositories are small.

Avoid:

- loading entire Git history synchronously on startup
- rendering thousands of UI elements unnecessarily
- blocking the UI thread during Git operations
- loading huge diffs into memory unnecessarily

Use:

- asynchronous operations
- lazy loading
- virtualization
- caching
- background Git operations

where appropriate.

---

# Security

Treat repositories as potentially untrusted input.

Important requirements:

- Never construct shell commands through string concatenation.
- Pass Git arguments safely.
- Do not expose credentials in logs.
- Do not store credentials in plain text.
- Be careful with Git hooks.
- Do not automatically execute arbitrary repository code.
- Validate external input.
- Keep destructive operations explicit.
- Do not send repository contents to external services without explicit user action.

Security-sensitive decisions should be documented.

---

# Credentials

Prefer existing platform and Git mechanisms.

Support should eventually include:

- SSH
- HTTPS
- Git Credential Manager
- platform credential stores

Do not invent a custom credential storage system unless there is a compelling reason.

Never log:

- passwords
- access tokens
- private keys
- authentication headers

---

# UI Concept

The primary application layout should revolve around three concepts:

```text
Repository / Branches
        +
Commit Graph
        +
Changes / Details
```

The UI should avoid excessive navigation between separate screens.

A typical workflow should feel like:

```text
Change files
    ↓
Review diff
    ↓
Stage
    ↓
Commit
    ↓
Review history
    ↓
Push
```

The user should always understand:

- where they are
- which branch they are on
- what changed
- what is staged
- what is committed
- whether the branch is ahead/behind
- what operation is currently running

---

# Diff Viewer

The diff viewer is a core product feature.

It should eventually support:

- unified diff
- split diff
- file-level staging
- hunk-level staging
- line-level staging
- binary file indication
- rename detection
- syntax highlighting where appropriate

Do not treat the diff viewer as a minor component.

---

# Commit Graph

The commit graph is another core feature.

It should eventually support:

- commit nodes
- branch lines
- merges
- local branches
- remote branches
- HEAD
- tags
- commit selection
- commit details
- commit actions

Performance matters heavily here.

Use virtualization/lazy rendering where necessary.

---

# Command Palette

GitCraft should eventually provide a command palette similar to modern developer tools.

Example:

```text
Search GitCraft

> Commit
> Push
> Pull
> Fetch
> Create Branch
> Checkout Branch
> Create Worktree
> Merge
> Rebase
> Stash
> Open Terminal
```

The command palette should expose actions, not just search text.

---

# AI

AI is optional and must not define the product.

Potential future functionality:

- generate commit message
- summarize changes
- explain conflicts
- explain historical changes
- answer questions about repository history

AI must never silently execute destructive Git operations.

AI-generated actions should be transparent and reviewable.

Do not introduce an AI dependency into the core Git functionality.

---

# Testing

GitCraft requires a strong integration-test strategy.

Use temporary real Git repositories for Git behavior tests.

Test workflows such as:

```text
initialize
commit
branch
checkout
modify
stage
unstage
merge
conflict
rebase
stash
tag
push
pull
fetch
worktree
```

Important edge cases should become regression tests.

Do not rely exclusively on mocked Git behavior.

---

# Definition of Done

A feature is not complete merely because the happy path works.

A feature should be considered complete only when relevant:

- happy path
- error path
- loading state
- cancellation
- keyboard interaction
- accessibility
- platform behavior
- logging
- tests
- documentation

have been considered.

For destructive operations, safety behavior is part of the feature.

---

# Development Process

## Important

Do NOT immediately start implementing large amounts of code.

Before major implementation:

1. Inspect the existing repository.
2. Identify current architecture.
3. Identify available tooling.
4. Identify constraints.
5. Propose the smallest sensible implementation.
6. Identify risks.
7. Implement incrementally.
8. Run tests.
9. Review the result.
10. Update documentation.

Do not rewrite working code without a concrete reason.

Do not introduce frameworks or dependencies merely because they are popular.

---

# Technology Decisions

Technology choices must be justified.

Before committing to major technologies, compare realistic alternatives.

The UI stack is decided: egui/eframe, see `docs/decisions/ADR-001-ui-framework.md`.
Tauri, Electron, webviews and handwritten JS/TS are excluded (`AGENTS.md`).

Further technology choices are evaluated based on:

- performance
- native feel
- accessibility
- keyboard support
- rendering
- cross-platform behavior
- packaging
- ecosystem maturity
- developer experience
- long-term maintainability
- complexity

Do not choose a technology simply because it is familiar.

Document important architectural decisions.

---

# Architecture Decision Records

Use ADRs for important decisions.

Examples:

```text
docs/decisions/
  ADR-001-ui-framework.md
  ADR-002-git-backend.md
  ADR-003-credential-handling.md
  ADR-004-update-mechanism.md
  ADR-005-telemetry-policy.md
```

Each ADR should explain:

- context
- decision
- alternatives
- consequences

---

# Repository Structure

A possible structure is:

```text
gitcraft/
├── apps/
│   └── desktop/
│
├── crates/
│   ├── gitcraft-core/
│   ├── gitcraft-git/
│   ├── gitcraft-domain/
│   ├── gitcraft-diff/
│   ├── gitcraft-graph/
│   ├── gitcraft-config/
│   └── gitcraft-platform/
│
├── tests/
│   ├── git/
│   ├── repositories/
│   └── fixtures/
│
├── docs/
│   ├── architecture/
│   ├── ux/
│   └── decisions/
│
├── assets/
├── scripts/
│
├── README.md
├── CONTRIBUTING.md
├── LICENSE
└── Cargo.toml
```

This is a starting point, not a rigid requirement.

Adapt it to the selected technology stack.

---

# Roadmap

## Phase 0 — Foundation

- repository setup
- license
- CI
- development environment
- architecture
- UI technology decision
- Git abstraction
- logging
- error handling

## Phase 1 — Repository

- open repository
- clone
- initialize
- repository state
- branches

## Phase 2 — Changes

- status
- diff
- stage
- unstage
- discard
- commit

## Phase 3 — History

- commit graph
- commit details
- commit diff
- branch visualization
- search

## Phase 4 — Branching

- create
- checkout
- rename
- delete
- merge
- rebase

## Phase 5 — Remote

- fetch
- pull
- push
- upstream
- authentication

## Phase 6 — Conflicts

- conflict detection
- conflict UI
- ours/theirs
- mark resolved
- continue
- abort

## Phase 7 — Worktrees

- create
- remove
- open
- branch/worktree management

## Phase 8 — Polish

- keyboard shortcuts
- command palette
- accessibility
- performance
- onboarding
- settings
- error UX

## Phase 9 — Release

- Linux (x86_64, aarch64, riscv64)
- Windows
- macOS (universal)
- FreeBSD
- installers
- signing
- update mechanism
- documentation
- release process

---

# What GitCraft Must NOT Become

Avoid feature creep.

GitCraft should not become:

- an IDE
- an issue tracker
- a code hosting platform
- a project management platform
- a cloud service
- an AI coding agent
- an enterprise collaboration suite

The product should remain excellent at one thing:

> **Making Git pleasant to use.**

---

# Current Objective

The immediate objective is NOT to implement the entire product.

The immediate objective is:

1. establish the project repository
2. evaluate the UI technology
3. establish the architecture
4. create the initial project skeleton
5. implement the Git abstraction
6. open a real local Git repository
7. display its basic status
8. build from there incrementally

The first milestone is a small but real GitCraft application, not a large prototype full of mocked functionality.

---

# Product Quality Bar

GitCraft should feel like software that someone would willingly use every day.

The goal is not:

> "It works."

The goal is:

> "This is a really good Git client."

That distinction should guide product, architecture, UX, testing, and implementation decisions.
