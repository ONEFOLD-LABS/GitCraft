//! Questions from git and ssh (askpass bridge, ADR-003, issue #42): user name, password or
//! passphrase (entered hidden), an unknown host key (fingerprint shown, explicit decision).
//! The answer goes to git only; the field is emptied right after.

use egui::RichText;
use gitcraft_core::askpass::{Prompt, PromptKind};
use gitcraft_git::escape;

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// The open question and what the user typed so far.
pub(crate) struct PromptDialog {
    pub(crate) prompt: Prompt,
    input: String,
    focus: bool,
}

impl PromptDialog {
    pub(crate) fn new(prompt: Prompt) -> PromptDialog {
        PromptDialog {
            prompt,
            input: String::new(),
            focus: true,
        }
    }

    /// `Some(answer)` when the user answered (`None` inside: refused).
    pub(crate) fn show(&mut self, ctx: &egui::Context, t: &Tokens) -> Option<Option<String>> {
        let mut done = None;
        let title = match self.prompt.kind {
            PromptKind::Username | PromptKind::Secret => tr("Anmeldung"),
            PromptKind::HostKey { .. } => tr("Unbekannter SSH-Host"),
            PromptKind::Confirm => tr("Rückfrage von Git"),
        };
        let modal = egui::Modal::new(egui::Id::unique("gitcraft-askpass")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(RichText::new(title).color(t.text_strong).size(16.0));
            ui.add_space(6.0);
            match &self.prompt.kind {
                PromptKind::HostKey { host, fingerprint } => {
                    ui.label(trf(
                        "ssh kennt den Schlüssel von {host} noch nicht. Vergleiche den Fingerabdruck mit dem, den der Anbieter veröffentlicht:",
                        &[("host", &escape(host))],
                    ));
                    ui.label(
                        RichText::new(escape(fingerprint))
                            .monospace()
                            .color(t.text_strong),
                    );
                    ui.label(
                        RichText::new(tr(
                            "Annehmen trägt ihn in known_hosts ein; danach fragt ssh nicht mehr. Stimmt er nicht, lehne ab: Jemand könnte sich als dieser Host ausgeben.",
                        ))
                        .color(t.text_dim),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(tr("Annehmen")).clicked() {
                            done = Some(Some("yes".to_owned()));
                        }
                        if ui.button(tr("Ablehnen")).clicked() {
                            done = Some(Some("no".to_owned()));
                        }
                    });
                }
                PromptKind::Confirm => {
                    ui.label(escape(self.prompt.text.trim()));
                    ui.horizontal(|ui| {
                        if ui.button(tr("Ja")).clicked() {
                            done = Some(Some("yes".to_owned()));
                        }
                        if ui.button(tr("Nein")).clicked() {
                            done = Some(Some("no".to_owned()));
                        }
                    });
                }
                PromptKind::Username | PromptKind::Secret => {
                    let secret = self.prompt.kind == PromptKind::Secret;
                    let l = ui.label(escape(self.prompt.text.trim()));
                    let field = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.input)
                                .password(secret)
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(l.id);
                    if std::mem::take(&mut self.focus) {
                        field.request_focus();
                    }
                    ui.label(
                        RichText::new(tr(
                            "Git fragt danach. GitCraft gibt die Eingabe nur an Git weiter und speichert sie nicht; dauerhaft merkt sie sich ein Credential-Helper.",
                        ))
                        .color(t.text_dim),
                    );
                    let enter =
                        field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button(tr("OK")).clicked() || enter {
                            done = Some(Some(std::mem::take(&mut self.input)));
                        }
                        if ui.button(tr("Abbrechen")).clicked() {
                            done = Some(None);
                        }
                    });
                }
            }
        });
        if done.is_none() && modal.should_close() {
            done = Some(None);
        }
        if done.is_some() {
            self.input.clear();
        }
        done
    }
}
