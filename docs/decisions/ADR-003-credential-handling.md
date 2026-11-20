# ADR-003: Credential handling

Status: accepted, 2026-11-20

## Context

Fetch, pull, push and clone need credentials: HTTPS usernames/tokens, SSH keys and their
passphrases, sometimes confirmation of an unknown SSH host key. Git and SSH already have
mature mechanisms (credential helpers, the platform keychains behind them, ssh-agent). A Git
client that stores secrets itself becomes a target and duplicates them.

## Decision

- GitCraft stores no credentials and has no credential database.
- HTTPS: Git's configured credential helper (e.g. the platform keychain helper) stores and
  returns credentials. If none is configured, GitCraft explains how to set one up.
- SSH: the user's ssh client, keys and agent.
- Interactive input: GitCraft sets `GIT_ASKPASS` and `SSH_ASKPASS` (with
  `SSH_ASKPASS_REQUIRE=force`) to its own executable in helper mode. The helper connects to the
  running window over a local channel (loopback or named pipe, per-session random token, file
  readable by the user only), the window shows the prompt (host, user, kind), and the answer
  goes back to git only. It is never logged, cached or written to disk by GitCraft.
- `GIT_TERMINAL_PROMPT=0` stays set: git never waits on an invisible terminal.
- Credentials in remote URLs are redacted everywhere GitCraft shows or logs a URL.

## Alternatives

- **Own encrypted store:** a new secret store to secure and migrate; duplicates the keychain.
- **libgit2-style credential callbacks:** not available with the git CLI backend (ADR-002).
- **Terminal prompts:** impossible in a GUI without a terminal.

## Consequences

- Behaviour matches the command line: what works in a terminal works in GitCraft.
- The askpass bridge is security-sensitive code (local IPC); it gets its own tests and review.
- Users without a credential helper type HTTPS tokens on every operation until they configure
  one; GitCraft points them to it.
