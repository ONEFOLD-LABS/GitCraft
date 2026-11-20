# GitCraft

GitCraft is a cross-platform desktop Git client written in Rust. It is a project of ONEFOLD LABS.
It aims to make Git powerful, understandable, fast and pleasant to use, while keeping Git's own
model visible: every action corresponds to real Git concepts and could be reproduced with the
Git command line. See [`docs/spec.md`](docs/spec.md).

- Local first: no account, no mandatory cloud service, no telemetry.
- Safe by default: destructive operations say exactly what they will do.
- Keyboard first, fast on large repositories.
- Pure Rust with an egui user interface; uses the installed `git`.

> **Status:** just started, nothing to use yet. The first milestone opens a local repository and
> shows its status.

## Platforms

Linux (x86_64, aarch64, riscv64), Windows, macOS and FreeBSD.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Working with an AI agent? Read [AGENTS.md](AGENTS.md).

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
