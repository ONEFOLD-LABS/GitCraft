# ADR-005: Telemetry policy

Status: accepted, 2026-11-20

## Context

The spec makes privacy a design principle: local first, no account, no mandatory cloud. Usage
data would tell us which features matter, but a Git client sees source code, file names,
remote URLs and commit messages, and users must be able to trust it with private repositories.

## Decision

GitCraft collects no telemetry: no usage statistics, no crash uploads, no identifiers. The only
network access GitCraft starts on its own is the opt-in update check (ADR-004, planned for 0.9), which sends
nothing but the request for the latest version. Crash reports are files on the user's machine
that the user may attach to an issue.

## Alternatives

- **Opt-in telemetry:** still a data flow to secure, document and host; small benefit for a
  community project.
- **Anonymous crash upload:** stack traces can contain paths and repository names.

## Consequences

- Priorities come from issues, discussions and maintainers' own use.
- No backend to run; nothing to disclose in a privacy policy beyond the update check.
