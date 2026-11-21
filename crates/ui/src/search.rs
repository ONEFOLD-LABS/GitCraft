//! History search (issue #32): a bar above the graph. Matches stay in place and the rest of the
//! history is dimmed; Enter/F3 and Shift+Enter/Shift+F3 jump between matches. Git searches on
//! a background thread and reports matches as it finds them; a new search or closing the bar
//! stops it. Commit ids are matched locally against the loaded history.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, channel};

use egui::RichText;
use gitcraft_core::Reader;
use gitcraft_git::{Commit, GitError, Oid, SearchKind};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Longest search text.
const MAX_TEXT: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Message,
    Author,
    Id,
    Path,
    Content,
}

impl Kind {
    pub(crate) const ALL: [Kind; 5] = [
        Kind::Message,
        Kind::Author,
        Kind::Id,
        Kind::Path,
        Kind::Content,
    ];

    /// German label (translated where shown).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Kind::Message => "Nachricht",
            Kind::Author => "Autor",
            Kind::Id => "Commit-ID",
            Kind::Path => "Pfad",
            Kind::Content => "Inhalt",
        }
    }

    fn git(self) -> Option<SearchKind> {
        match self {
            Kind::Message => Some(SearchKind::Message),
            Kind::Author => Some(SearchKind::Author),
            Kind::Path => Some(SearchKind::Path),
            Kind::Content => Some(SearchKind::Content),
            Kind::Id => None,
        }
    }
}

enum Msg {
    Batch(Vec<Oid>),
    Done,
    Failed(GitError),
}

/// What the user asked for in the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarAction {
    Next,
    Previous,
    Close,
}

pub(crate) struct Search {
    pub(crate) open: bool,
    text: String,
    kind: Kind,
    focus: bool,
    /// The search the matches belong to.
    ran: Option<(Kind, String)>,
    matches: HashSet<Oid>,
    running: Option<Receiver<Msg>>,
    error: Option<GitError>,
    /// Jump to the first match once one arrives.
    pub(crate) jump_pending: bool,
}

impl Default for Search {
    fn default() -> Self {
        Search {
            open: false,
            text: String::new(),
            kind: Kind::Message,
            focus: false,
            ran: None,
            matches: HashSet::new(),
            running: None,
            error: None,
            jump_pending: false,
        }
    }
}

impl Search {
    #[cfg(test)]
    pub(crate) fn set_for_test(&mut self, text: Option<&str>, kind: Option<Kind>) {
        if let Some(text) = text {
            self.text = text.to_owned();
        }
        if let Some(kind) = kind {
            self.kind = kind;
        }
    }

    /// Opens the bar and puts the cursor into the text field.
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    /// Closes the bar, stops a running search and forgets the matches.
    pub(crate) fn close(&mut self) {
        *self = Search {
            kind: self.kind,
            ..Search::default()
        };
    }

    /// A search has run (or runs): the history shows matches.
    pub(crate) fn active(&self) -> bool {
        self.ran.is_some()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub(crate) fn error(&self) -> Option<&GitError> {
        self.error.as_ref()
    }

    pub(crate) fn is_match(&self, commit: &Commit) -> bool {
        match &self.ran {
            Some((Kind::Id, prefix)) => commit.id.as_str().starts_with(prefix.as_str()),
            Some(_) => self.matches.contains(&commit.id),
            None => true,
        }
    }

    /// Runs the search in the bar (a running one is stopped).
    pub(crate) fn run(&mut self, repo: &Reader, ctx: &egui::Context) {
        let text: String = self.text.trim().chars().take(MAX_TEXT).collect();
        self.running = None;
        self.matches.clear();
        self.error = None;
        if text.is_empty() {
            self.ran = None;
            return;
        }
        self.jump_pending = true;
        let Some(kind) = self.kind.git() else {
            self.ran = Some((Kind::Id, text.to_lowercase()));
            return;
        };
        self.ran = Some((self.kind, text.clone()));
        let (tx, rx) = channel();
        let repo = repo.clone();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new()
            .name("gitcraft-search".into())
            .spawn(move || {
                let mut stream = match repo.search(kind, &text) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(Msg::Failed(e));
                        ctx.request_repaint();
                        return;
                    }
                };
                loop {
                    let msg = match stream.next_batch(200) {
                        Ok(b) if b.is_empty() && stream.is_done() => Msg::Done,
                        Ok(b) => Msg::Batch(b),
                        Err(e) => Msg::Failed(e),
                    };
                    let end = !matches!(msg, Msg::Batch(_));
                    // The bar was closed or a new search started: stop (drops git).
                    if tx.send(msg).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    if end {
                        return;
                    }
                }
            });
        match started {
            Ok(_) => self.running = Some(rx),
            Err(e) => self.error = Some(GitError::Spawn(e.to_string())),
        }
    }

    /// Takes matches that arrived.
    pub(crate) fn poll(&mut self) {
        let Some(rx) = &self.running else {
            return;
        };
        let msgs: Vec<Msg> = rx.try_iter().collect();
        for msg in msgs {
            match msg {
                Msg::Batch(ids) => self.matches.extend(ids),
                Msg::Done => self.running = None,
                Msg::Failed(e) => {
                    self.error = Some(e);
                    self.running = None;
                }
            }
        }
    }

    /// The search bar; `position` is the selected match's number and `count` the matches in
    /// the loaded history.
    pub(crate) fn bar(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        position: Option<usize>,
        count: usize,
    ) -> (Option<BarAction>, bool) {
        let mut action = None;
        let mut run = false;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("suchart")
                .selected_text(tr(self.kind.label()))
                .show_ui(ui, |ui| {
                    for k in Kind::ALL {
                        ui.selectable_value(&mut self.kind, k, tr(k.label()));
                    }
                })
                .response
                .on_hover_text(tr("Wonach gesucht wird"));
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .hint_text(tr("Historie durchsuchen"))
                    // Leave room for the buttons and the count beside it.
                    .desired_width((ui.available_width() - 330.0).clamp(100.0, 320.0)),
            );
            if self.focus {
                field.request_focus();
                self.focus = false;
            }
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if enter {
                let shift = ui.input(|i| i.modifiers.shift);
                let current = Some((self.kind, self.text.trim().to_owned()));
                let same = match (&self.ran, &current) {
                    (Some((Kind::Id, a)), Some((Kind::Id, b))) => a == &b.to_lowercase(),
                    (a, b) => a == b,
                };
                if same && self.active() {
                    action = Some(if shift {
                        BarAction::Previous
                    } else {
                        BarAction::Next
                    });
                } else {
                    run = true;
                }
                field.request_focus();
            }
            if ui
                .button(tr("Vorheriger"))
                .on_hover_text("Shift+F3")
                .clicked()
            {
                action = Some(BarAction::Previous);
            }
            if ui.button(tr("Nächster")).on_hover_text("F3").clicked() {
                action = Some(BarAction::Next);
            }
            if self.is_running() {
                ui.add(egui::Spinner::new());
            }
            if self.active() {
                let text = match (position, count) {
                    (_, 0) if self.is_running() => tr("Suche …").to_owned(),
                    (_, 0) => tr("Keine Treffer").to_owned(),
                    (Some(p), n) => trf("{p} von {n}", &[("p", &p), ("n", &n)]),
                    (None, n) => trf("{n} Treffer", &[("n", &n)]),
                };
                ui.label(RichText::new(text).color(t.text_dim));
            }
            if ui.button(tr("Schließen")).on_hover_text("Esc").clicked() {
                action = Some(BarAction::Close);
            }
        });
        (action, run)
    }
}
