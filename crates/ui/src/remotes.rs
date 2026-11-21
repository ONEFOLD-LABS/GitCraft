//! Dialogs for remotes (issue #45): add, edit URLs, rename, remove (with a way back), and
//! moving a password out of a URL into git's credential helper. URLs are checked while typing;
//! unencrypted ones (`http`, `git://`) are accepted with a warning.

use egui::RichText;
use gitcraft_core::Command;
use gitcraft_git::PullMode;
use gitcraft_git::{UrlProblem, check_url, escape, redact_url};

use crate::branches::{Decision, name_hint};
use crate::i18n::{tr, trf};
use crate::theme::Tokens;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Dialog {
    /// Add a remote, or (with `old`) edit the URLs of an existing one.
    Edit {
        old: Option<String>,
        name: String,
        url: String,
        /// A separate push URL is used.
        separate: bool,
        push_url: String,
        focus: bool,
    },
    Rename {
        old: String,
        name: String,
        focus: bool,
    },
    Remove {
        name: String,
        url: String,
    },
    /// Move the password in the URL into the credential helper.
    MoveCredentials {
        name: String,
    },
    /// A fast-forward pull is not possible; merge or rebase must be chosen.
    Diverged {
        branch: String,
        upstream: String,
        ahead: u32,
        behind: u32,
    },
}

/// What is wrong with a URL, or a warning about it; `(text, blocks)`.
pub(crate) fn url_hint(url: &str) -> Option<(&'static str, bool)> {
    match check_url(url.trim()) {
        Ok(kind) if !kind.is_encrypted() => Some((
            "Unverschlüsselt: Daten (und Zugangsdaten) gehen im Klartext über das Netz.",
            false,
        )),
        Ok(_) => None,
        Err(problem) => Some((problem_text(&problem), true)),
    }
}

pub(crate) fn problem_text(problem: &UrlProblem) -> &'static str {
    match problem {
        UrlProblem::Empty => "Die URL fehlt.",
        UrlProblem::LeadingDash => "Eine URL (oder ihr Host) darf nicht mit „-“ beginnen.",
        UrlProblem::Control => "Die URL enthält Steuerzeichen.",
        UrlProblem::Helper => {
            "Transport-Helfer („ext::“, „fd::“ …) können beliebige Programme starten; GitCraft nimmt sie nicht an."
        }
        UrlProblem::Scheme(_) => "GitCraft nimmt nur https, ssh, git, file und lokale Pfade an.",
    }
}

impl Dialog {
    pub(crate) fn add() -> Dialog {
        Dialog::Edit {
            old: None,
            name: String::new(),
            url: String::new(),
            separate: false,
            push_url: String::new(),
            focus: true,
        }
    }

    pub(crate) fn title(&self) -> &'static str {
        match self {
            Dialog::Edit { old: None, .. } => "Remote hinzufügen",
            Dialog::Edit { .. } => "Remote bearbeiten",
            Dialog::Rename { .. } => "Remote umbenennen",
            Dialog::Remove { .. } => "Remote entfernen?",
            Dialog::MoveCredentials { .. } => "Zugangsdaten aus der URL nehmen",
            Dialog::Diverged { .. } => "Vorspulen nicht möglich",
        }
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context, t: &Tokens) -> Decision {
        let mut decision = Decision::Open;
        let title = self.title();
        let modal = egui::Modal::new(egui::Id::unique("gitcraft-remote")).show(ctx, |ui| {
            ui.set_width(500.0);
            ui.label(RichText::new(tr(title)).color(t.text_strong).size(16.0));
            ui.add_space(6.0);
            decision = self.body(ui, t);
        });
        if matches!(decision, Decision::Open) && modal.should_close() {
            decision = Decision::Close;
        }
        decision
    }

    fn body(&mut self, ui: &mut egui::Ui, t: &Tokens) -> Decision {
        let mut decision = Decision::Open;
        match self {
            Dialog::Edit {
                old,
                name,
                url,
                separate,
                push_url,
                focus,
            } => {
                let mut first = None;
                if old.is_none() {
                    let l = ui.label(tr("Name"));
                    let field = ui
                        .add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY))
                        .labelled_by(l.id);
                    first = Some(field);
                }
                let l = ui.label(tr("URL"));
                let url_field = ui
                    .add(egui::TextEdit::singleline(url).desired_width(f32::INFINITY))
                    .labelled_by(l.id);
                if std::mem::take(focus) {
                    first.as_ref().unwrap_or(&url_field).request_focus();
                }
                ui.checkbox(separate, tr("Eigene URL zum Hochladen (Push)"));
                let mut push_field = None;
                if *separate {
                    let l = ui.label(tr("Push-URL"));
                    push_field = Some(
                        ui.add(egui::TextEdit::singleline(push_url).desired_width(f32::INFINITY))
                            .labelled_by(l.id),
                    );
                }
                let mut blocked = false;
                if old.is_none()
                    && let Some(h) = name_hint(name.trim())
                {
                    ui.label(RichText::new(tr(h)).color(t.text_dim));
                    blocked = true;
                }
                let mut hints = vec![url_hint(url)];
                if *separate {
                    hints.push(url_hint(push_url));
                }
                for (text, blocks) in hints.into_iter().flatten() {
                    let color = if blocks { t.text_dim } else { t.modified };
                    ui.label(RichText::new(tr(text)).color(color));
                    blocked |= blocks;
                }
                let enter = [first.as_ref(), Some(&url_field), push_field.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|f| f.lost_focus())
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let label = if old.is_none() {
                        tr("Hinzufügen")
                    } else {
                        tr("Speichern")
                    };
                    let ok = ui.add_enabled(!blocked, egui::Button::new(label));
                    if !blocked && (ok.clicked() || enter) {
                        let push = separate.then(|| push_url.trim().to_owned());
                        let url = url.trim().to_owned();
                        decision = Decision::Run(match old {
                            None => Command::AddRemote {
                                name: name.trim().to_owned(),
                                url,
                                push_url: push,
                            },
                            Some(old) => Command::SetRemoteUrls {
                                name: old.clone(),
                                url,
                                push_url: push,
                            },
                        });
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Rename { old, name, focus } => {
                ui.label(trf("Remote {name}", &[("name", &escape(old))]));
                let l = ui.label(tr("Neuer Name"));
                let field = ui
                    .add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY))
                    .labelled_by(l.id);
                if std::mem::take(focus) {
                    field.request_focus();
                }
                ui.label(
                    RichText::new(tr(
                        "Die Remote-Branches und die Upstreams lokaler Branches ziehen mit um.",
                    ))
                    .color(t.text_dim),
                );
                let trimmed = name.trim();
                let hint =
                    name_hint(trimmed).or((trimmed == old).then_some("Der Name ist unverändert."));
                if let Some(h) = hint {
                    ui.label(RichText::new(tr(h)).color(t.text_dim));
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let ok = ui.add_enabled(hint.is_none(), egui::Button::new(tr("Umbenennen")));
                    if hint.is_none() && (ok.clicked() || enter) {
                        decision = Decision::Run(Command::RenameRemote {
                            old: old.clone(),
                            new: trimmed.to_owned(),
                        });
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Remove { name, url } => {
                ui.label(trf(
                    "Entfernt das Remote {name} ({url}) und seine Remote-Branches. Lokale Branches und Commits bleiben; ihr Upstream fällt weg.",
                    &[("name", &escape(name)), ("url", &escape(&redact_url(url)))],
                ));
                ui.label(
                    RichText::new(tr(
                        "Danach fügt „Wiederherstellen“ in der Meldung es wieder hinzu.",
                    ))
                    .color(t.text_dim),
                );
                ui.horizontal(|ui| {
                    let danger =
                        egui::Button::new(RichText::new(tr("Entfernen")).color(t.error_text))
                            .fill(t.error_bg);
                    if ui.add(danger).clicked() {
                        decision = Decision::Run(Command::RemoveRemote(name.clone()));
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::MoveCredentials { name } => {
                ui.label(trf(
                    "Die URL von {name} enthält ein Passwort oder Token im Klartext: Es steht in der Konfigurationsdatei und in jeder Kopie davon.",
                    &[("name", &escape(name))],
                ));
                ui.label(tr(
                    "GitCraft übergibt es dem Credential-Helper von Git (zum Beispiel dem Schlüsselbund des Systems) und entfernt es aus der URL. Ohne eingerichteten Helper ändert sich nichts.",
                ));
                ui.horizontal(|ui| {
                    if ui.button(tr("Verschieben")).clicked() {
                        decision = Decision::Run(Command::MoveCredentials(name.clone()));
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Diverged {
                branch,
                upstream,
                ahead,
                behind,
            } => {
                let (branch, upstream) = (escape(branch), escape(upstream));
                ui.label(trf(
                    "{branch} und {upstream} sind auseinandergelaufen (eigene Commits: {eigene}, neue vom Remote: {neue}). Nichts wurde geändert.",
                    &[
                        ("branch", &branch),
                        ("upstream", &upstream),
                        ("eigene", ahead),
                        ("neue", behind),
                    ],
                ));
                ui.add_space(4.0);
                ui.label(trf(
                    "Mergen: ein Merge-Commit verbindet beide Seiten (git pull --no-rebase). Rebasen: deine Commits werden auf {upstream} neu aufgesetzt und bekommen neue IDs (git pull --rebase).",
                    &[("upstream", &upstream)],
                ));
                ui.horizontal(|ui| {
                    if ui.button(tr("Mergen")).clicked() {
                        decision = Decision::Run(Command::Pull(PullMode::Merge));
                    }
                    if ui.button(tr("Rebasen")).clicked() {
                        decision = Decision::Run(Command::Pull(PullMode::Rebase));
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
        }
        decision
    }
}
