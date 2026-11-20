//! GitCraft's user interface (egui): the repository window.
//!
//! First milestone (docs/spec.md, "Current Objective"): open a local repository and show where
//! HEAD is, how the branch relates to its upstream, and what is conflicted, staged, changed and
//! untracked. Git runs on worker threads ([`worker`]); the window never waits for it.

pub mod i18n;
pub mod theme;
mod worker;

use std::path::{Path, PathBuf};

use egui::{Align, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use gitcraft_git::{
    Change, Conflict, FileChange, GitError, GitRunner, GitVersion, Head, MIN_VERSION, Repository,
    Status,
};

use crate::i18n::{tr, trf};
use crate::worker::{Job, Outcome, Worker};

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const REFRESH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F5);

/// Colour role of a file row; the colour itself comes from the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Added,
    Modified,
    Deleted,
    Renamed,
    Conflict,
    Untracked,
}

/// One line of the change list. The list is built once per status and drawn virtualised.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Section {
        title: &'static str,
        count: usize,
    },
    File {
        letter: char,
        tone: Tone,
        /// What the letter means, for the tooltip and screen readers.
        meaning: &'static str,
        path: String,
    },
}

/// What is running right now (shown next to the spinner).
#[derive(Debug, Clone, PartialEq)]
enum Busy {
    Opening(PathBuf),
    Refreshing,
}

/// The repository window.
pub struct RepoWindow {
    runner: GitRunner,
    repo: Option<Repository>,
    status: Option<Status>,
    rows: Vec<Row>,
    error: Option<String>,
    busy: Option<Busy>,
    git_version: Option<GitVersion>,
    worker: Worker,
    /// Results of older open/refresh jobs are dropped.
    generation: u64,
    /// Opened at the first frame (command line), when a context exists.
    pending_open: Option<PathBuf>,
    version_requested: bool,
    was_focused: bool,
}

impl RepoWindow {
    /// A window that uses `runner` for git and opens `initial` at the first frame.
    pub fn new(runner: GitRunner, initial: Option<PathBuf>) -> Self {
        RepoWindow {
            runner,
            repo: None,
            status: None,
            rows: Vec::new(),
            error: None,
            busy: None,
            git_version: None,
            worker: Worker::new(),
            generation: 0,
            pending_open: initial,
            version_requested: false,
            was_focused: true,
        }
    }

    /// Opens the repository that contains `path` (in the background).
    pub fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.start(Job::Open(path.clone()), Busy::Opening(path), ctx);
    }

    /// Reads the status of the open repository again (in the background).
    pub fn refresh(&mut self, ctx: &egui::Context) {
        if let Some(repo) = self.repo.clone() {
            self.start(Job::Refresh(repo), Busy::Refreshing, ctx);
        }
    }

    /// True while git is working for the window.
    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// The open repository.
    pub fn repository(&self) -> Option<&Repository> {
        self.repo.as_ref()
    }

    /// The last error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn start(&mut self, job: Job, busy: Busy, ctx: &egui::Context) {
        self.generation = self.generation.wrapping_add(1);
        match self
            .worker
            .start(self.generation, job, self.runner.clone(), ctx.clone())
        {
            Ok(()) => self.busy = Some(busy),
            Err(e) => {
                self.busy = None;
                self.error = Some(trf(
                    "Git konnte nicht gestartet werden: {fehler}",
                    &[("fehler", &e)],
                ));
            }
        }
    }

    /// Shows `status` of `repo` (also used by tests that read the status synchronously).
    fn show_status(&mut self, repo: Repository, status: Status) {
        self.rows = rows(&status);
        self.repo = Some(repo);
        self.status = Some(status);
        self.error = None;
    }

    fn collect(&mut self) {
        for (generation, outcome) in self.worker.poll() {
            if let Outcome::Version(v) = outcome {
                self.git_version = Some(v);
                continue;
            }
            if generation != self.generation {
                continue;
            }
            self.busy = None;
            match outcome {
                Outcome::Opened(opened) => {
                    let (repo, status) = *opened;
                    self.show_status(repo, status);
                }
                Outcome::Refreshed(status) => {
                    self.rows = rows(&status);
                    self.status = Some(status);
                    self.error = None;
                }
                Outcome::Failed(e) => self.error = Some(error_text(&e)),
                Outcome::Version(_) => {}
            }
        }
    }

    /// Draws the window and handles its input.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.version_requested {
            self.version_requested = true;
            // Failures show up when a repository is opened; the footer just stays empty.
            let _ = self
                .worker
                .start(u64::MAX, Job::Version, self.runner.clone(), ctx.clone());
        }
        if let Some(path) = self.pending_open.take() {
            self.open(path, &ctx);
        }
        self.collect();
        self.handle_input(&ctx);

        egui::Panel::top("werkzeugzeile").show(ui, |ui| self.toolbar(ui));
        if self.error.is_some() || self.repo.is_some() {
            egui::Panel::top("kopf").show(ui, |ui| self.header(ui));
        }
        egui::Panel::bottom("statuszeile")
            .exact_size(22.0)
            .show(ui, |ui| self.status_line(ui));
        egui::CentralPanel::default().show(ui, |ui| self.changes(ui));
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let (open, refresh, focused, dropped) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&OPEN),
                i.consume_shortcut(&REFRESH),
                i.focused,
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .next(),
            )
        });
        if open {
            self.pick_folder(ctx);
        }
        if let Some(path) = dropped {
            self.open(path, ctx);
        } else if refresh || (focused && !self.was_focused && !self.is_busy()) {
            // Back in the window: files may have changed outside.
            self.refresh(ctx);
        }
        self.was_focused = focused;
    }

    fn pick_folder(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title(tr("Repository öffnen"));
        if let Some(repo) = &self.repo {
            dialog = dialog.set_directory(repo.workdir());
        }
        if let Some(path) = dialog.pick_folder() {
            self.open(path, ctx);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            let open = ui.button(tr("Öffnen …")).on_hover_text(shortcut_hint(
                &ctx,
                tr("Repository öffnen"),
                &OPEN,
            ));
            if open.clicked() {
                self.pick_folder(&ctx);
            }
            let refresh = ui
                .add_enabled(
                    self.repo.is_some() && !self.is_busy(),
                    egui::Button::new(tr("Aktualisieren")),
                )
                .on_hover_text(shortcut_hint(&ctx, tr("Status neu lesen"), &REFRESH));
            if refresh.clicked() {
                self.refresh(&ctx);
            }
            if let Some(busy) = &self.busy {
                ui.add(egui::Spinner::new());
                let text = match busy {
                    Busy::Opening(p) => trf("Öffne {pfad} …", &[("pfad", &p.display())]),
                    Busy::Refreshing => tr("Lese Status …").to_owned(),
                };
                ui.label(RichText::new(text).color(theme::tokens(&ctx).text_dim));
            }
        });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        if let Some(error) = &self.error {
            egui::Frame::new()
                .fill(t.error_bg)
                .inner_margin(8)
                .corner_radius(4)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(error).color(t.error_text));
                });
        }
        let (Some(repo), Some(status)) = (&self.repo, &self.status) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(repo_name(repo.workdir()))
                    .color(t.text_strong)
                    .size(16.0),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(repo.workdir().display().to_string()).color(t.text_dim),
                )
                .truncate(),
            );
        });
        ui.horizontal(|ui| {
            ui.label(head_text(&status.head));
            ui.label(RichText::new("·").color(t.text_dim));
            ui.label(RichText::new(upstream_text(status)).color(t.text_dim));
        });
        ui.add_space(2.0);
    }

    fn status_line(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            if let Some(status) = &self.status {
                let text = trf(
                    "Konflikte {konflikte} · Vorgemerkt {vorgemerkt} · Geändert {geaendert} · Unversioniert {neu}",
                    &[
                        ("konflikte", &status.conflicts().count()),
                        ("vorgemerkt", &status.staged().count()),
                        ("geaendert", &status.unstaged().count()),
                        ("neu", &status.untracked().count()),
                    ],
                );
                ui.label(RichText::new(text).color(t.text_dim));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(v) = self.git_version {
                    ui.label(RichText::new(format!("git {v}")).color(t.text_dim));
                }
            });
        });
    }

    fn changes(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = theme::tokens(&ctx);
        let Some(status) = &self.status else {
            if self.busy.is_none() {
                self.welcome(ui);
            }
            return;
        };
        if status.is_clean() {
            let unborn = matches!(status.head, Head::Branch { commit: None, .. });
            let (title, text) = if unborn {
                (
                    tr("Leeres Repository"),
                    tr("Noch keine Commits und keine Dateien."),
                )
            } else {
                (
                    tr("Keine Änderungen"),
                    tr("Arbeitsverzeichnis und Index entsprechen dem letzten Commit."),
                )
            };
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(title).color(t.text_strong).size(16.0));
                ui.label(RichText::new(text).color(t.text_dim));
            });
            return;
        }
        let row_h = ui.text_style_height(&egui::TextStyle::Body).max(18.0);
        let rows = &self.rows;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_h, rows.len(), |ui, range| {
                for (i, row) in rows.iter().enumerate().skip(range.start).take(range.len()) {
                    let size = egui::vec2(ui.available_width(), row_h);
                    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
                        // Stripes on file rows only, so section titles stand apart.
                        if i % 2 == 1 && matches!(row, Row::File { .. }) {
                            ui.painter().rect_filled(ui.max_rect(), 0.0, t.row_alt);
                        }
                        draw_row(ui, row, t);
                    });
                }
            });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(tr("Kein Repository geöffnet"))
                    .color(t.text_strong)
                    .size(18.0),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(tr(
                    "Öffne einen Ordner mit einem Git-Repository oder ziehe ihn in dieses Fenster.",
                ))
                .color(t.text_dim),
            );
            ui.add_space(12.0);
            if ui.button(tr("Repository öffnen …")).clicked() {
                let ctx = ui.ctx().clone();
                self.pick_folder(&ctx);
            }
        });
    }
}

fn draw_row(ui: &mut egui::Ui, row: &Row, t: &theme::Tokens) {
    match row {
        Row::Section { title, count } => {
            ui.label(RichText::new(tr(title)).color(t.text_strong));
            ui.label(RichText::new(count.to_string()).color(t.text_dim));
        }
        Row::File {
            letter,
            tone,
            meaning,
            path,
        } => {
            let colour = match tone {
                Tone::Added => t.added,
                Tone::Modified => t.modified,
                Tone::Deleted => t.deleted,
                Tone::Renamed => t.renamed,
                Tone::Conflict => t.conflict,
                Tone::Untracked => t.untracked,
            };
            ui.add_space(12.0);
            ui.label(RichText::new(letter.to_string()).monospace().color(colour))
                .on_hover_text(tr(meaning));
            ui.add(egui::Label::new(path.as_str()).truncate())
                .on_hover_text(tr(meaning));
        }
    }
}

fn shortcut_hint(ctx: &egui::Context, text: &str, shortcut: &KeyboardShortcut) -> String {
    format!("{text} ({})", ctx.format_shortcut(shortcut))
}

/// The folder name of the working tree.
fn repo_name(workdir: &Path) -> String {
    workdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| workdir.display().to_string())
}

fn head_text(head: &Head) -> String {
    match head {
        Head::Branch {
            name,
            commit: Some(_),
        } => trf("Branch {name}", &[("name", name)]),
        Head::Branch { name, commit: None } => {
            trf("Branch {name} (noch keine Commits)", &[("name", name)])
        }
        Head::Detached { commit } => trf(
            "Losgelöster HEAD bei {commit}",
            &[("commit", &commit.short())],
        ),
    }
}

fn upstream_text(status: &Status) -> String {
    let Some(up) = &status.upstream else {
        return tr("kein Upstream").to_owned();
    };
    match up.ahead_behind {
        None => trf(
            "Upstream {upstream} existiert nicht mehr",
            &[("upstream", &up.name)],
        ),
        Some((0, 0)) => trf("auf dem Stand von {upstream}", &[("upstream", &up.name)]),
        Some((ahead, behind)) => trf(
            "{voraus} voraus, {zurueck} zurück gegenüber {upstream}",
            &[
                ("voraus", &ahead),
                ("zurueck", &behind),
                ("upstream", &up.name),
            ],
        ),
    }
}

fn change_row(file: &FileChange, change: Change) -> Row {
    let (tone, meaning) = match change {
        Change::Added => (Tone::Added, "hinzugefügt"),
        Change::Modified => (Tone::Modified, "geändert"),
        Change::TypeChanged => (Tone::Modified, "Dateityp geändert"),
        Change::Deleted => (Tone::Deleted, "gelöscht"),
        Change::Renamed => (Tone::Renamed, "umbenannt"),
        Change::Copied => (Tone::Renamed, "kopiert"),
    };
    let path = match (&file.orig_path, change) {
        (Some(orig), Change::Renamed | Change::Copied) => {
            format!("{} -> {}", orig.display(), file.path.display())
        }
        _ => file.path.display(),
    };
    Row::File {
        letter: change.letter(),
        tone,
        meaning,
        path,
    }
}

fn conflict_meaning(c: Conflict) -> &'static str {
    match c {
        Conflict::BothModified => "Konflikt: beide geändert",
        Conflict::BothAdded => "Konflikt: beide hinzugefügt",
        Conflict::BothDeleted => "Konflikt: beide gelöscht",
        Conflict::AddedByUs => "Konflikt: von uns hinzugefügt",
        Conflict::AddedByThem => "Konflikt: von ihnen hinzugefügt",
        Conflict::DeletedByUs => "Konflikt: von uns gelöscht",
        Conflict::DeletedByThem => "Konflikt: von ihnen gelöscht",
    }
}

/// The change list: conflicts first (they block everything else), then staged, unstaged and
/// untracked; empty sections are left out.
fn rows(status: &Status) -> Vec<Row> {
    let mut out = Vec::new();
    let mut section = |title: &'static str, items: Vec<Row>| {
        if !items.is_empty() {
            out.push(Row::Section {
                title,
                count: items.len(),
            });
            out.extend(items);
        }
    };
    section(
        "Konflikte",
        status
            .conflicts()
            .map(|(f, c)| Row::File {
                letter: 'U',
                tone: Tone::Conflict,
                meaning: conflict_meaning(c),
                path: f.path.display(),
            })
            .collect(),
    );
    section(
        "Vorgemerkt",
        status.staged().map(|(f, c)| change_row(f, c)).collect(),
    );
    section(
        "Nicht vorgemerkt",
        status.unstaged().map(|(f, c)| change_row(f, c)).collect(),
    );
    section(
        "Unversioniert",
        status
            .untracked()
            .map(|f| Row::File {
                letter: '?',
                tone: Tone::Untracked,
                meaning: "unversioniert",
                path: f.path.display(),
            })
            .collect(),
    );
    out
}

/// The message for a git error, in the UI language.
fn error_text(e: &GitError) -> String {
    match e {
        GitError::NotInstalled => trf(
            "Git wurde nicht gefunden. Installiere Git {min} oder neuer und starte GitCraft neu.",
            &[("min", &MIN_VERSION)],
        ),
        GitError::TooOld(v) => trf(
            "Git {version} ist zu alt; GitCraft braucht {min} oder neuer.",
            &[("version", v), ("min", &MIN_VERSION)],
        ),
        GitError::NotARepository(p) => {
            trf("{pfad} ist kein Git-Repository.", &[("pfad", &p.display())])
        }
        GitError::Bare(p) => trf(
            "{pfad} ist ein Bare-Repository ohne Arbeitsverzeichnis; das wird noch nicht unterstützt.",
            &[("pfad", &p.display())],
        ),
        GitError::DubiousOwnership(p) => trf(
            "Git verweigert {pfad}, weil der Ordner einem anderen Benutzer gehört. Wenn du ihm vertraust: git config --global --add safe.directory {pfad}",
            &[("pfad", &p.display())],
        ),
        GitError::Failed {
            command, stderr, ..
        } => trf(
            "git {befehl} ist fehlgeschlagen: {meldung}",
            &[("befehl", command), ("meldung", stderr)],
        ),
        GitError::OutputTooLarge { command, limit } => trf(
            "git {befehl} lieferte zu viel Ausgabe (über {mib} MiB).",
            &[("befehl", command), ("mib", &(limit / (1024 * 1024)))],
        ),
        GitError::Spawn(err) => trf(
            "Git konnte nicht gestartet werden: {fehler}",
            &[("fehler", err)],
        ),
        GitError::Parse(err) => trf("Unerwartete Ausgabe von git: {fehler}", &[("fehler", err)]),
    }
}

#[cfg(test)]
mod tests;
