//! GitCraft's user interface (egui): the repository window.
//!
//! Layout and operation follow docs/ux/layout.md: toolbar on top, sidebar with refs on the
//! left, the history graph in the centre, details of the selection on the right, status line at
//! the bottom. Git runs in the repository's session ([`gitcraft_core::RepoSession`]) and on the
//! history and search threads; the window never waits for it.

mod avatars;
mod commit;
mod diffview;
mod discard;
mod history;
pub mod i18n;
mod search;
pub mod settings;
mod sidebar;
pub mod theme;
mod toolbar;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use gitcraft_core::{Command, DiffKey, Event, Outcome, RepoSession, Snapshot, Task, Wake};
use gitcraft_git::{
    Change, ChangedFile, Conflict, Discarded, FileChange, FileStatus, GitError, GitRunner,
    GitVersion, Head, MIN_VERSION, Oid, RepoPath, Status, escape,
};

use crate::avatars::Avatars;
use crate::commit::{CommitForm, FormAction};
use crate::diffview::{DiffAction, Prepared, Side};
use crate::discard::{Confirm, Pending};
use crate::history::{Action, History, Labels, Selection, View, WipCounts};
use crate::i18n::{tr, trf};
use crate::search::{BarAction, Search};
use crate::settings::{Loaded, Settings};
use crate::sidebar::{Pick, Sidebar};

/// The ONEFOLD LABS donation page, linked from the help window (F1).
const SPONSOR_URL: &str = "https://ko-fi.com/onefoldlabs";

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const REFRESH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
const COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::C);
const FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
const NEXT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F3);
const PREVIOUS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F3);
const KEYS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F1);

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

/// What staging does with a file row or a whole section.
#[derive(Debug, Clone, PartialEq)]
enum StageOp {
    Stage(Vec<RepoPath>),
    Unstage(Vec<RepoPath>),
}

impl StageOp {
    fn label(&self) -> &'static str {
        match self {
            StageOp::Stage(_) => "Vormerken",
            StageOp::Unstage(_) => "Zurücknehmen",
        }
    }

    fn all_label(&self) -> &'static str {
        match self {
            StageOp::Stage(_) => "Alle vormerken",
            StageOp::Unstage(_) => "Alle zurücknehmen",
        }
    }

    fn command(self) -> Command {
        match self {
            StageOp::Stage(p) => Command::Stage(p),
            StageOp::Unstage(p) => Command::Unstage(p),
        }
    }
}

/// One line of the change list of the work in progress.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Section {
        title: &'static str,
        count: usize,
        /// "Stage all" or "unstage all" for the section.
        all: Option<StageOp>,
    },
    File {
        letter: char,
        tone: Tone,
        /// What the letter means, for the tooltip and screen readers.
        meaning: &'static str,
        path: String,
        /// The diff a click opens (none for conflicts until the conflict view exists).
        diff: Option<DiffKey>,
        /// What a double click or the row's button does (none for conflicts).
        stage: Option<StageOp>,
        /// Discarding (tracked) or deleting (untracked) from the context menu.
        discard: Option<Pending>,
    },
}

/// Message and files of the selected commit (loading while `None`).
struct Details {
    id: Oid,
    message: Option<String>,
    files: Option<Vec<ChangedFile>>,
    error: Option<String>,
}

/// The diff shown in the centre.
struct DiffState {
    key: DiffKey,
    title: String,
    prepared: Option<Prepared>,
    error: Option<String>,
}

/// A repository being opened in the background.
type Opened = Result<(RepoSession, Arc<Snapshot>), GitError>;

/// The repository window.
pub struct RepoWindow {
    runner: GitRunner,
    session: Option<RepoSession>,
    snapshot: Option<Arc<Snapshot>>,
    rows: Vec<Row>,
    labels: Labels,
    history: History,
    selection: Selection,
    details: Option<Details>,
    diff: Option<DiffState>,
    search: Search,
    sidebar: Sidebar,
    settings: Settings,
    /// Where the settings are saved; `None` keeps them in memory (tests).
    settings_dir: Option<PathBuf>,
    show_settings: bool,
    show_keys: bool,
    avatars: Avatars,
    error: Option<String>,
    /// The commit form of the work in progress.
    commit: CommitForm,
    /// Commit messages not committed yet, per repository (working tree).
    drafts: std::collections::HashMap<PathBuf, (String, String)>,
    /// A destructive command waiting for confirmation.
    confirm: Option<Confirm>,
    /// What the last discard, delete or restore did, with the saved state that undoes it.
    notice: Option<(String, Option<String>)>,
    /// Saved states of discarded and deleted files, newest first.
    discarded: Vec<Discarded>,
    show_discarded: bool,
    /// The error shown came from a read; the next good snapshot clears it.
    read_error: bool,
    /// A write waiting for the user to trust the repository (the trust dialog is open).
    trust_prompt: Option<Command>,
    /// The user chose to keep this repository read-only (no more asking until it is reopened).
    read_only: bool,
    /// The repository being opened; a newer open replaces it (the older result is dropped).
    opening: Option<(PathBuf, Task<Opened>)>,
    git_version: Option<GitVersion>,
    version: Option<Task<Result<GitVersion, GitError>>>,
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
            session: None,
            snapshot: None,
            rows: Vec::new(),
            labels: Labels::default(),
            history: History::empty(),
            selection: Selection::None,
            details: None,
            diff: None,
            search: Search::default(),
            sidebar: Sidebar::default(),
            settings: Settings::default(),
            settings_dir: None,
            show_settings: false,
            show_keys: false,
            avatars: Avatars::off(),
            error: None,
            commit: CommitForm::default(),
            drafts: std::collections::HashMap::new(),
            read_error: false,
            confirm: None,
            notice: None,
            discarded: Vec::new(),
            show_discarded: false,
            trust_prompt: None,
            read_only: false,
            opening: None,
            git_version: None,
            version: None,
            pending_open: initial,
            version_requested: false,
            was_focused: true,
        }
    }

    /// Loads the settings from `dir` and saves changes there.
    pub fn use_settings(&mut self, dir: Option<PathBuf>) {
        if let Some(dir) = &dir {
            let (settings, loaded) = Settings::load(dir);
            if let Loaded::Broken(e) = loaded {
                self.error = Some(trf(
                    "Die Einstellungen waren nicht lesbar und wurden zurückgesetzt ({fehler}); die alte Datei liegt als settings.json.bak daneben.",
                    &[("fehler", &escape(&e))],
                ));
            }
            self.settings = settings;
        }
        self.settings_dir = dir;
        self.apply_settings();
    }

    fn apply_settings(&mut self) {
        self.avatars = if self.settings.avatars {
            Avatars::on(settings::cache_dir().map(|d| d.join("avatars")))
        } else {
            Avatars::off()
        };
    }

    fn save_settings(&mut self) {
        if let Some(dir) = &self.settings_dir
            && let Err(e) = self.settings.save(dir)
        {
            self.error = Some(trf(
                "Die Einstellungen konnten nicht gespeichert werden: {fehler}",
                &[("fehler", &escape(&e))],
            ));
        }
    }

    /// Opens the repository that contains `path` (in the background).
    pub fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let runner = self.runner.clone();
        let wake = wake(ctx);
        let target = path.clone();
        let task = Task::spawn("gitcraft-open", wake.clone(), move || {
            RepoSession::open(runner, &target, wake)
        });
        match task {
            Ok(task) => self.opening = Some((path, task)),
            Err(e) => self.error = Some(error_text(&e)),
        }
    }

    /// Reads the state of the open repository again (in the background).
    pub fn refresh(&mut self) {
        if let Some(session) = &self.session {
            session.submit(Command::Refresh);
        }
    }

    /// Runs a write in the open repository; asks first when its program settings are not
    /// trusted yet (docs/security.md T1).
    /// Returns whether the write was submitted right away.
    fn write(&mut self, command: Command) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        if session.is_trusted() {
            session.submit(command);
            return true;
        }
        if self.read_only {
            self.error = Some(error_text(&GitError::NotTrusted));
        } else {
            self.trust_prompt = Some(command);
        }
        false
    }

    /// The user trusts the open repository: remembered in the settings, the waiting write runs.
    fn trust(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        self.settings
            .trust(session.workdir(), session.git_dir(), session.trust_report());
        session.set_trusted(true);
        if let Some(command) = self.trust_prompt.take() {
            session.submit(command);
        }
        self.save_settings();
    }

    /// True while git is working for the window (the history may still load afterwards).
    pub fn is_busy(&self) -> bool {
        self.opening.is_some() || self.session.as_ref().is_some_and(RepoSession::busy)
    }

    /// Top level of the open repository's working tree.
    pub fn workdir(&self) -> Option<&Path> {
        self.session.as_ref().map(RepoSession::workdir)
    }

    /// The last error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn status(&self) -> Option<&Status> {
        self.snapshot.as_ref().map(|s| &s.status)
    }

    /// Shows a freshly opened repository; the history starts loading.
    fn show_opened(&mut self, session: RepoSession, snapshot: Arc<Snapshot>, ctx: &egui::Context) {
        self.history = History::start(&session.reader(), ctx);
        self.trust_prompt = None;
        self.read_only = false;
        self.error = None;
        self.read_error = false;
        // Keep the draft of the repository shown so far; pick up this one's.
        if let Some(old) = &self.session {
            let draft = (
                std::mem::take(&mut self.commit.summary),
                std::mem::take(&mut self.commit.body),
            );
            self.drafts.insert(old.workdir().to_path_buf(), draft);
        }
        self.commit = CommitForm::default();
        if let Some((summary, body)) = self.drafts.remove(session.workdir()) {
            self.commit.summary = summary;
            self.commit.body = body;
        }
        session.submit(Command::Identity);
        self.confirm = None;
        self.notice = None;
        self.discarded.clear();
        if session.is_trusted() {
            let keep = i64::from(self.settings.discard_days).saturating_mul(24 * 3600);
            session.submit(Command::PruneDiscarded(keep));
        }
        session.submit(Command::ListDiscarded);
        if !session.is_trusted()
            && self.settings.is_trusted(
                session.workdir(),
                session.git_dir(),
                session.trust_report(),
            )
        {
            session.set_trusted(true);
        }
        self.session = Some(session);
        self.selection = Selection::None;
        self.details = None;
        self.diff = None;
        self.search.close();
        self.apply(snapshot);
    }

    /// Takes a new snapshot; restarts the history when branches, tags or HEAD moved.
    fn refreshed(&mut self, snapshot: Arc<Snapshot>, ctx: &egui::Context) {
        let moved = self.snapshot.as_ref().is_none_or(|old| {
            old.refs != snapshot.refs || head_commit(&old.status) != head_commit(&snapshot.status)
        });
        if moved && let Some(session) = &self.session {
            self.history = History::start(&session.reader(), ctx);
        }
        self.apply(snapshot);
        // A diff of the working tree may have changed with it.
        if let Some(state) = &self.diff
            && !matches!(state.key, DiffKey::Commit { .. })
        {
            let (key, title) = (state.key.clone(), state.title.clone());
            self.open_diff(key, title);
        }
    }

    fn apply(&mut self, snapshot: Arc<Snapshot>) {
        self.rows = rows(&snapshot.status);
        self.labels = Labels::new(&snapshot.refs);
        let dirty = !snapshot.status.is_clean();
        if self.selection == Selection::WorkInProgress && !dirty {
            self.selection = Selection::None;
        }
        if self.selection == Selection::None && dirty {
            self.selection = Selection::WorkInProgress;
        }
        self.snapshot = Some(snapshot);
        // A successful read ends an error of an earlier read, not that of a write (the refresh
        // after a failed write must not hide why it failed).
        if self.read_error {
            self.read_error = false;
            self.error = None;
        }
    }

    fn collect(&mut self, ctx: &egui::Context) {
        if let Some(v) = self.version.as_mut().and_then(Task::poll) {
            // Failures show up when a repository is opened; the footer just stays empty.
            self.git_version = v.ok();
        }
        if let Some(opened) = self.opening.as_mut().and_then(|(_, t)| t.poll()) {
            self.opening = None;
            match opened {
                Ok((session, snapshot)) => self.show_opened(session, snapshot, ctx),
                Err(e) => self.error = Some(error_text(&e)),
            }
        }
        let events = self
            .session
            .as_ref()
            .map(RepoSession::poll)
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Finished {
                    command, result, ..
                } => self.finished(command, result, ctx),
            }
        }
        self.history.poll();
        self.search.poll();
        if self.search.jump_pending && self.first_match().is_some() {
            self.search.jump_pending = false;
            self.jump_to(self.first_match());
        }
        // Nothing selected yet: the commit at HEAD once it has loaded.
        if self.selection == Selection::None
            && let Some(head) = self.status().and_then(head_commit)
            && self.history.position(&head).is_some()
        {
            self.selection = Selection::Commit(head);
        }
    }

    /// Takes the result of a command.
    fn finished(
        &mut self,
        command: Command,
        result: Result<Outcome, GitError>,
        ctx: &egui::Context,
    ) {
        match (command, result) {
            (Command::Refresh, Ok(Outcome::Refreshed(snapshot))) => self.refreshed(snapshot, ctx),
            (Command::CommitDetails(commit), result) => {
                if let Some(d) = self.details.as_mut().filter(|d| d.id == commit.id) {
                    match result {
                        Ok(Outcome::CommitDetails { message, files }) => {
                            d.message = Some(message);
                            d.files = Some(files);
                        }
                        Ok(_) => {}
                        Err(e) => d.error = Some(error_text(&e)),
                    }
                }
            }
            (Command::Diff(key), result) => {
                if let Some(s) = self.diff.as_mut().filter(|s| s.key == key) {
                    match result {
                        Ok(Outcome::Diff(diff)) => {
                            s.prepared = Some(Prepared::new(s.title.clone(), &diff));
                        }
                        Ok(_) => {}
                        Err(e) => s.error = Some(error_text(&e)),
                    }
                }
            }
            (Command::Commit { .. }, Ok(Outcome::Written(out))) => {
                self.commit.committed(&out.stderr);
                self.error = None;
            }
            (Command::Commit { .. }, Err(e)) => {
                self.commit.failed();
                self.error = Some(error_text(&e));
            }
            (Command::Identity, Ok(Outcome::Identity(identity))) => {
                self.commit.set_identity(identity);
            }
            (Command::SetIdentity { .. }, Ok(_)) => {
                self.error = None;
                if let Some(session) = &self.session {
                    session.submit(Command::Identity);
                }
            }
            (Command::AmendInfo, Ok(Outcome::AmendInfo(info))) => self.commit.set_amend_info(info),
            (Command::CountLines(_), Ok(Outcome::Lines(n))) => {
                if let Some(c) = self.confirm.as_mut() {
                    c.lines = Some(n);
                }
            }
            (Command::ListDiscarded, Ok(Outcome::Discarded(list))) => self.discarded = list,
            (command, Ok(Outcome::Saved(id))) => {
                let text = match &command {
                    Command::Discard(p) if p.len() == 1 => tr("1 Datei verworfen.").to_owned(),
                    Command::Discard(p) => trf("{n} Dateien verworfen.", &[("n", &p.len())]),
                    Command::DiscardLines { .. } => tr("Zeilen verworfen.").to_owned(),
                    Command::DeleteUntracked(p) if p.len() == 1 => {
                        tr("1 Datei gelöscht.").to_owned()
                    }
                    Command::DeleteUntracked(p) => trf("{n} Dateien gelöscht.", &[("n", &p.len())]),
                    _ => tr("Wiederhergestellt.").to_owned(),
                };
                self.notice = Some((text, Some(id)));
                self.error = None;
                if let Some(session) = &self.session {
                    session.submit(Command::ListDiscarded);
                }
            }
            (Command::PruneDiscarded(_), Ok(_)) => {
                if let Some(session) = &self.session {
                    session.submit(Command::ListDiscarded);
                }
            }
            (Command::Refresh, Err(e)) => {
                self.error = Some(error_text(&e));
                self.read_error = true;
            }
            (_, Err(e)) => self.error = Some(error_text(&e)),
            (_, Ok(Outcome::Written(_))) => self.error = None,
            (_, Ok(_)) => {}
        }
    }

    fn wip(&self) -> Option<WipCounts> {
        let status = self.status()?;
        (!status.is_clean()).then(|| WipCounts {
            changed: status.unstaged().count() + status.untracked().count(),
            staged: status.staged().count(),
            conflicts: status.conflicts().count(),
        })
    }

    /// Draws the window and handles its input.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.version_requested {
            self.version_requested = true;
            let runner = self.runner.clone();
            self.version =
                Task::spawn("gitcraft-version", wake(&ctx), move || runner.version()).ok();
        }
        if let Some(path) = self.pending_open.take() {
            self.open(path, &ctx);
        }
        self.collect(&ctx);
        self.avatars.poll(&ctx);
        self.handle_input(&ctx);
        let t = theme::tokens(&ctx);
        self.settings_window(&ctx);
        self.keys_window(&ctx);
        self.trust_dialog(&ctx);
        self.confirm_dialog(&ctx);
        if self.show_discarded {
            let mut open = true;
            let restore = discard::discarded_window(
                &ctx,
                &mut open,
                t,
                &self.discarded,
                self.settings.discard_days,
            );
            self.show_discarded = open;
            if let Some(id) = restore {
                self.write(Command::RestoreDiscarded(id));
            }
        }

        egui::Panel::top("werkzeugzeile").show(ui, |ui| self.toolbar(ui));
        if let Some(error) = &self.error {
            egui::Panel::top("fehler").show(ui, |ui| {
                egui::Frame::new()
                    .fill(t.error_bg)
                    .inner_margin(8)
                    .corner_radius(4)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(error).color(t.error_text));
                    });
            });
        }
        let mut undo = None;
        let mut close_notice = false;
        if let Some((text, saved)) = &self.notice {
            egui::Panel::top("hinweis").show(ui, |ui| {
                egui::Frame::new()
                    .fill(t.toolbar_bg)
                    .inner_margin(6)
                    .corner_radius(4)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(text).color(t.text_strong));
                            // Not "Rückgängig": that is the toolbar's undo.
                            if let Some(id) = saved
                                && ui.button(tr("Wiederherstellen")).clicked()
                            {
                                undo = Some(id.clone());
                            }
                            if ui.small_button(tr("Schließen")).clicked() {
                                close_notice = true;
                            }
                        });
                    });
            });
        }
        if close_notice {
            self.notice = None;
        }
        if let Some(id) = undo {
            self.notice = None;
            self.write(Command::RestoreDiscarded(id));
        }
        egui::Panel::bottom("statuszeile")
            .exact_size(22.0)
            .show(ui, |ui| self.status_line(ui));
        if self.session.is_none() {
            egui::CentralPanel::default().show(ui, |ui| {
                if self.opening.is_none() {
                    self.welcome(ui);
                }
            });
            return;
        }
        egui::Panel::left("seitenleiste")
            .resizable(true)
            .default_size(230.0)
            .min_size(150.0)
            .show(ui, |ui| {
                let (refs, stashes) = self
                    .snapshot
                    .as_ref()
                    .map(|s| (s.refs.as_slice(), s.stashes.as_slice()))
                    .unwrap_or_default();
                let wip = self.wip().map(|w| w.changed + w.staged + w.conflicts);
                let history = &self.history;
                let lane = |id: &Oid| history.color_of(id).map(|c| t.lanes[c % t.lanes.len()]);
                match self.sidebar.show(ui, t, refs, stashes, wip, &lane) {
                    Some(Pick::Commit(id)) => self.select_commit(id),
                    Some(Pick::WorkInProgress) => {
                        self.diff = None;
                        self.selection = Selection::WorkInProgress;
                        self.history.scroll_to(0);
                    }
                    None => {}
                }
            });
        egui::Panel::right("details")
            .resizable(true)
            .default_size(340.0)
            .min_size(220.0)
            .show(ui, |ui| self.details(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            if self.diff.is_some() {
                self.diff_view(ui);
            } else {
                self.history_view(ui);
            }
        });
        self.ensure_details();
    }

    /// Starts loading message and files of the selected commit when they are not there yet.
    fn ensure_details(&mut self) {
        let Selection::Commit(id) = &self.selection else {
            return;
        };
        if self.details.as_ref().is_some_and(|d| &d.id == id) {
            return;
        }
        let (Some(session), Some(commit)) = (&self.session, self.history.commit(id).cloned())
        else {
            return;
        };
        session.submit(Command::CommitDetails(Box::new(commit)));
        self.details = Some(Details {
            id: id.clone(),
            message: None,
            files: None,
            error: None,
        });
    }

    /// Shows the diff `key` in the centre (loading in the background).
    fn open_diff(&mut self, key: DiffKey, title: String) {
        let Some(session) = &self.session else {
            return;
        };
        session.submit(Command::Diff(key.clone()));
        let keep = self
            .diff
            .take()
            .filter(|s| s.key == key)
            .and_then(|s| s.prepared);
        self.diff = Some(DiffState {
            key,
            title,
            prepared: keep,
            error: None,
        });
    }

    fn diff_view(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let Some(state) = self.diff.as_mut() else {
            return;
        };
        let op = diff_stage_op(&state.key);
        let back = |ui: &mut egui::Ui| {
            if ui.button(tr("Zurück zum Graphen")).clicked() {
                DiffAction::Back
            } else {
                DiffAction::None
            }
        };
        let action = match (&mut state.prepared, &state.error) {
            (_, Some(e)) => {
                let action = back(ui);
                ui.label(RichText::new(e).color(t.error_text));
                action
            }
            (Some(prepared), None) => {
                let side = match &state.key {
                    DiffKey::Worktree { staged: false, .. } => Some(Side::Unstaged),
                    DiffKey::Worktree { staged: true, .. } => Some(Side::Staged),
                    _ => None,
                };
                let discard = match &state.key {
                    DiffKey::Worktree { staged: false, .. } => Some("Verwerfen …"),
                    DiffKey::Untracked { .. } => Some("Löschen …"),
                    _ => None,
                };
                prepared.show(ui, t, op.as_ref().map(StageOp::label), side, discard)
            }
            (None, None) => {
                let action = back(ui);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new(tr("Lade Diff …")).color(t.text_dim));
                });
                action
            }
        };
        match action {
            DiffAction::Back => self.diff = None,
            DiffAction::Act => {
                if let Some(op) = op {
                    self.stage(op);
                }
            }
            DiffAction::Discard => {
                let pending = self.diff.as_ref().and_then(|state| match &state.key {
                    DiffKey::Worktree {
                        path,
                        staged: false,
                        ..
                    } => Some(Pending::Files(vec![path.clone()])),
                    DiffKey::Untracked { path } => Some(Pending::Delete(vec![path.clone()])),
                    _ => None,
                });
                if let Some(p) = pending {
                    self.ask(p);
                }
            }
            DiffAction::DiscardLines(lines) => {
                let pending =
                    self.diff
                        .as_ref()
                        .and_then(|state| match (&state.key, &state.prepared) {
                            (
                                DiffKey::Worktree {
                                    path,
                                    staged: false,
                                    ..
                                },
                                Some(prepared),
                            ) => Some(Pending::Lines {
                                path: path.clone(),
                                shown: Box::new(prepared.diff.clone()),
                                lines,
                            }),
                            _ => None,
                        });
                if let Some(p) = pending {
                    self.ask(p);
                }
            }
            DiffAction::Lines(lines) => {
                if let Some(state) = &self.diff
                    && let DiffKey::Worktree { path, old, staged } = &state.key
                    && let Some(prepared) = &state.prepared
                {
                    let command = Command::StageLines {
                        path: path.clone(),
                        old: old.clone(),
                        staged: *staged,
                        shown: Box::new(prepared.diff.clone()),
                        lines,
                    };
                    // The refresh after the write reloads the diff.
                    self.write(command);
                }
            }
            DiffAction::None => {}
        }
    }

    /// Selects a commit and scrolls the history to it (if it has loaded).
    fn select_commit(&mut self, id: Oid) {
        self.diff = None;
        self.selection = Selection::Commit(id);
        let wip = self.wip().is_some();
        if let Some(row) = self.history.row_of(&self.selection, wip) {
            self.history.scroll_to(row);
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        if (self.diff.is_some() || self.search.open)
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            // The innermost first: an open diff, then the search.
            if self.diff.is_some() {
                self.diff = None;
            } else {
                self.search.close();
            }
        }
        let (find, previous, next) = ctx.input_mut(|i| {
            // Shift+F3 first, so that it does not also count as F3.
            let find = i.consume_shortcut(&FIND);
            let previous = i.consume_shortcut(&PREVIOUS);
            let next = i.consume_shortcut(&NEXT);
            if i.consume_shortcut(&KEYS) {
                self.show_keys = !self.show_keys;
            }
            (find, previous, next)
        });
        if find && self.session.is_some() {
            self.diff = None;
            self.search.open();
        }
        if next {
            self.jump(true);
        }
        if previous {
            self.jump(false);
        }
        let in_diff = self.diff.is_some();
        let (open, refresh, copy, focused, dropped, step) = ctx.input_mut(|i| {
            let mut step: i64 = 0;
            if !typing && !in_diff {
                for (key, n) in [
                    (Key::ArrowDown, 1),
                    (Key::ArrowUp, -1),
                    (Key::PageDown, 20),
                    (Key::PageUp, -20),
                    (Key::End, i64::MAX / 2),
                    (Key::Home, -(i64::MAX / 2)),
                ] {
                    if i.consume_key(Modifiers::NONE, key) {
                        step = step.saturating_add(n);
                    }
                }
            }
            (
                i.consume_shortcut(&OPEN),
                i.consume_shortcut(&REFRESH),
                !typing && i.consume_shortcut(&COPY),
                i.focused,
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .next(),
                step,
            )
        });
        if open {
            self.pick_folder(ctx);
        }
        if let Some(path) = dropped {
            self.open(path, ctx);
        } else if refresh || (focused && !self.was_focused && !self.is_busy()) {
            // Back in the window: files may have changed outside.
            self.refresh();
        }
        self.was_focused = focused;
        if step != 0 {
            self.move_selection(step);
        }
        if copy && let Selection::Commit(id) = &self.selection {
            ctx.copy_text(id.as_str().to_owned());
        }
    }

    fn move_selection(&mut self, step: i64) {
        let wip = self.wip().is_some();
        let total = self.history.display_rows(wip);
        if total == 0 {
            return;
        }
        let current = self.history.row_of(&self.selection, wip).unwrap_or(0);
        let last = i64::try_from(total - 1).unwrap_or(i64::MAX);
        let target = i64::try_from(current)
            .unwrap_or(0)
            .saturating_add(step)
            .clamp(0, last);
        let row = usize::try_from(target).unwrap_or(0);
        self.selection = self.history.selection_at(row, wip);
        self.history.scroll_to(row);
    }

    fn pick_folder(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title(tr("Repository öffnen"));
        if let Some(dir) = self.workdir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_folder() {
            self.open(path, ctx);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = theme::tokens(&ctx);
        ui.horizontal(|ui| {
            if let (Some(workdir), Some(status)) = (self.workdir(), self.status()) {
                let hint = format!(
                    "{}\n{}",
                    shown_path(workdir),
                    shortcut_hint(&ctx, tr("Anderes Repository öffnen"), &OPEN)
                );
                let repo_clicked =
                    toolbar::caption(ui, t, tr("Repository"), &repo_name(workdir), &hint, true)
                        .clicked();
                toolbar::caption(
                    ui,
                    t,
                    tr("Branch"),
                    &branch_name(&status.head),
                    &format!("{} · {}", head_text(&status.head), upstream_text(status)),
                    false,
                );
                ui.separator();
                if repo_clicked {
                    self.pick_folder(&ctx);
                }
            }
            let (behind, ahead) = self
                .status()
                .and_then(|s| s.upstream.as_ref())
                .and_then(|u| u.ahead_behind)
                .map_or((0, 0), |(a, b)| (b, a));
            toolbar::coming(ui, t, behind, ahead);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if toolbar::button(
                    ui,
                    t,
                    toolbar::Icon::Settings,
                    tr("Einstellungen"),
                    true,
                    None,
                )
                .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                toolbar::button(ui, t, toolbar::Icon::Commands, tr("Befehle"), false, None)
                    .on_hover_text(trf("Kommt mit Version {v}", &[("v", &"0.8")]));
                let search = ui.add_enabled(
                    self.session.is_some(),
                    egui::Button::new(
                        RichText::new(trf(
                            "Commits suchen ({taste})",
                            &[("taste", &ctx.format_shortcut(&FIND))],
                        ))
                        .color(t.text_dim),
                    )
                    .min_size(egui::vec2(190.0, 26.0)),
                );
                if search.clicked() {
                    self.diff = None;
                    self.search.open();
                }
                if self.session.is_none() {
                    let open = ui.button(tr("Öffnen …")).on_hover_text(shortcut_hint(
                        &ctx,
                        tr("Repository öffnen"),
                        &OPEN,
                    ));
                    if open.clicked() {
                        self.pick_folder(&ctx);
                    }
                }
                let busy = match &self.opening {
                    Some((p, _)) => Some(trf("Öffne {pfad} …", &[("pfad", &shown_path(p))])),
                    None => self.is_busy().then(|| tr("Git arbeitet …").to_owned()),
                };
                if let Some(text) = busy {
                    ui.label(RichText::new(text).color(t.text_dim));
                    ui.add(egui::Spinner::new());
                }
            });
        });
    }

    /// Asks before discarding or deleting; counts the lines that would be lost meanwhile.
    fn ask(&mut self, pending: Pending) {
        if let (Pending::Files(paths), Some(session)) = (&pending, &self.session) {
            session.submit(Command::CountLines(paths.clone()));
        }
        self.confirm = Some(Confirm::new(pending));
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let t = theme::tokens(ctx);
        match confirm.show(ctx, t, self.settings.discard_days) {
            Some(true) => {
                let command = confirm.command();
                self.confirm = None;
                self.write(command);
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    /// Asks whether the open repository may run the programs its configuration names, before
    /// the first write (docs/security.md T1). Esc or a click outside keeps the write from
    /// running and asks again next time.
    fn trust_dialog(&mut self, ctx: &egui::Context) {
        if self.trust_prompt.is_none() {
            return;
        }
        let Some(report) = self.session.as_ref().map(|s| s.trust_report().clone()) else {
            self.trust_prompt = None;
            return;
        };
        let t = theme::tokens(ctx);
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("gitcraft-vertrauen")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(
                RichText::new(tr("Diesem Repository vertrauen?"))
                    .color(t.text_strong)
                    .size(16.0),
            );
            ui.add_space(4.0);
            ui.label(tr(
                "Die Konfiguration dieses Repositorys lässt Git Programme starten. Beim Lesen schaltet GitCraft sie ab; beim Ändern (Vormerken, Commit, …) laufen sie mit. Vertraue dem Repository nur, wenn du weißt, woher es stammt.",
            ));
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for s in &report.settings {
                        ui.label(
                            RichText::new(format!("{} = {}", escape(&s.key), escape(&s.value)))
                                .monospace(),
                        );
                    }
                    if !report.hooks.is_empty() {
                        ui.add_space(4.0);
                        ui.label(trf(
                            "Hooks in {ordner}:",
                            &[("ordner", &shown_path(&report.hooks_dir))],
                        ));
                        for h in &report.hooks {
                            ui.label(RichText::new(escape(h)).monospace());
                        }
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Vertrauen")).clicked() {
                    choice = Some(true);
                }
                if ui
                    .button(tr("Nur lesen"))
                    .on_hover_text(tr("Bis zum nächsten Öffnen nichts ändern und nicht mehr fragen"))
                    .clicked()
                {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => self.trust(),
            Some(false) => {
                self.trust_prompt = None;
                self.read_only = true;
            }
            None if modal.should_close() => self.trust_prompt = None,
            None => {}
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut changed = false;
        egui::Window::new(tr("Einstellungen"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
            .show(ctx, |ui| {
                let t = theme::tokens(ui.ctx());
                changed |= ui
                    .checkbox(
                        &mut self.settings.avatars,
                        tr("Autorenbilder von Gravatar laden"),
                    )
                    .changed();
                ui.label(
                    RichText::new(tr(
                        "Für jede Autorin und jeden Autor geht ein Hash der E-Mail-Adresse an gravatar.com. Daran ist erkennbar, welche Personen in deinen Repositories vorkommen, auch in privaten. Bilder werden eine Woche lokal zwischengespeichert. Ohne diese Einstellung zeigt GitCraft Initialen und nimmt keine Verbindung auf.",
                    ))
                    .color(t.text_dim),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let label = ui.label(tr("Verworfene Inhalte aufbewahren (Tage)"));
                    changed |= ui
                        .add(egui::DragValue::new(&mut self.settings.discard_days).range(1..=365))
                        .labelled_by(label.id)
                        .changed();
                });
                ui.add_space(10.0);
                ui.label(RichText::new(tr("Vertraute Repositories")).color(t.text_strong));
                ui.label(
                    RichText::new(tr(
                        "Diese Repositories dürfen beim Ändern die Programme starten, die ihre Konfiguration nennt.",
                    ))
                    .color(t.text_dim),
                );
                if self.settings.trusted.is_empty() {
                    ui.label(RichText::new(tr("Keine")).color(t.text_dim));
                }
                let mut revoke = None;
                for (i, entry) in self.settings.trusted.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button(tr("Vertrauen entziehen")).clicked() {
                            revoke = Some(i);
                        }
                        ui.add(egui::Label::new(shown_path(&entry.workdir)).truncate());
                    });
                }
                if let Some(i) = revoke {
                    let entry = self.settings.trusted.remove(i);
                    changed = true;
                    if let Some(session) = &self.session
                        && session.workdir() == entry.workdir
                        && !session.trust_report().is_empty()
                    {
                        session.set_trusted(false);
                    }
                }
            });
        self.show_settings = open;
        if changed {
            self.apply_settings();
            self.save_settings();
        }
    }

    fn status_line(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let zoom = (ui.ctx().zoom_factor() * 100.0).round();
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            if let Some(workdir) = self.workdir() {
                ui.add(egui::Label::new(RichText::new(shown_path(workdir)).color(t.text_dim)).truncate());
                ui.label(RichText::new("·").color(t.text_dim));
            }
            if let Some(status) = self.status() {
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
                if !self.discarded.is_empty() {
                    if ui
                        .link(trf(
                            "Zuletzt verworfen ({n})",
                            &[("n", &self.discarded.len())],
                        ))
                        .clicked()
                    {
                        self.show_discarded = true;
                    }
                    ui.label(RichText::new("·").color(t.text_dim));
                }
                if let Some(v) = self.git_version {
                    ui.label(RichText::new(format!("git {v}")).color(t.text_dim));
                    ui.label(RichText::new("·").color(t.text_dim));
                }
                if ui
                    .add(egui::Label::new(RichText::new(tr("F1: Tastenkürzel")).color(t.text_dim)).sense(egui::Sense::click()))
                    .clicked()
                {
                    self.show_keys = !self.show_keys;
                }
                ui.label(RichText::new("·").color(t.text_dim));
                ui.label(RichText::new(trf("Zoom {p} %", &[("p", &zoom)])).color(t.text_dim))
                    .on_hover_text(tr("Ctrl+Plus, Ctrl+Minus, Ctrl+0"));
                if self.session.is_some() {
                    let n = self.history.commits().len();
                    let text = if self.history.is_loading() {
                        trf("{n} Commits geladen …", &[("n", &n)])
                    } else {
                        trf("{n} Commits", &[("n", &n)])
                    };
                    ui.label(RichText::new("·").color(t.text_dim));
                    ui.label(RichText::new(text).color(t.text_dim));
                }
            });
        });
    }

    /// The list of keyboard shortcuts (F1).
    fn keys_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_keys;
        egui::Window::new(tr("Tastenkürzel"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
            .show(ctx, |ui| {
                let t = theme::tokens(ui.ctx());
                let f = |s: &KeyboardShortcut| ctx.format_shortcut(s);
                let rows: Vec<(String, &str)> = vec![
                    (f(&OPEN), "Repository öffnen"),
                    (f(&REFRESH), "Status neu lesen"),
                    (f(&FIND), "Historie durchsuchen"),
                    (
                        format!("{} / {}", f(&NEXT), f(&PREVIOUS)),
                        "Nächster / vorheriger Treffer",
                    ),
                    (
                        tr("Pfeil hoch/runter, Bild hoch/runter, Pos1, Ende").to_owned(),
                        "Commit wählen",
                    ),
                    (f(&COPY), "Commit-ID kopieren"),
                    (
                        tr("Pfeil hoch/runter, Shift+Pfeil, Enter").to_owned(),
                        "Zeilen im Diff wählen und vormerken",
                    ),
                    (f(&commit::COMMIT_KEY), "Committen (im Commit-Formular)"),
                    ("Esc".to_owned(), "Diff oder Suche schließen"),
                    (tr("Ctrl+Plus, Ctrl+Minus, Ctrl+0").to_owned(), "Zoom"),
                    (f(&KEYS), "Diese Übersicht"),
                ];
                egui::Grid::new("tastenkuerzel")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (keys, what) in rows {
                            ui.label(RichText::new(keys).monospace().color(t.text_strong));
                            ui.label(tr(what));
                            ui.end_row();
                        }
                    });
                ui.separator();
                if ui.link(tr("GitCraft unterstützen …")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(SPONSOR_URL));
                }
            });
        self.show_keys = open;
    }

    fn history_view(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        if self.search.open {
            let (position, count) = self.match_position();
            let (action, run) = self.search.bar(ui, t, position, count);
            if let Some(e) = self.search.error() {
                ui.label(RichText::new(error_text(e)).color(t.error_text));
            }
            if run && let Some(reader) = self.session.as_ref().map(RepoSession::reader) {
                self.search.run(&reader, ui.ctx());
                self.collect_search_now();
            }
            match action {
                Some(BarAction::Next) => self.jump(true),
                Some(BarAction::Previous) => self.jump(false),
                Some(BarAction::Close) => self.search.close(),
                None => {}
            }
            ui.add_space(2.0);
        }
        if let Some(e) = self.history.error() {
            ui.label(RichText::new(error_text(e)).color(t.error_text));
        }
        let wip = self.wip();
        if self.history.commits().is_empty() && wip.is_none() && !self.history.is_loading() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(tr("Leeres Repository"))
                        .color(t.text_strong)
                        .size(16.0),
                );
                ui.label(
                    RichText::new(tr("Noch keine Commits und keine Dateien.")).color(t.text_dim),
                );
            });
            return;
        }
        let head = self.status().and_then(head_commit);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let view = View {
            labels: &self.labels,
            avatars: &self.avatars,
            search: Some(&self.search),
            wip,
            head: head.as_ref(),
            now,
        };
        let actions = self.history.show(ui, t, &view, &self.selection);
        for action in actions {
            match action {
                Action::Select(s) => self.selection = s,
                Action::CopyId(id) => ui.ctx().copy_text(id.as_str().to_owned()),
                Action::CopyMessage(m) => ui.ctx().copy_text(m),
            }
        }
    }

    /// The details panel: the selected commit, or the changes of the work in progress.
    fn details(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        // Keep the panel at its width instead of shrinking to the content.
        ui.set_min_width(ui.available_width());
        match self.selection.clone() {
            Selection::WorkInProgress => {
                ui.label(
                    RichText::new(tr("Arbeitsstand"))
                        .color(t.text_strong)
                        .size(15.0),
                );
                ui.add_space(4.0);
                let staged = self.status().map_or(0, |s| s.staged().count());
                let action = egui::Panel::bottom("commit-formular")
                    .resizable(false)
                    .show(ui, |ui| self.commit.show(ui, t, staged))
                    .inner;
                self.changes(ui);
                match action {
                    Some(FormAction::Commit {
                        message,
                        amend,
                        no_verify,
                    }) => {
                        if !self.write(Command::Commit {
                            message,
                            amend,
                            no_verify,
                        }) {
                            self.commit.failed();
                        }
                    }
                    Some(FormAction::LoadAmend) => {
                        if let Some(session) = &self.session {
                            session.submit(Command::AmendInfo);
                        }
                    }
                    Some(FormAction::SetIdentity {
                        name,
                        email,
                        global,
                    }) => {
                        self.write(Command::SetIdentity {
                            name,
                            email,
                            global,
                        });
                    }
                    None => {}
                }
            }
            Selection::Commit(id) => {
                let Some(c) = self.history.commit(&id).cloned() else {
                    return;
                };
                ui.label(
                    RichText::new(escape(&c.subject))
                        .color(t.text_strong)
                        .size(15.0),
                );
                ui.add_space(6.0);
                let labels = self.labels.of(&c.id);
                if !labels.is_empty() {
                    let names: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
                    ui.label(RichText::new(names.join(", ")).color(t.accent));
                }
                egui::Grid::new("commit-details")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new(tr("Commit")).color(t.text_dim));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(c.id.short()).monospace())
                                .on_hover_text(c.id.as_str());
                            if ui.small_button(tr("Kopieren")).clicked() {
                                ui.ctx().copy_text(c.id.as_str().to_owned());
                            }
                        });
                        ui.end_row();
                        ui.label(RichText::new(tr("Autor")).color(t.text_dim));
                        ui.label(format!(
                            "{} <{}>\n{}",
                            escape(&c.author.name),
                            escape(&c.author.email),
                            history::absolute_date(c.author.time)
                        ));
                        ui.end_row();
                        if c.committer != c.author {
                            ui.label(RichText::new(tr("Committer")).color(t.text_dim));
                            ui.label(format!(
                                "{} <{}>\n{}",
                                escape(&c.committer.name),
                                escape(&c.committer.email),
                                history::absolute_date(c.committer.time)
                            ));
                            ui.end_row();
                        }
                        ui.label(RichText::new(tr("Eltern")).color(t.text_dim));
                        ui.horizontal_wrapped(|ui| {
                            if c.parents.is_empty() {
                                ui.label(
                                    RichText::new(tr("keine (erster Commit)")).color(t.text_dim),
                                );
                            }
                            for p in &c.parents {
                                if ui.link(p.short()).on_hover_text(p.as_str()).clicked() {
                                    self.select_commit(p.clone());
                                }
                            }
                        });
                        ui.end_row();
                    });
                ui.add_space(8.0);
                self.commit_body(ui, t, &c);
            }
            Selection::None => {
                ui.label(RichText::new(tr("Nichts ausgewählt")).color(t.text_dim));
            }
        }
    }

    /// Full message and changed files of the selected commit; a click on a file shows its diff.
    fn commit_body(&mut self, ui: &mut egui::Ui, t: &theme::Tokens, commit: &gitcraft_git::Commit) {
        let Some(details) = self.details.as_ref().filter(|d| d.id == commit.id) else {
            return;
        };
        if let Some(e) = &details.error {
            ui.label(RichText::new(e).color(t.error_text));
            return;
        }
        let (Some(message), Some(files)) = (&details.message, &details.files) else {
            ui.add(egui::Spinner::new());
            return;
        };
        // The body (the subject is the heading already).
        let body = message.split_once('\n').map_or("", |(_, b)| b.trim());
        if !body.is_empty() {
            ui.add(egui::Label::new(RichText::new(escape_message(body)).color(t.text)).wrap());
            ui.add_space(8.0);
        }
        ui.label(
            RichText::new(if files.len() == 1 {
                tr("1 Datei geändert").to_owned()
            } else {
                trf("{n} Dateien geändert", &[("n", &files.len())])
            })
            .color(t.text_strong),
        );
        let current = self.diff.as_ref().map(|s| s.key.clone());
        let mut open = None;
        for (i, f) in files.iter().enumerate() {
            let key = DiffKey::Commit {
                commit: Box::new(commit.clone()),
                file: f.clone(),
            };
            let path = match &f.old_path {
                Some(old) => format!("{} -> {}", old.display(), f.path.display()),
                None => f.path.display(),
            };
            let (letter, color, meaning) = match f.status {
                FileStatus::Added => ('A', t.added, "hinzugefügt"),
                FileStatus::Modified => ('M', t.modified, "geändert"),
                FileStatus::Deleted => ('D', t.deleted, "gelöscht"),
                FileStatus::Renamed => ('R', t.renamed, "umbenannt"),
            };
            let row_h = ui.text_style_height(&egui::TextStyle::Body).max(20.0);
            let inner = ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), row_h),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if Some(&key) == current.as_ref() {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, t.selection);
                    }
                    let counts = match f.lines {
                        Some((a, d)) => format!("+{a} -{d}"),
                        None => tr("binär").to_owned(),
                    };
                    egui::Sides::new().shrink_left().show(
                        ui,
                        |ui| {
                            ui.label(RichText::new(letter.to_string()).monospace().color(color))
                                .on_hover_text(tr(meaning));
                            ui.add(egui::Label::new(path.as_str()).truncate());
                        },
                        |ui| {
                            ui.label(RichText::new(counts).color(t.text_dim).size(12.0));
                        },
                    );
                },
            );
            let resp = ui.interact(
                inner.response.rect,
                ui.scope_id().with(("file", i)),
                egui::Sense::click(),
            );
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &path));
            if resp.clicked() {
                open = Some((key, path));
            }
        }
        if let Some((key, title)) = open {
            self.open_diff(key, title);
        }
    }

    /// The change list of the work in progress: what is not staged yet, then what is. A click
    /// on a file shows its diff; a double click, the row's button or its context menu stages or
    /// unstages it; each section stages or unstages all of its files.
    fn changes(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let row_h = ui.text_style_height(&egui::TextStyle::Body).max(20.0) + 2.0;
        let current = self.diff.as_ref().map(|s| s.key.clone());
        let mut open: Option<(DiffKey, String)> = None;
        let mut op: Option<StageOp> = None;
        let mut ask: Option<Pending> = None;
        let rows = &self.rows;
        egui::ScrollArea::vertical()
            .id_salt("changes")
            .auto_shrink([false, false])
            .show_rows(ui, row_h, rows.len(), |ui, range| {
                for (i, row) in rows.iter().enumerate().skip(range.start).take(range.len()) {
                    let size = egui::vec2(ui.available_width(), row_h);
                    let file = matches!(row, Row::File { .. });
                    let sense = if file { egui::Sense::click() } else { egui::Sense::hover() };
                    let (rect, resp) = ui.allocate_exact_size(size, sense);
                    let selected = matches!(row, Row::File { diff: Some(k), .. } if Some(k) == current.as_ref());
                    if selected {
                        ui.painter().rect_filled(rect, 0.0, t.selection);
                    } else if i % 2 == 1 && file {
                        // Stripes on file rows only, so section titles stand apart.
                        ui.painter().rect_filled(rect, 0.0, t.row_alt);
                    }
                    let hot = ui.rect_contains_pointer(rect) || resp.has_focus();
                    // Drawn after the row's own response, so the buttons get the clicks.
                    let mut cell = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(Layout::right_to_left(Align::Center)),
                    );
                    match row {
                        Row::Section { all, .. } => {
                            if let Some(all) = all
                                && cell.small_button(tr(all.all_label())).clicked()
                            {
                                op = Some(all.clone());
                            }
                            cell.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                draw_row(ui, row, t);
                            });
                        }
                        Row::File { diff, stage, path, discard, .. } => {
                            if let Some(stage) = stage
                                && hot
                                && cell.small_button(tr(stage.label())).clicked()
                            {
                                op = Some(stage.clone());
                            }
                            cell.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                draw_row(ui, row, t);
                            });
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, path));
                            if resp.double_clicked() {
                                op = stage.clone();
                            } else if resp.clicked()
                                && let Some(key) = diff
                            {
                                open = Some((key.clone(), path.clone()));
                            }
                            resp.context_menu(|ui| {
                                if let Some(stage) = stage
                                    && ui.button(tr(stage.label())).clicked()
                                {
                                    op = Some(stage.clone());
                                    ui.close();
                                }
                                if let Some(d) = discard {
                                    let label = match d {
                                        Pending::Delete(_) => "Löschen …",
                                        _ => "Verwerfen …",
                                    };
                                    if ui.button(RichText::new(tr(label)).color(t.deleted)).clicked() {
                                        ask = Some(d.clone());
                                        ui.close();
                                    }
                                }
                                if ui.button(tr("Pfad kopieren")).clicked() {
                                    // The file's own path, also for a rename ("old -> new").
                                    let copied = match diff {
                                        Some(
                                            DiffKey::Worktree { path, .. }
                                            | DiffKey::Untracked { path },
                                        ) => path.display(),
                                        _ => path.clone(),
                                    };
                                    ui.ctx().copy_text(copied);
                                    ui.close();
                                }
                            });
                        }
                    }
                }
            });
        if let Some((key, title)) = open {
            self.open_diff(key, title);
        }
        if let Some(op) = op {
            self.stage(op);
        }
        if let Some(p) = ask {
            self.ask(p);
        }
    }

    /// Stages or unstages; an open diff of one of the files follows it to the other side.
    fn stage(&mut self, op: StageOp) {
        let paths = match &op {
            StageOp::Stage(p) | StageOp::Unstage(p) => p.clone(),
        };
        if !self.write(op.command()) {
            return;
        }
        let follow = self.diff.as_ref().and_then(|state| {
            let path = match &state.key {
                DiffKey::Worktree { path, .. } | DiffKey::Untracked { path } => path,
                DiffKey::Commit { .. } => return None,
            };
            paths
                .contains(path)
                .then(|| moved_key(&state.key))
                .flatten()
                .map(|key| (key, state.title.clone()))
        });
        if let Some((key, title)) = follow {
            // The refresh after the write reloads it.
            self.open_diff(key, title);
        }
    }

    /// Commit indices (in the loaded history) that match the active search.
    fn match_indices(&self) -> Vec<usize> {
        if !self.search.active() {
            return Vec::new();
        }
        self.history
            .commits()
            .iter()
            .enumerate()
            .filter(|(_, c)| self.search.is_match(c))
            .map(|(i, _)| i)
            .collect()
    }

    /// The selected commit's number among the matches, and the number of matches.
    fn match_position(&self) -> (Option<usize>, usize) {
        let matches = self.match_indices();
        let selected = match &self.selection {
            Selection::Commit(id) => self.history.position(id),
            _ => None,
        };
        let position = selected.and_then(|s| matches.iter().position(|m| *m == s).map(|p| p + 1));
        (position, matches.len())
    }

    fn first_match(&self) -> Option<usize> {
        self.match_indices().first().copied()
    }

    /// Selects the commit at index `i` of the history and scrolls to it.
    fn jump_to(&mut self, i: Option<usize>) {
        let Some(commit) = i.and_then(|i| self.history.commits().get(i)) else {
            return;
        };
        self.select_commit(commit.id.clone());
    }

    /// The next (or previous) match after the selection; wraps around in the loaded history
    /// and asks for more history while it is still loading.
    fn jump(&mut self, forward: bool) {
        let matches = self.match_indices();
        if self.history.is_loading() {
            self.history.load_more();
        }
        if matches.is_empty() {
            return;
        }
        let current = match &self.selection {
            Selection::Commit(id) => self.history.position(id),
            _ => None,
        };
        let target = match (current, forward) {
            (Some(c), true) => matches.iter().find(|m| **m > c).or(matches.first()),
            (Some(c), false) => matches.iter().rev().find(|m| **m < c).or(matches.last()),
            (None, true) => matches.first(),
            (None, false) => matches.last(),
        };
        self.jump_to(target.copied());
    }

    /// Commit-ID searches need no git: jump right away.
    fn collect_search_now(&mut self) {
        if self.search.jump_pending && !self.search.is_running() {
            self.search.jump_pending = false;
            self.jump_to(self.first_match());
        }
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

/// The branch name for the toolbar, or `HEAD 1a2b3c4` when detached.
/// Staging the file a diff shows: stage a working-tree or untracked diff, unstage a staged one.
fn diff_stage_op(key: &DiffKey) -> Option<StageOp> {
    match key {
        DiffKey::Commit { .. } => None,
        DiffKey::Worktree { path, old, staged } => {
            let paths: Vec<RepoPath> = std::iter::once(path.clone()).chain(old.clone()).collect();
            Some(if *staged {
                StageOp::Unstage(paths)
            } else {
                StageOp::Stage(paths)
            })
        }
        DiffKey::Untracked { path } => Some(StageOp::Stage(vec![path.clone()])),
    }
}

/// The diff of the same file on the other side of the index, after staging or unstaging it.
fn moved_key(key: &DiffKey) -> Option<DiffKey> {
    match key {
        DiffKey::Commit { .. } => None,
        DiffKey::Worktree { path, old, staged } => Some(DiffKey::Worktree {
            path: path.clone(),
            old: old.clone(),
            staged: !staged,
        }),
        DiffKey::Untracked { path } => Some(DiffKey::Worktree {
            path: path.clone(),
            old: None,
            staged: true,
        }),
    }
}

/// Asks egui for a repaint from a worker thread.
fn wake(ctx: &egui::Context) -> Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

fn branch_name(head: &Head) -> String {
    match head {
        Head::Branch { name, .. } => escape(name),
        Head::Detached { commit } => format!("HEAD {}", commit.short()),
    }
}

/// A commit message for display: line breaks kept, other control and invisible characters
/// escaped.
fn escape_message(text: &str) -> String {
    text.split('\n').map(escape).collect::<Vec<_>>().join("\n")
}

/// The commit HEAD points at, if any.
fn head_commit(status: &Status) -> Option<Oid> {
    match &status.head {
        Head::Branch { commit, .. } => commit.clone(),
        Head::Detached { commit } => Some(commit.clone()),
    }
}

fn draw_row(ui: &mut egui::Ui, row: &Row, t: &theme::Tokens) {
    // Not selectable: the row underneath takes the clicks (diff, double click to stage).
    let label = |text: RichText| egui::Label::new(text).selectable(false);
    match row {
        Row::Section { title, count, .. } => {
            ui.add(label(RichText::new(tr(title)).color(t.text_strong)));
            ui.add(label(RichText::new(count.to_string()).color(t.text_dim)));
        }
        Row::File {
            letter,
            tone,
            meaning,
            path,
            ..
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
            ui.add(label(
                RichText::new(letter.to_string()).monospace().color(colour),
            ))
            .on_hover_text(tr(meaning));
            ui.add(label(RichText::new(path.as_str())).truncate())
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
        .map(|n| escape(&n.to_string_lossy()))
        .unwrap_or_else(|| shown_path(workdir))
}

/// A path for display: escaped like file names (control, invisible and direction characters).
fn shown_path(path: &Path) -> String {
    escape(&path.to_string_lossy())
}

fn head_text(head: &Head) -> String {
    match head {
        Head::Branch {
            name,
            commit: Some(_),
        } => trf("Branch {name}", &[("name", &escape(name))]),
        Head::Branch { name, commit: None } => trf(
            "Branch {name} (noch keine Commits)",
            &[("name", &escape(name))],
        ),
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
    let name = escape(&up.name);
    match up.ahead_behind {
        None => trf(
            "Upstream {upstream} existiert nicht mehr",
            &[("upstream", &name)],
        ),
        Some((0, 0)) => trf("auf dem Stand von {upstream}", &[("upstream", &name)]),
        Some((ahead, behind)) => trf(
            "{voraus} voraus, {zurueck} zurück gegenüber {upstream}",
            &[
                ("voraus", &ahead),
                ("zurueck", &behind),
                ("upstream", &name),
            ],
        ),
    }
}

fn change_row(file: &FileChange, change: Change, staged: bool) -> Row {
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
    // A rename moves both paths: the old one's deletion goes with the new one.
    let mut paths = vec![file.path.clone()];
    if let (Some(orig), Change::Renamed) = (&file.orig_path, change) {
        paths.push(orig.clone());
    }
    Row::File {
        letter: change.letter(),
        tone,
        meaning,
        path,
        diff: Some(DiffKey::Worktree {
            path: file.path.clone(),
            old: file.orig_path.clone(),
            staged,
        }),
        discard: (!staged).then(|| Pending::Files(vec![file.path.clone()])),
        stage: Some(if staged {
            StageOp::Unstage(paths)
        } else {
            StageOp::Stage(paths)
        }),
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

/// The rows of the change list: conflicts, not staged (changed and untracked files), staged.
fn rows(status: &Status) -> Vec<Row> {
    let mut out = Vec::new();
    let mut section = |title: &'static str, items: Vec<Row>| {
        if items.is_empty() {
            return;
        }
        let mut all: Option<StageOp> = None;
        for item in &items {
            let Row::File {
                stage: Some(op), ..
            } = item
            else {
                continue;
            };
            match (&mut all, op) {
                (None, op) => all = Some(op.clone()),
                (Some(StageOp::Stage(a)), StageOp::Stage(p))
                | (Some(StageOp::Unstage(a)), StageOp::Unstage(p)) => a.extend(p.iter().cloned()),
                _ => {}
            }
        }
        out.push(Row::Section {
            title,
            count: items.len(),
            all,
        });
        out.extend(items);
    };
    section(
        "Konflikte",
        status
            .conflicts()
            .map(|(f, c)| Row::File {
                letter: 'U',
                tone: Tone::Conflict,
                meaning: conflict_meaning(c),
                diff: None,
                stage: None,
                discard: None,
                path: f.path.display(),
            })
            .collect(),
    );
    let untracked = status.untracked().map(|f| Row::File {
        letter: '?',
        tone: Tone::Untracked,
        meaning: "unversioniert",
        diff: Some(DiffKey::Untracked {
            path: f.path.clone(),
        }),
        stage: Some(StageOp::Stage(vec![f.path.clone()])),
        discard: Some(Pending::Delete(vec![f.path.clone()])),
        path: f.path.display(),
    });
    section(
        "Nicht vorgemerkt",
        status
            .unstaged()
            .map(|(f, c)| change_row(f, c, false))
            .chain(untracked)
            .collect(),
    );
    section(
        "Vorgemerkt",
        status
            .staged()
            .map(|(f, c)| change_row(f, c, true))
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
        GitError::NotARepository(p) => trf(
            "{pfad} ist kein Git-Repository.",
            &[("pfad", &shown_path(p))],
        ),
        GitError::Bare(p) => trf(
            "{pfad} ist ein Bare-Repository ohne Arbeitsverzeichnis; das wird noch nicht unterstützt.",
            &[("pfad", &shown_path(p))],
        ),
        GitError::DubiousOwnership(p) => trf(
            "Git verweigert {pfad}, weil der Ordner einem anderen Benutzer gehört. Wenn du ihm vertraust, trage ihn in Gits Einstellung safe.directory ein.",
            &[("pfad", &shown_path(p))],
        ),
        GitError::Failed {
            command, stderr, ..
        } => trf(
            "git {befehl} ist fehlgeschlagen: {meldung}",
            &[("befehl", command), ("meldung", &escape(stderr))],
        ),
        GitError::OutputTooLarge { command, limit } => trf(
            "git {befehl} lieferte zu viel Ausgabe (über {mib} MiB).",
            &[("befehl", command), ("mib", &(limit / (1024 * 1024)))],
        ),
        GitError::Spawn(err) => trf(
            "Git konnte nicht gestartet werden: {fehler}",
            &[("fehler", err)],
        ),
        GitError::TimedOut { command, seconds } => trf(
            "git {befehl} brauchte länger als {sekunden} s und wurde beendet.",
            &[("befehl", command), ("sekunden", seconds)],
        ),
        GitError::UnsafeConfig(key) => trf(
            "Die Konfiguration des Repositorys setzt {schluessel}; GitCraft kann das nicht sicher abschalten und liest das Repository deshalb nicht.",
            &[("schluessel", &escape(key))],
        ),
        GitError::Cancelled => tr("Abgebrochen.").to_owned(),
        GitError::UnsafePath(p) => trf(
            "GitCraft ändert diesen Pfad nicht: {pfad}",
            &[("pfad", &escape(p))],
        ),
        GitError::Stale => tr("Die Datei hat sich geändert, seit ihr Diff angezeigt wurde; nichts wurde übernommen. Der Diff ist neu geladen.").to_owned(),
        GitError::InvalidValue(v) => trf(
            "Ungültige Eingabe: {wert}",
            &[("wert", &escape(v))],
        ),
        GitError::NotTrusted => tr("Dieses Repository ist nur zum Lesen geöffnet: Seine Konfiguration startet Programme, denen du noch nicht vertraut hast.").to_owned(),
        GitError::Parse(err) => trf(
            "Unerwartete Ausgabe von git: {fehler}",
            &[("fehler", &escape(err))],
        ),
    }
}

#[cfg(test)]
mod tests;
