# ADR-001: UI framework

Status: accepted, 2026-11-20

## Context

GitCraft needs a fast, keyboard-driven desktop UI with dense information (commit graph, diffs,
file lists) on Linux, Windows, macOS and FreeBSD. ONEFOLD LABS projects are pure Rust and exclude
Tauri, Electron, webviews and handwritten JS/TS. StudioCraft, another ONEFOLD LABS project,
already ships an egui/eframe application on all these platforms, with theme tokens, i18n,
accessibility checks and headless render tests.

## Decision

egui/eframe with the wgpu renderer and AccessKit.

## Alternatives

- **Retained-mode native Rust toolkits** (e.g. Slint, Iced, GTK bindings): more native look in
  places, but a second stack to learn and maintain next to StudioCraft, a different accessibility
  story per toolkit, and GTK adds a platform-heavy C dependency.
- **Web-based shells:** excluded by the project rules.

## Consequences

- Immediate-mode rendering: long lists (history, file lists, diffs) must be virtualised
  (`ScrollArea::show_rows` or equivalent) to stay fast on large repositories.
- Text editing widgets are basic; a rich commit-message editor or conflict editor may need custom
  work later.
- Theme, i18n and test patterns are taken over from StudioCraft, so the two apps can later share
  them as a common library.
- Accessibility goes through AccessKit; render tests run `check_accessibility`.
