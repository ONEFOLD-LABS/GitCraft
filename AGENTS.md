# GitCraft – guide for contributors working with AI agents

This file is for AI coding agents and for the people who run them. `CLAUDE.md` points here. Read
it fully before the first change; [CONTRIBUTING.md](CONTRIBUTING.md) applies to agents as well.

GitCraft is a clean-room, pure-Rust desktop Git client, as described in `docs/spec.md`. It is a
project of ONEFOLD LABS; the rules in this file apply to all ONEFOLD LABS projects.

## Rules that are never bent

1. **A person is accountable.** Whoever runs an agent answers for every commit, pull request and
   review comment it produces. Read the diff before you open a pull request.
2. **`main` changes only through a pull request** with green CI. Never push to `main`, never
   force-push a shared branch. Branch protection enforces this.
3. **Clean room.** Learn from Git's own behaviour, its documentation and public specifications.
   Never decompile, disassemble or copy anything from other Git clients or other products: no
   code, icons, images, templates, wording beyond generic feature names. Do not name other
   proprietary products or their vendors anywhere in the project (UI, code, comments, docs,
   commits, issues, pull requests). Technologies GitCraft works with (Git, SSH, hosting APIs,
   credential helpers) may be named where interoperability needs it. Do not copy GPL/LGPL/AGPL
   code either (Git itself is GPL: read its documentation, never its source); permissive sources
   (MIT, Apache-2.0, BSD) only, credited in `NOTICE`.
4. **Licences.** Dependencies and assets must be MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib,
   OFL-1.1, CC0-1.0, CC-BY-4.0, BSL-1.0, Unicode-3.0 or Apache-2.0 WITH LLVM-exception. Also
   accepted: MPL-2.0 for unmodified dependencies only, and the Ubuntu Font Licence 1.0 for the
   default font egui embeds. Anything else: stop and ask a maintainer.
5. **Third-party notices ship with every copy.** `THIRD-PARTY-LICENSES.txt` holds the licence
   texts of all dependencies. After changing dependencies run `cargo xtask licenses` and commit
   the file; `cargo xtask ci` fails while it is out of date. Packages include it and the app
   shows it in its About dialog.
6. **Report only what ran.** State which commands you ran and what you could not test (other
   operating systems, remotes, credentials, SSH). Never claim a test passed that you did not run.
7. **No secrets or personal data** in code, commits, logs, test fixtures or requests to external
   services. Commit with an address you are happy to publish (e.g. your GitHub noreply address).
8. **Stop and ask** before: adding a licence outside the list, publishing or releasing, adding a
   remote, touching secrets, spending money, destructive git on shared branches, deleting user
   data. Otherwise keep working.

## Before you change code

- Read the relevant part of [docs/spec.md](docs/spec.md) and the code you touch. Open a
  definition before using it; do not guess APIs.
- Check open issues and pull requests for the same work.
- Keep a change focused: one feature or fix per pull request, with tests. Do not reformat or
  refactor unrelated code. CI workflow changes go in their own pull request.
- Architectural decisions go into `docs/decisions/ADR-NNN-*.md` (context, decision,
  alternatives, consequences). Do not silently change a decided ADR; write a new one.
- When a request conflicts with `docs/spec.md`, say so before proceeding. When requirements are
  ambiguous, name the ambiguity and take the most conservative reasonable reading.

## Gate: run before every commit

```sh
cargo xtask ci     # licences, cargo fmt --check, clippy -D warnings, all tests
```

UI changes: also render screenshots and look at them (dark and light).

```sh
GITCRAFT_SHOTS=/some/dir cargo test -p gitcraft-ui   # writes PNGs of the render tests
```

## Git rules

The threat model and the per-PR security checklist are in [docs/security.md](docs/security.md);
the layers and internal API in [docs/architecture.md](docs/architecture.md).

- Git is the source of truth. GitCraft runs the installed `git` executable through
  `crates/git` (ADR-002); nothing else spawns git.
- Arguments are passed as a list, never through a shell or string concatenation. Refs, names
  and URLs from the user or the repository go after `--end-of-options`, paths after `--` or
  through `--pathspec-from-file=- --pathspec-file-nul`.
- Every call is non-interactive (`GIT_TERMINAL_PROMPT=0`), uses machine-readable output
  (`--porcelain=v2`, `-z`, explicit formats) and never depends on the user's locale.
- Repositories are untrusted input. Do not let repository configuration run programs on its own
  (e.g. `core.fsmonitor` is disabled for every call). Hooks run only for operations whose Git
  semantics include them (commit, merge, push), never for reads.
- Raw git output never leaves `crates/git`: parse it into the domain types there.
- Long operations run off the UI thread and can be cancelled; the UI shows what is running.
- Destructive operations state the concrete consequence ("discards changes in 3 files"), never a
  generic "Are you sure?".
- Git behaviour is tested against real temporary repositories, not mocks.

## Robustness

- New non-test code does not `unwrap()`, `expect()` or `panic!`; return `Result` and show the
  error in the UI.
- Treat repository content, git output and file names as hostile: bounds-checked access, checked
  arithmetic, capped allocations, any bytes in file names (non-UTF-8, spaces, newlines).
- Every crash fix comes with a test that failed before.

## UI and design language

- Colours come from `crates/ui/src/theme.rs` tokens (`theme::tokens(ctx)`); do not hard-code
  `Color32` values. Check dark and light theme.
- Draw only what is visible (virtualised lists); large repositories must stay responsive.
- Keyboard first: every core action has a shortcut.
- Every interactive widget has an accessible label; render tests run `check_accessibility`.
- UI text is written in German in the code and doubles as the translation key: wrap every
  user-visible text in `crate::i18n::tr("…")` (or `trf("… {name} …", &[("name", &v)])`) and add
  a `German<TAB>English` line to `crates/ui/i18n/en.tsv`; a test fails otherwise.

## Platforms

Linux (x86_64, aarch64, riscv64), Windows, macOS (universal) and FreeBSD. No web build (GitCraft
needs the `git` executable). Do not introduce platform-only dependencies without a `cfg`
fallback. No Tauri, Electron, webviews or handwritten JS/TS.

## Commits and pull requests

- Commit messages in **English**, imperative subject (≤ 72 chars), body explaining what and why.
- One logical change per commit; the tree passes `cargo xtask ci` at every commit.
- Branch names: `feat/…`, `fix/…`, `docs/…`, `ci/…`.
- Fill in the pull request template: what changed, why, how it was tested, what could not be tested.
