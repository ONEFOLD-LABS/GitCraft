# Roadmap

The order in which GitCraft grows, derived from the phases in [spec.md](spec.md#roadmap). Each
milestone below is a GitHub milestone; each bullet is (at least) one issue. A milestone is done
when its issues meet the Definition of Done in the spec (happy path, error path, loading
state, cancellation, keyboard, accessibility, platforms, logging, tests, documentation).

The spec's "MVP / v0.1" scope (repository, changes, commits, branches, remotes, merge, stash,
tags, history) is reached with milestone **0.6**; versions here count milestones, so the first
release for everyday use is **1.0**, after polish (0.8) and release work (0.9).

| Milestone | Goal |
|---|---|
| 0.1 – Foundation and status | A real repository opens and shows its status; project infrastructure in place |
| 0.2 – Layout and history | The main window: history graph in the centre, sidebar, commit details with diffs |
| 0.3 – Changes and commit | Review, stage, discard and commit at the work-in-progress row |
| 0.4 – Branches, tags and stash | Everyday branch work |
| 0.5 – Remotes and authentication | Clone, fetch, pull, push, safely authenticated |
| 0.6 – Merge, rebase and conflicts | Integrate work and resolve conflicts (MVP scope complete) |
| 0.7 – Worktrees | First-class worktree support |
| 0.8 – Polish | Keyboard, command palette, accessibility, performance, settings |
| 0.9 – Release | Packages, signing, updates, documentation → 1.0 |
| Later | Beyond 1.0: interactive rebase, LFS, submodules, hosting, AI, extensions |

## 0.1 – Foundation and status

- Workspace, CI gate on Linux/Windows/macOS, licence allowlist (done in #1)
- Open a repository and show its status (done in #2)
- Required CI check on `main`; FreeBSD CI leg
- RustSec advisories and dependency policy in CI
- Settings file (theme, language, recent repositories)
- Start screen with recent repositories; initialise a new repository
- Repository state detection (merge, rebase, cherry-pick, revert, bisect in progress)
- Automatic refresh through a file watcher
- About dialog with version, git version and third-party licences
- Logging with redaction

## 0.2 – Layout and history

The main window as described in [ux/layout.md](ux/layout.md); read-only.

- Main window layout: toolbar, sidebar, history, details, status line
- Streaming log reader in pages
- Commit graph layout (lanes, stable colours)
- History view: refs column, graph, message, author, date; work-in-progress row
- Sidebar: branches, remote branches, tags, stashes, worktrees
- Diff parser (unified, binary, renames, modes, submodules)
- Commit details and commit diff (merge commits); diff viewer, unified, virtualised
- Context menus on commits and ref labels (entries appear as their milestones land)
- Search (message, author, hash, path, content)
- File history
- Parser fuzzing; performance targets for large repositories

## 0.3 – Changes and commit

- Application layer `crates/core`: RepoSession, operation queue, events (ADR-006)
- Command log: every git command GitCraft ran, copyable
- Safe arguments (`--end-of-options`, pathspec from stdin) and checked paths
- Repository trust for program-running configuration
- Staging and committing in the details panel at the work-in-progress row
- Stage and unstage files; hunks; single lines
- Discard changes (files, hunks) with recovery; delete untracked files
- Commit: message editor, identity check, hooks, signing as configured
- Amend the last commit (warns when already pushed)

## 0.4 – Branches, tags and stash

- Branches: create, checkout (with local changes), rename, delete
- Checkout of a commit (detached HEAD) with explanation
- Compare branches
- Tags: list, create, delete, push
- Stash: create, list, show, apply, pop, drop
- Undo the last local operation (toolbar)

## 0.5 – Remotes and authentication

- Credential handling (ADR-003) and askpass bridge (passwords, passphrases, host keys)
- Fetch and pull with progress and cancellation
- Push, upstream, non-fast-forward explanation, force with lease only
- Remotes: list, add, rename, remove, URL redaction and validation
- Clone with progress and cancellation
- Upstream tracking UI; shallow repositories

## 0.6 – Merge, rebase and conflicts

- Merge (fast-forward or merge commit), abort
- Rebase onto (non-interactive), continue, skip, abort
- Cherry-pick and revert
- Operation-in-progress bar (continue, skip, abort with consequences)
- Conflict view: ours, theirs, base; take per hunk; mark resolved
- Drag a branch onto another to merge, rebase or fast-forward

## 0.7 – Worktrees

- Worktrees: list, create, open, remove, prune, lock
- One window per repository or worktree

## 0.8 – Polish

- Command palette
- Configurable keyboard shortcuts
- Settings dialog
- Diff options (whitespace, context, word diff) and syntax highlighting
- Split (side-by-side) diff
- Font fallback for all scripts; right-to-left file names
- Accessibility audit on all platforms
- First-start checks and onboarding
- Error message review

## 0.9 – Release

- Packages for Linux, Windows, macOS, FreeBSD (layout as in StudioCraft); aarch64 and riscv64
- Code signing and notarisation
- Update check, opt-in (ADR-004)
- User manual
- Release process and changelog

## Later

Interactive rebase, Git LFS, submodules, sparse checkout, blame, bisect, signature
verification display, hosting integrations (pull requests on GitHub, GitLab, Forgejo),
optional AI assistance (commit messages, conflict explanations), automation API for agents
(MCP), extensions.
