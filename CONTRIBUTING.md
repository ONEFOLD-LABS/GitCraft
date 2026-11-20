# Contributing to GitCraft

Thanks for helping. GitCraft is a clean-room Rust Git client described in `docs/spec.md`.
Contributions of code, tests, documentation, translations, bug reports and Git edge cases that
GitCraft handles badly are all welcome.

Working with an AI agent? Read [AGENTS.md](AGENTS.md) too; its rules apply to everyone.

## Ground rules

- **Clean room.** Never copy code, icons, images or text from other Git clients or other
  products, and do not name other proprietary products anywhere in the project. Git itself is
  GPL-licensed: learn from its documentation and behaviour, never from its source code.
- **Licence.** By contributing you agree that your work is licensed under MIT OR Apache-2.0, like
  the rest of the project. New dependencies and assets must be permissively licensed (see AGENTS.md).
- **`main` is protected.** Every change goes through a pull request and needs green CI.
- **Be kind.** Assume good intent, review the code, not the person.

## Getting started

Requirements: Rust (stable, edition 2024), Git 2.30 or newer, and on Linux the development
packages for X11/Wayland and fontconfig:

```sh
# Debian/Ubuntu
sudo apt install pkg-config libxkbcommon-dev libwayland-dev libx11-dev libxcb1-dev \
  libxcursor-dev libxrandr-dev libxi-dev libfontconfig1-dev libfreetype-dev libgl1-mesa-dev
# Arch
sudo pacman -S git libxkbcommon wayland libx11 fontconfig
```

```sh
cargo run --release -p gitcraft -- /path/to/repo   # the app
cargo xtask ci                                     # licences, fmt, clippy -D warnings, tests
```

## Workflow

1. Look for an issue or open one. For larger features, describe the planned behaviour first
   (ideally with a reference to `docs/spec.md`) and wait for a maintainer's go-ahead.
2. Fork and create a branch: `feat/…`, `fix/…`, `docs/…`, `ci/…`.
3. Make focused commits with English messages (imperative subject, body with the why).
4. Run `cargo xtask ci`. For UI changes, render screenshots
   (`GITCRAFT_SHOTS=dir cargo test -p gitcraft-ui`) and attach before/after images to the PR.
5. Open a pull request and fill in the template. Mention what you could not test.
6. Address review feedback with new commits; the maintainer squashes or merges.

## Reporting bugs

Use the bug report template. Include the version (`gitcraft --version`), `git --version`,
operating system and, if possible, the commands that build a repository showing the problem.

## Security

Please report vulnerabilities (e.g. a crafted repository that makes GitCraft run code or crash)
privately through GitHub's "Report a vulnerability" in the Security tab, not in a public issue.
