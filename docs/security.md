# Security

GitCraft runs git on repositories the user did not necessarily write, talks to remotes with the
user's credentials and can destroy work with a single click. This document lists what we
protect, against what, and the rules every change has to follow. Report vulnerabilities
privately (see CONTRIBUTING.md).

## What we protect

1. The user's work: uncommitted changes, stashes, local branches, unpushed commits.
2. The user's machine: no program runs because a repository says so, unless Git itself would
   run it for the operation the user chose.
3. The user's credentials: passwords, tokens, SSH keys and passphrases.
4. The user's privacy: no repository content or metadata leaves the machine unless the user
   pushes it; no telemetry (ADR-005).

## Threats and rules

### T1 – Repository configuration that runs programs

A repository's `.git/config` (and included files) can name programs: `core.fsmonitor`,
`core.pager`, `core.sshCommand`, `core.hooksPath`, `diff.external`, `diff.<driver>.command`,
`diff.<driver>.textconv`, `filter.<driver>.clean/smudge/process`, `merge.<driver>.driver`,
`credential.helper`, `gpg.program`, `uploadpack`/`receivepack` settings. Cloning never copies
this file, but a repository received as an archive or on a shared drive can contain it.

Rules:

- Reads never run such programs: `-c core.fsmonitor=false`, no pager, `--no-ext-diff`,
  `--no-textconv` for diffs (text conversion can be enabled per repository by the user later).
- Before the first write operation in a repository, GitCraft lists program-running settings
  from the repository's local configuration and asks the user once to trust them (stored per
  repository path). Untrusted repositories still allow reads.
- Hooks run only where Git runs them (commit, merge, rebase, push, checkout); GitCraft shows
  that hooks exist and shows their output. Skipping them (`--no-verify`) is an explicit choice.

- Content filters (`filter.<driver>.clean/smudge/process`) defined in the repository's local
  or worktree configuration are switched off for every read (`-c filter.<d>.clean=` and so on),
  because `status` runs a clean filter for every stat-dirty file, which is every file of a
  freshly extracted archive. Driver names that `-c` cannot address (containing `=`) make
  GitCraft refuse the repository. Filters from global or system configuration stay active.
  Tested with a planted filter (#86).

### T2 – Argument injection

Branch names, tags, remotes, URLs and paths come from users and repositories. A value starting
with `-` must never become an option.

Rules: arguments are lists (no shell); user-provided refs and URLs come after
`--end-of-options`; paths after `--` or through `--pathspec-from-file=- --pathspec-file-nul`
(which also avoids pathspec magic); new names are checked with `git check-ref-format` before
use. Tests try names like `--upload-pack=touch x` and `-rf`.

### T3 – Environment and redirection

Variables that redirect git or inject configuration are never inherited: `GIT_DIR`,
`GIT_WORK_TREE`, `GIT_INDEX_FILE`, object directories, `GIT_EXTERNAL_DIFF`, `GIT_CONFIG`,
`GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`/`KEY_n`/`VALUE_n`, `GIT_ATTR_SOURCE`,
`GIT_REPLACE_REF_BASE`, `GIT_GRAFT_FILE`, `GIT_SHALLOW_FILE`, `GIT_TRACE*` (tested for
`GIT_DIR`). Reads are stopped after 300 s. `GIT_TERMINAL_PROMPT=0`: git cannot block on a terminal prompt.

### T4 – Paths and file names

File names are bytes and may contain newlines, control, zero-width and direction characters.
Display escapes them, invalid bytes as `\xNN` (tested: `invoice\u{202e}txt.exe` does not read as
`invoiceexe.txt`). The same applies to ref names, remote names, paths and git's messages.
Destructive file operations (discard, delete untracked) resolve paths through one checked
function: no absolute paths, no `..`, nothing outside the working tree, symlinks are removed
as links and never followed.

### T5 – Hostile or huge output

Output of git is capped (stdout 256 MiB, stderr 16 KiB); diffs and histories are loaded in
pages and drawn virtualised; binary content is detected and not rendered as text; parsers
return errors instead of panicking and are fuzzed.

### T6 – Credentials

ADR-003. GitCraft stores no credentials. HTTPS uses Git's credential helpers; SSH uses the
agent and the user's keys. When git needs input, GitCraft acts as `GIT_ASKPASS`/`SSH_ASKPASS`:
the helper process asks the running window over a local channel bound to the session (random
token, user-only permissions) and returns the answer to git only. Prompts show which host and
user ask. Unknown SSH host keys are shown with their fingerprint and must be confirmed.
Credentials in remote URLs (`https://user:token@host`) are redacted in the UI, the command log
and log files; GitCraft offers to move them into the credential helper.

### T7 – Remotes and protocols

Only `https`, `ssh`, `git` and local paths are offered; `ext::` and `fd::` stay disabled (Git's
defaults are kept and never widened). Clone does not recurse into submodules by default;
submodule operations run with `protocol.file.allow=user` (Git's default since 2.38.1, set explicitly for older versions). Plain `http` and `git://` URLs show a
warning that the connection is not encrypted.

### Destructive operations

Discard changes, delete untracked files, reset, branch deletion with unmerged commits, stash
drop, force push, worktree removal with changes:

- The confirmation names the consequence and the objects ("discards changes in 3 files",
  "deletes branch `feature` with 2 commits that are on no other branch"), never "Are you sure?".
- Where possible a way back is kept: discarded file contents are written as blobs to the
  object database and listed under "Recently discarded" for a while; dropped stashes and
  deleted branches show the commit id to restore them.
- Force push is always `--force-with-lease=<ref>:<commit>` on exactly the remote commit the
  confirmation showed (GitCraft fetches before asking), so commits that reached the remote since
  are never overwritten; plain `--force` is not offered. (`--force-if-includes` guards against
  background fetches when the lease names no commit; with an explicit commit git ignores it.)
- Nothing destructive runs from a keyboard shortcut without the confirmation.

### Logging and privacy

Logs (and the command log in the UI) contain git arguments, durations and exit codes, with URLs
redacted and without file contents, commit messages or prompt answers. No network access
except what the user starts (fetch, pull, push, clone), the opt-in author images (ADR-005
amendment: e-mail hashes to gravatar.com, off by default) and the opt-in update check (ADR-004,
planned for 0.9).

### Supply chain

Permissive dependencies only (AGENTS.md rule 4, checked by `cargo xtask licenses`), RustSec
advisories checked in CI, GitHub Actions pinned to commits, Dependabot for updates. Release
artifacts are built in CI from tags and published with checksums (signing: roadmap 0.9).

## Checklist for every pull request

- No new place that starts git outside `crates/git`.
- User or repository values cannot become options (T2) and are tested with a leading `-`.
- Destructive actions name their consequence and keep a way back where possible.
- No secrets, file contents or messages in logs.
- Hostile input (names, output sizes, malformed output) has a test.
