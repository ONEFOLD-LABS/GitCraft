//! Dialogs for remotes (issue #45): add, edit URLs, rename, remove (with a way back), and
//! moving a password out of a URL into git's credential helper. URLs are checked while typing;
//! unencrypted ones (`http`, `git://`) are accepted with a warning.

use egui::RichText;
use gitcraft_core::Command;
use gitcraft_git::{Loss, Oid, PullMode};
use gitcraft_git::{UrlProblem, check_url, escape, redact_url};

use crate::branches::{Decision, danger, loss_list, name_hint};
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
    /// The first push of a branch: to which remote (its upstream afterwards).
    FirstPush {
        branch: String,
        remotes: Vec<String>,
        chosen: String,
    },
    /// The remote refused a push because it has commits the branch lacks.
    Rejected {
        branch: String,
    },
    /// A forced push: what it removes from the remote; `None` while that is read.
    Force {
        info: Option<(String, Option<Oid>, Loss)>,
    },
    /// What a local branch tracks: `choices` are (full ref name, short name) of the remote
    /// branches.
    Upstream {
        branch: String,
        current: Option<String>,
        choices: Vec<(String, String)>,
        chosen: usize,
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
            Dialog::FirstPush { .. } => "Branch zum ersten Mal hochladen",
            Dialog::Upstream { .. } => "Upstream festlegen",
            Dialog::Rejected { .. } => "Push abgelehnt",
            Dialog::Force { .. } => "Force-Push?",
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
            Dialog::Upstream {
                branch,
                current,
                choices,
                chosen,
            } => {
                ui.label(match current {
                    Some(up) => trf(
                        "{branch} folgt {upstream}. Pull holt von dort, Push lädt dorthin hoch, und die Zahlen voraus/zurück beziehen sich darauf.",
                        &[("branch", &escape(branch)), ("upstream", &escape(up))],
                    ),
                    None => trf(
                        "{branch} folgt keinem Remote-Branch. Mit einem Upstream wissen Pull und Push, wohin.",
                        &[("branch", &escape(branch))],
                    ),
                });
                if choices.is_empty() {
                    ui.label(
                        RichText::new(tr(
                            "Es gibt keine Remote-Branches; hole sie zuerst (Fetch).",
                        ))
                        .color(t.text_dim),
                    );
                } else {
                    let l = ui.label(tr("Remote-Branch"));
                    let shown = choices
                        .get(*chosen)
                        .map(|(_, short)| escape(short))
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("upstream-choice")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (i, (_, short)) in choices.iter().enumerate() {
                                ui.selectable_value(chosen, i, escape(short));
                            }
                        })
                        .response
                        .labelled_by(l.id);
                }
                ui.horizontal(|ui| {
                    if let Some((full, _)) = choices.get(*chosen)
                        && ui.button(tr("Festlegen")).clicked()
                    {
                        decision = Decision::Run(Command::SetUpstream {
                            branch: branch.clone(),
                            upstream: full.clone(),
                        });
                    }
                    if current.is_some() && ui.button(tr("Upstream entfernen")).clicked() {
                        decision = Decision::Run(Command::UnsetUpstream(branch.clone()));
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::FirstPush {
                branch,
                remotes,
                chosen,
            } => {
                ui.label(trf(
                    "{branch} gibt es auf dem Remote noch nicht. Der Push legt ihn dort an und setzt ihn als Upstream, so dass Pull und Push danach wissen, wohin.",
                    &[("branch", &escape(branch))],
                ));
                if remotes.len() > 1 {
                    let l = ui.label(tr("Remote"));
                    egui::ComboBox::from_id_salt("push-remote")
                        .selected_text(escape(chosen))
                        .show_ui(ui, |ui| {
                            for r in remotes.iter() {
                                ui.selectable_value(chosen, r.clone(), escape(r));
                            }
                        })
                        .response
                        .labelled_by(l.id);
                }
                ui.horizontal(|ui| {
                    let label = trf(
                        "Nach {remote}/{branch} pushen",
                        &[("remote", &escape(chosen)), ("branch", &escape(branch))],
                    );
                    if ui.button(label).clicked() {
                        decision = Decision::Run(Command::Push {
                            remote: Some(chosen.clone()),
                            lease: None,
                        });
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Rejected { branch } => {
                ui.label(trf(
                    "Der Remote hat Commits auf {branch}, die dir lokal fehlen; ein normaler Push würde sie überschreiben und wurde abgelehnt. Nichts wurde geändert.",
                    &[("branch", &escape(branch))],
                ));
                ui.label(
                    RichText::new(tr(
                        "Hole sie zuerst mit Pull und pushe dann noch einmal. Ein Force-Push ersetzt den Stand auf dem Remote durch deinen.",
                    ))
                    .color(t.text_dim),
                );
                ui.horizontal(|ui| {
                    if ui.button(tr("Pull")).clicked() {
                        decision = Decision::Run(Command::Pull(PullMode::FastForward));
                    }
                    if ui.button(tr("Force-Push …")).clicked() {
                        decision = Decision::Run(Command::PushInfo(None));
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Force { info } => {
                let Some((target, expected, loss)) = info else {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(tr("Prüfe, was auf dem Remote verloren ginge …"));
                    });
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                    return decision;
                };
                if loss.total == 0 {
                    ui.label(trf(
                        "Ersetzt {ziel} durch deinen Stand. Nach dem letzten Fetch geht dort kein Commit verloren.",
                        &[("ziel", &escape(target))],
                    ));
                } else {
                    let text = if loss.total == 1 {
                        trf(
                            "Ersetzt {ziel} durch deinen Stand. Dieser Commit verschwindet vom Remote:",
                            &[("ziel", &escape(target))],
                        )
                    } else {
                        trf(
                            "Ersetzt {ziel} durch deinen Stand. Diese {n} Commits verschwinden vom Remote:",
                            &[("ziel", &escape(target)), ("n", &loss.total)],
                        )
                    };
                    ui.label(text);
                    loss_list(ui, t, loss);
                }
                let lease = match expected {
                    Some(id) => trf(
                        "Nur solange der Remote noch bei {id} steht: Kam seitdem etwas dazu, bricht der Push ab, ohne etwas zu überschreiben.",
                        &[("id", &id.short())],
                    ),
                    None => tr(
                        "Nur solange es den Branch auf dem Remote nicht gibt: Entsteht er seitdem, bricht der Push ab.",
                    )
                    .to_owned(),
                };
                ui.label(RichText::new(lease).color(t.text_dim));
                ui.horizontal(|ui| {
                    if ui.add(danger(t, tr("Überschreiben"))).clicked() {
                        decision = Decision::Run(Command::Push {
                            remote: None,
                            lease: Some(expected.clone()),
                        });
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
