//! The commit form at the bottom of the work-in-progress panel (issues #26, #27): summary with
//! a 72-character hint, optional description, amend, skip hooks, Ctrl+Enter. Before the first
//! commit without an identity it asks for name and e-mail instead.

use egui::{Key, KeyboardShortcut, Modifiers, RichText};
use gitcraft_git::{Identity, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Commits from anywhere in the form.
pub(crate) const COMMIT_KEY: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
/// Length of a summary that reads well in one line of most tools.
const SUMMARY_HINT: usize = 72;
/// Longest message GitCraft passes on.
const MAX_MESSAGE: usize = 1024 * 1024;

/// What the user asked for in the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FormAction {
    Commit {
        message: String,
        amend: bool,
        no_verify: bool,
    },
    /// Amend was switched on: load the last commit's message.
    LoadAmend,
    SetIdentity {
        name: String,
        email: String,
        global: bool,
    },
}

#[derive(Default)]
pub(crate) struct CommitForm {
    pub(crate) summary: String,
    pub(crate) body: String,
    pub(crate) amend: bool,
    pub(crate) no_verify: bool,
    /// The last commit's message and whether a remote has it; `None` until loaded.
    amend_info: Option<Option<(String, bool)>>,
    /// `None` until read.
    pub(crate) identity: Option<Identity>,
    name: String,
    email: String,
    global: bool,
    /// A commit is running.
    pub(crate) committing: bool,
    /// What the hooks printed for the last commit.
    note: Option<String>,
}

impl CommitForm {
    /// The message as git gets it: summary, blank line, description.
    pub(crate) fn message(&self) -> String {
        let summary = self.summary.trim();
        let body = self.body.trim_end();
        let mut m = if body.trim().is_empty() {
            summary.to_owned()
        } else {
            format!("{summary}\n\n{body}")
        };
        if m.len() > MAX_MESSAGE {
            let mut end = MAX_MESSAGE;
            while !m.is_char_boundary(end) {
                end -= 1;
            }
            m.truncate(end);
        }
        m
    }

    /// Identity read: prefills the identity fields with what is there.
    pub(crate) fn set_identity(&mut self, identity: Identity) {
        if self.name.is_empty() {
            self.name = identity.name.clone().unwrap_or_default();
        }
        if self.email.is_empty() {
            self.email = identity.email.clone().unwrap_or_default();
        }
        self.identity = Some(identity);
    }

    /// The last commit's message arrived: fills an empty form with it.
    pub(crate) fn set_amend_info(&mut self, info: Option<(String, bool)>) {
        if self.amend
            && self.summary.trim().is_empty()
            && self.body.trim().is_empty()
            && let Some((message, _)) = &info
        {
            let (summary, body) = message.split_once('\n').unwrap_or((message, ""));
            self.summary = summary.to_owned();
            self.body = body.trim_start_matches('\n').to_owned();
        }
        self.amend_info = Some(info);
    }

    /// The commit went through: the form starts over; hook output stays visible.
    pub(crate) fn committed(&mut self, output: &str) {
        *self = CommitForm {
            identity: self.identity.take(),
            note: (!output.trim().is_empty()).then(|| escape(output.trim())),
            ..CommitForm::default()
        };
    }

    pub(crate) fn failed(&mut self) {
        self.committing = false;
    }

    /// Draws the form; `staged` is the number of staged files.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        staged: usize,
    ) -> Option<FormAction> {
        ui.add_space(6.0);
        match &self.identity {
            Some(id) if !id.is_complete() => return self.identity_form(ui, t),
            _ => {}
        }
        let mut action = None;
        let mut summary_label = None;
        ui.horizontal(|ui| {
            summary_label = Some(
                ui.label(RichText::new(tr("Zusammenfassung")).color(t.text_strong))
                    .id,
            );
            let n = self.summary.trim().chars().count();
            let colour = if n > SUMMARY_HINT {
                t.conflict
            } else {
                t.text_dim
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{n}/{SUMMARY_HINT}")).color(colour))
                    .on_hover_text(tr("Zusammenfassungen bis 72 Zeichen passen in jede Liste."));
            });
        });
        let summary = ui.add(
            egui::TextEdit::singleline(&mut self.summary)
                .hint_text(tr("Was ändert dieser Commit?"))
                .desired_width(f32::INFINITY),
        );
        if let Some(id) = summary_label {
            summary.labelled_by(id);
        }
        let body_label = ui.label(RichText::new(tr("Beschreibung (optional)")).color(t.text_dim));
        ui.add(
            egui::TextEdit::multiline(&mut self.body)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(body_label.id);
        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(&mut self.amend, tr("Letzten Commit ändern"))
                .on_hover_text("git commit --amend")
                .changed()
            {
                if self.amend {
                    self.amend_info = None;
                    action = Some(FormAction::LoadAmend);
                } else if let Some(Some((message, _))) = &self.amend_info
                    && self.message() == *message
                {
                    // Switched off again before changing anything: start empty.
                    self.summary.clear();
                    self.body.clear();
                }
            }
            ui.checkbox(&mut self.no_verify, tr("Hooks überspringen"))
                .on_hover_text("git commit --no-verify");
        });
        if self.amend
            && let Some(Some((_, true))) = &self.amend_info
        {
            ui.label(
                RichText::new(tr(
                    "Dieser Commit ist schon auf einem Remote. Nach dem Ändern braucht der Branch einen Force-Push, und wer den alten Commit schon hat, muss nacharbeiten.",
                ))
                .color(t.error_text),
            );
        }
        let missing = if self.summary.trim().is_empty() {
            Some(tr("Die Zusammenfassung fehlt."))
        } else if staged == 0 && !self.amend {
            Some(tr("Noch nichts vorgemerkt."))
        } else {
            None
        };
        let label = if self.amend {
            tr("Commit ändern").to_owned()
        } else if staged == 1 {
            tr("1 Datei committen").to_owned()
        } else {
            trf("{n} Dateien committen", &[("n", &staged)])
        };
        let ready = missing.is_none() && !self.committing && self.identity.is_some();
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let button = ui
                .add_enabled(
                    ready,
                    egui::Button::new(label).min_size(egui::vec2(160.0, 26.0)),
                )
                .on_hover_text(ui.ctx().format_shortcut(&COMMIT_KEY));
            if self.committing {
                ui.add(egui::Spinner::new());
            }
            let key = ready && ui.input_mut(|i| i.consume_shortcut(&COMMIT_KEY));
            if button.clicked() || key {
                self.committing = true;
                action = Some(FormAction::Commit {
                    message: self.message(),
                    amend: self.amend,
                    no_verify: self.no_verify,
                });
            }
        });
        if let Some(why) = missing {
            ui.label(RichText::new(why).color(t.text_dim));
        }
        if let Some(note) = &self.note {
            ui.label(RichText::new(tr("Ausgabe der Hooks:")).color(t.text_dim));
            egui::ScrollArea::vertical()
                .id_salt("hook-output")
                .max_height(80.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(note).monospace().color(t.text_dim))
                });
        }
        action
    }

    fn identity_form(&mut self, ui: &mut egui::Ui, t: &Tokens) -> Option<FormAction> {
        let mut action = None;
        ui.label(RichText::new(tr("Wer committet?")).color(t.text_strong));
        ui.label(
            RichText::new(tr(
                "Git speichert Name und E-Mail-Adresse in jedem Commit. Sie sind noch nicht eingestellt.",
            ))
            .color(t.text_dim),
        );
        egui::Grid::new("identitaet").num_columns(2).show(ui, |ui| {
            let label = ui.label(tr("Name"));
            ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY))
                .labelled_by(label.id);
            ui.end_row();
            let label = ui.label(tr("E-Mail"));
            ui.add(egui::TextEdit::singleline(&mut self.email).desired_width(f32::INFINITY))
                .labelled_by(label.id);
            ui.end_row();
        });
        ui.radio_value(&mut self.global, true, tr("Für alle Repositories"));
        ui.radio_value(&mut self.global, false, tr("Nur für dieses Repository"));
        let ok = !self.name.trim().is_empty() && self.email.contains('@');
        if ui
            .add_enabled(ok, egui::Button::new(tr("Speichern")))
            .clicked()
        {
            action = Some(FormAction::SetIdentity {
                name: self.name.trim().to_owned(),
                email: self.email.trim().to_owned(),
                global: self.global,
            });
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_joins_summary_and_description() {
        let mut f = CommitForm {
            summary: "  Titel ".into(),
            ..CommitForm::default()
        };
        assert_eq!(f.message(), "Titel");
        f.body = "Zeile 1\n#2 bleibt\n\n".into();
        assert_eq!(f.message(), "Titel\n\nZeile 1\n#2 bleibt");
    }

    #[test]
    fn amend_fills_an_empty_form_only() {
        let mut f = CommitForm {
            amend: true,
            ..CommitForm::default()
        };
        f.set_amend_info(Some(("Alt\n\nText".into(), false)));
        assert_eq!((f.summary.as_str(), f.body.as_str()), ("Alt", "Text"));
        let mut g = CommitForm {
            amend: true,
            summary: "Neu".into(),
            ..CommitForm::default()
        };
        g.set_amend_info(Some(("Alt".into(), true)));
        assert_eq!(g.summary, "Neu");
    }
}
