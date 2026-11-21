//! Dialogs for branches (issues #37, #38): create, rename, delete (naming the commits that
//! would be lost), check out a commit (detached HEAD, explained), leave a detached HEAD that
//! holds commits, and the files a switch would overwrite.

use egui::RichText;
use gitcraft_core::Command;
use gitcraft_git::{Loss, Oid, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Commits listed in a warning at most.
const LISTED: usize = 10;

/// Where a switch goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Branch(String),
    Commit(Oid),
}

impl Target {
    pub(crate) fn command(&self) -> Command {
        match self {
            Target::Branch(b) => Command::SwitchBranch(b.clone()),
            Target::Commit(id) => Command::SwitchDetached(id.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Dialog {
    Create {
        /// Commit id or ref the branch starts at.
        start: String,
        /// How the start is shown ("main", "a1b2c3d Fix parser").
        label: String,
        name: String,
        checkout: bool,
        focus: bool,
    },
    Rename {
        old: String,
        name: String,
        focus: bool,
    },
    Delete {
        name: String,
        /// `None` while it is being determined.
        loss: Option<Loss>,
    },
    /// Explains detached HEAD before checking out a commit.
    Detach { id: Oid, label: String },
    /// Leaving a detached HEAD: `loss` are the commits only it holds.
    Leave { target: Target, loss: Option<Loss> },
    /// A switch stopped because it would overwrite these files; `target` is where it went.
    Overwrite {
        files: Vec<String>,
        target: Option<Target>,
    },
    /// Park the changes.
    Stash {
        message: String,
        untracked: bool,
        keep_index: bool,
        focus: bool,
    },
    /// Drop a stash (asks; its id stays for restoring).
    DropStash {
        selector: String,
        id: Oid,
        message: String,
    },
}

/// What the user decided.
#[derive(Debug, Clone)]
pub(crate) enum Decision {
    /// Keep the dialog open.
    Open,
    Close,
    Run(Command),
    /// Several writes in this order (e.g. stash, then switch).
    RunAll(Vec<Command>),
    /// Check out a commit (after the explanation); the window checks for a detached HEAD
    /// that would be left first.
    Switch(Target),
    /// Offer a branch at this commit instead (from the detached-HEAD warning).
    CreateAt(Oid),
}

/// A quick check while typing; git checks the full rules when the command runs.
pub(crate) fn name_hint(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("Der Name fehlt.")
    } else if name.starts_with('-') {
        Some("Ein Name darf nicht mit „-“ beginnen.")
    } else if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Some("Ein Name darf keine Leerzeichen enthalten.")
    } else {
        None
    }
}

fn loss_list(ui: &mut egui::Ui, t: &Tokens, loss: &Loss) {
    for c in loss.commits.iter().take(LISTED) {
        ui.label(RichText::new(format!("{}  {}", c.id.short(), escape(&c.subject))).monospace());
    }
    if loss.total > LISTED {
        ui.label(
            RichText::new(trf("und {n} weitere", &[("n", &(loss.total - LISTED))]))
                .color(t.text_dim),
        );
    }
}

fn danger(t: &Tokens, label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label.to_owned()).color(t.error_text)).fill(t.error_bg)
}

impl Dialog {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Dialog::Create { .. } => "Neuer Branch",
            Dialog::Rename { .. } => "Branch umbenennen",
            Dialog::Delete { .. } => "Branch löschen?",
            Dialog::Detach { .. } => "Commit auschecken?",
            Dialog::Leave { .. } => "Commits würden unerreichbar",
            Dialog::Overwrite { .. } => "Wechsel nicht möglich",
            Dialog::Stash { .. } => "Änderungen parken (Stash)",
            Dialog::DropStash { .. } => "Stash löschen?",
        }
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context, t: &Tokens) -> Decision {
        let mut decision = Decision::Open;
        let title = self.title();
        let modal = egui::Modal::new(egui::Id::unique("gitcraft-branch")).show(ctx, |ui| {
            ui.set_width(480.0);
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
            Dialog::Create {
                start,
                label,
                name,
                checkout,
                focus,
            } => {
                ui.label(trf("Beginnt bei {start}.", &[("start", &escape(label))]));
                let l = ui.label(tr("Name"));
                let field = ui
                    .add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY))
                    .labelled_by(l.id);
                if std::mem::take(focus) {
                    field.request_focus();
                }
                ui.checkbox(checkout, tr("Danach auschecken"));
                let hint = name_hint(name.trim());
                if let Some(h) = hint {
                    ui.label(RichText::new(tr(h)).color(t.text_dim));
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let ok = ui.add_enabled(hint.is_none(), egui::Button::new(tr("Anlegen")));
                    if hint.is_none() && (ok.clicked() || enter) {
                        decision = Decision::Run(Command::CreateBranch {
                            name: name.trim().to_owned(),
                            start: start.clone(),
                            checkout: *checkout,
                        });
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Rename { old, name, focus } => {
                ui.label(trf("Neuer Name für {name}:", &[("name", &escape(old))]));
                let l = ui.label(tr("Name"));
                let field = ui
                    .add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY))
                    .labelled_by(l.id);
                if std::mem::take(focus) {
                    field.request_focus();
                }
                let hint = name_hint(name.trim())
                    .or((name.trim() == old).then_some("Der Name ist unverändert."));
                if let Some(h) = hint {
                    ui.label(RichText::new(tr(h)).color(t.text_dim));
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let ok = ui.add_enabled(hint.is_none(), egui::Button::new(tr("Umbenennen")));
                    if hint.is_none() && (ok.clicked() || enter) {
                        decision = Decision::Run(Command::RenameBranch {
                            old: old.clone(),
                            new: name.trim().to_owned(),
                        });
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Delete { name, loss } => {
                let name_shown = escape(name);
                match loss {
                    None => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(tr("Prüfe, welche Commits nur auf diesem Branch liegen …"));
                        });
                    }
                    Some(l) if l.total == 0 => {
                        ui.label(trf(
                            "Löscht den Branch {name}. Seine Commits bleiben auf anderen Branches erhalten.",
                            &[("name", &name_shown)],
                        ));
                    }
                    Some(l) => {
                        ui.label(trf(
                            if l.total == 1 {
                                "Löscht den Branch {name} mit 1 Commit, der auf keinem anderen Branch liegt:"
                            } else {
                                "Löscht den Branch {name} mit {n} Commits, die auf keinem anderen Branch liegen:"
                            },
                            &[("name", &name_shown), ("n", &l.total)],
                        ));
                        loss_list(ui, t, l);
                        ui.label(
                            RichText::new(tr(
                                "GitCraft merkt sich die Spitze; „Wiederherstellen“ legt den Branch danach wieder an.",
                            ))
                            .color(t.text_dim),
                        );
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(loss.is_some(), danger(t, tr("Löschen")))
                        .clicked()
                    {
                        decision = Decision::Run(Command::DeleteBranch(name.clone()));
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Detach { id, label } => {
                ui.label(trf(
                    "Checkt {commit} aus, ohne Branch.",
                    &[("commit", &escape(label))],
                ));
                ui.label(
                    RichText::new(tr(
                        "Du siehst dann diesen Stand („losgelöster HEAD“). Neue Commits gehören zu keinem Branch, bis du einen anlegst.",
                    ))
                    .color(t.text_dim),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Auschecken")).clicked() {
                        decision = Decision::Switch(Target::Commit(id.clone()));
                    }
                    if ui.button(tr("Branch hier anlegen …")).clicked() {
                        decision = Decision::CreateAt(id.clone());
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Leave { target, loss } => {
                match loss {
                    None => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(tr("Prüfe, ob Commits verloren gingen …"));
                        });
                    }
                    Some(l) => {
                        ui.label(trf(
                            if l.total == 1 {
                                "HEAD ist losgelöst und hält 1 Commit, der auf keinem Branch liegt. Nach dem Wechsel ist er nur noch über seine ID erreichbar:"
                            } else {
                                "HEAD ist losgelöst und hält {n} Commits, die auf keinem Branch liegen. Nach dem Wechsel sind sie nur noch über ihre ID erreichbar:"
                            },
                            &[("n", &l.total)],
                        ));
                        loss_list(ui, t, l);
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if let Some(first) = loss.as_ref().and_then(|l| l.commits.first())
                        && ui.button(tr("Branch hier anlegen …")).clicked()
                    {
                        decision = Decision::CreateAt(first.id.clone());
                    }
                    if ui
                        .add_enabled(loss.is_some(), danger(t, tr("Trotzdem wechseln")))
                        .clicked()
                    {
                        decision = Decision::Run(target.command());
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Stash {
                message,
                untracked,
                keep_index,
                focus,
            } => {
                ui.label(tr(
                    "Legt die Änderungen beiseite; der Arbeitsstand ist danach wieder der letzte Commit.",
                ));
                let l = ui.label(tr("Nachricht (optional)"));
                let field = ui
                    .add(egui::TextEdit::singleline(message).desired_width(f32::INFINITY))
                    .labelled_by(l.id);
                if std::mem::take(focus) {
                    field.request_focus();
                }
                ui.checkbox(untracked, tr("Unversionierte Dateien mitnehmen"));
                ui.checkbox(keep_index, tr("Vorgemerkte Änderungen dalassen"))
                    .on_hover_text("--keep-index");
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button(tr("Stashen")).clicked() || enter {
                        decision = Decision::Run(Command::StashPush {
                            message: message.trim().to_owned(),
                            untracked: *untracked,
                            keep_index: *keep_index,
                        });
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::DropStash {
                selector,
                id,
                message,
            } => {
                ui.label(trf(
                    "Löscht den Stash {stash}.",
                    &[("stash", &escape(message))],
                ));
                ui.label(
                    RichText::new(trf(
                        "Seine ID {id} bleibt im Hinweis danach; „Wiederherstellen“ legt ihn wieder an.",
                        &[("id", &id.short())],
                    ))
                    .color(t.text_dim),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(danger(t, tr("Löschen"))).clicked() {
                        decision = Decision::Run(Command::StashDrop {
                            selector: selector.clone(),
                            id: id.clone(),
                            message: message.clone(),
                        });
                    }
                    if ui.button(tr("Abbrechen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
            Dialog::Overwrite { files, target } => {
                ui.label(tr(
                    "Der Wechsel würde lokale Änderungen an diesen Dateien überschreiben; es wurde nichts geändert:",
                ));
                for f in files.iter().take(LISTED) {
                    ui.label(RichText::new(escape(f)).monospace());
                }
                if files.len() > LISTED {
                    ui.label(trf("und {n} weitere", &[("n", &(files.len() - LISTED))]));
                }
                ui.label(
                    RichText::new(tr(
                        "Committe die Änderungen, lege sie in einen Stash oder verwirf sie, dann wechsle erneut.",
                    ))
                    .color(t.text_dim),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if let Some(target) = target
                        && ui.button(tr("Stashen und wechseln")).clicked()
                    {
                        decision = Decision::RunAll(vec![
                            Command::StashPush {
                                message: tr("Vor dem Wechsel").to_owned(),
                                untracked: false,
                                keep_index: false,
                            },
                            target.command(),
                        ]);
                    }
                    if ui.button(tr("Schließen")).clicked() {
                        decision = Decision::Close;
                    }
                });
            }
        }
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_while_typing() {
        assert_eq!(name_hint("feature/x"), None);
        assert!(name_hint("").is_some());
        assert!(name_hint("-rf").is_some());
        assert!(name_hint("a b").is_some());
        assert!(name_hint("a\tb").is_some());
    }
}
