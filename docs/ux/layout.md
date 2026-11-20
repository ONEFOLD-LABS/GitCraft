# Main window and history

How GitCraft's main window is laid out and operated. The spec's UI concept ("Repository /
Branches + Commit Graph + Changes / Details") made concrete. Behaviour is described in our own
words; nothing is taken from other products (AGENTS.md, clean room).

## Layout

```text
┌──────────────────────────────────────────────────────────────────────────────────┐
│ Toolbar: repository ▾  branch ▾ │ Undo │ Pull ▾ Push │ Branch Stash Pop │ Search │
├──────────────┬──────────────────────────────────────────────┬────────────────────┤
│ Sidebar      │ History                                       │ Details            │
│              │ refs        graph   message      author  date │                    │
│ ▾ Local   3  │             ◌       Work in progress  (3 · 1) │ commit or          │
│   ● main ↑1  │ [main]      ●       Fix the parser        MK  2h │ work in progress   │
│     feature  │             │ ●     Add the diff view     AB  5h │                    │
│ ▾ Remote     │ [origin/…]  ●─╯     Merge branch 'x'      MK  1d │ files, then diff   │
│ ▾ Tags       │             ●       Release 0.1            MK  3d │                    │
│ ▾ Stashes    │                                               │                    │
│ ▾ Worktrees  │                                               │                    │
├──────────────┴──────────────────────────────────────────────┴────────────────────┤
│ Status line: running operation · counts · git version                            │
└──────────────────────────────────────────────────────────────────────────────────┘
```

- The **history** is the centre and the main view; it is always there.
- The **sidebar** (left) and **details** (right) are resizable and can be hidden
  (`Ctrl+K Ctrl+B` / `Ctrl+K Ctrl+D`); their widths are kept per window.
- The **status line** names the running operation with a cancel button.

## History

One row per commit, newest first, in graph (topological) order.

| Column | Content |
|---|---|
| Refs | Labels for the branches and tags pointing at the commit: local branches filled in their lane colour, remote branches outlined, tags with a tag shape, `HEAD` marked. Too many labels collapse into `+n` with a tooltip. Resizable. |
| Graph | One lane per line of history, each in its own colour; commits as discs on their lane; merges and forks as curves. The commit at `HEAD` is drawn larger. |
| Message | Subject line; the rest of the message only in the details. |
| Author | Initials in a disc (no images from the network), name on hover. |
| Date | Relative ("2 h", "3 d"), absolute on hover; author or commit date, user's choice. |
| Hash | Short id, optional column. |

**Work-in-progress row.** Above the newest commit, as long as the working tree or index has
changes, a row "Work in progress" with a hollow, dashed node on `HEAD`'s lane and the counts
(changed · staged · conflicts). Selecting it shows the changes in the details (staging and
committing there from 0.3).

**Lanes.** A small set of lane colours (from the theme, readable on both grounds) assigned per
lane and kept stable while scrolling and after refreshes. The first-parent line of the current
branch keeps the first colour.

**Loading.** Pages of commits are read on demand while scrolling (issue #28); the graph layout
is incremental (#29). Scrolling never waits for git; unknown rows show a placeholder.

**Selection and keys.**

| Key | Action |
|---|---|
| ↑ ↓, Page Up/Down, Home/End | Move the selection |
| Enter | Open the selected commit's first file diff |
| Esc | Close the diff, back to the history |
| Ctrl+F | Search the history (#32) |
| ← → on a merge | Jump to first / second parent |
| Ctrl+C | Copy the commit id |

**Context menu on a commit.** Checkout (detached, with explanation), create branch here, create
tag here, cherry-pick, revert, reset current branch to here (soft / mixed / hard, each with the
concrete consequence), compare with working tree, copy id, copy message. Items appear in the
milestones that implement them.

**Context menu on a ref label.** Checkout, merge into current, rebase current onto it, rename,
delete, push, set upstream, copy name.

## Drag and drop (0.6)

Dragging a branch label (in the history or the sidebar) onto another branch offers what makes
sense for that pair: "Merge `a` into `b`", "Rebase `a` onto `b`", "Fast-forward `b` to `a`".
Nothing runs on drop alone; the choice opens the same confirmation as the menu entry.

## Details panel

- **Commit:** subject and body, author and committer with dates, id (copy), parents (links),
  refs, then the changed files as a list or tree with `+/-` counts. Selecting a file opens its
  diff in the centre in place of the history (Esc returns); the history keeps its scroll
  position and selection.
- **Work in progress:** unstaged and staged files with stage/unstage per file, hunk and line,
  the commit message box below (0.3).
- **Branch or tag** selected in the sidebar: its commit, upstream, ahead/behind.

## Sidebar

Sections Local, Remote (per remote), Tags, Stashes, Worktrees, each collapsible with counts and a
filter field at the top. The current branch is marked; branches show ahead/behind against their
upstream. Selecting an entry scrolls the history to its commit; double click checks out (0.4).

## Toolbar

Repository switcher, current branch (switch by typing), Undo, Pull (with mode: fetch only, fast
forward, merge, rebase), Push, Branch, Stash, Pop, search, command palette. Buttons are disabled
with a tooltip saying why (e.g. "Nothing to push").

**Undo** reverts GitCraft's last local operation where Git keeps what is needed: checkout,
commit (not yet pushed), branch create/delete, merge or rebase before push, discard (through
the recovery store). The button says what it will undo ("Undo commit 'Fix the parser'") and
uses the reflog; operations that reached a remote cannot be undone and say so.

## Look

Colours from the theme tokens (brand: `paper`, `surface`, `ink`, accent `app-gitcraft`); lane
colours as their own token list. Rows 28 px, labels and badges as pills (`radius-disc`), hairline
dividers in `line`. Everything virtualised; a frame stays under 2 ms with 100 000 commits loaded.
