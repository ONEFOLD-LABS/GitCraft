//! Comparing two refs (issue #39): shown in the centre in place of the history. Commits only
//! one side has, and the files from the merge base to the second side; a click on a commit
//! shows its details, a click on a file its diff.

use egui::RichText;
use gitcraft_git::{ChangedFile, Commit, Comparison, FileStatus, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

pub(crate) struct CompareView {
    pub(crate) a: String,
    pub(crate) b: String,
    pub(crate) a_label: String,
    pub(crate) b_label: String,
    /// `None` while git works.
    pub(crate) result: Option<Result<Comparison, String>>,
}

pub(crate) enum CompareAction {
    Back,
    Commit(Box<Commit>),
    /// A file's diff from the merge base to `b`.
    File {
        base: String,
        file: ChangedFile,
    },
}

impl CompareView {
    pub(crate) fn show(&self, ui: &mut egui::Ui, t: &Tokens) -> Option<CompareAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            if ui
                .button(tr("Zurück zum Graphen"))
                .on_hover_text(tr("Esc"))
                .clicked()
            {
                action = Some(CompareAction::Back);
            }
            ui.label(
                RichText::new(trf(
                    "{a} verglichen mit {b}",
                    &[("a", &escape(&self.a_label)), ("b", &escape(&self.b_label))],
                ))
                .color(t.text_strong)
                .size(15.0),
            );
        });
        ui.separator();
        let cmp = match &self.result {
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new(tr("Vergleiche …")).color(t.text_dim));
                });
                return action;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(t.error_text));
                return action;
            }
            Some(Ok(c)) => c,
        };
        egui::ScrollArea::vertical()
            .id_salt("vergleich")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (title, commits) in [
                    (
                        trf("Nur in {name} ({n})", &[("name", &escape(&self.a_label)), ("n", &cmp.only_a.len())]),
                        &cmp.only_a,
                    ),
                    (
                        trf("Nur in {name} ({n})", &[("name", &escape(&self.b_label)), ("n", &cmp.only_b.len())]),
                        &cmp.only_b,
                    ),
                ] {
                    egui::CollapsingHeader::new(RichText::new(title).color(t.text_strong))
                        .id_salt(commits.as_ptr())
                        .default_open(true)
                        .show(ui, |ui| {
                            if commits.is_empty() {
                                ui.label(RichText::new(tr("Keine")).color(t.text_dim));
                            }
                            for c in commits.iter() {
                                let text = format!("{}  {}  ({})", c.id.short(), escape(&c.subject), escape(&c.author.name));
                                if ui.add(egui::Button::selectable(false, text).truncate()).clicked() {
                                    action = Some(CompareAction::Commit(Box::new(c.clone())));
                                }
                            }
                            if commits.len() >= gitcraft_git::COMPARE_LIMIT {
                                ui.label(
                                    RichText::new(trf("nur die neuesten {n}", &[("n", &gitcraft_git::COMPARE_LIMIT)]))
                                        .color(t.text_dim),
                                );
                            }
                        });
                }
                let Some(base) = &cmp.base else {
                    ui.label(
                        RichText::new(tr("Keine gemeinsame Geschichte: es gibt keine Basis für einen Vergleich der Dateien."))
                            .color(t.text_dim),
                    );
                    return;
                };
                let title = trf(
                    if cmp.files.len() == 1 {
                        "Was {b} seit der gemeinsamen Basis {basis} ändert (1 Datei)"
                    } else {
                        "Was {b} seit der gemeinsamen Basis {basis} ändert ({n} Dateien)"
                    },
                    &[("b", &escape(&self.b_label)), ("basis", &base.short()), ("n", &cmp.files.len())],
                );
                egui::CollapsingHeader::new(RichText::new(title).color(t.text_strong))
                    .id_salt("vergleich-dateien")
                    .default_open(true)
                    .show(ui, |ui| {
                        for f in &cmp.files {
                            let (letter, colour) = match f.status {
                                FileStatus::Added => ("A", t.added),
                                FileStatus::Deleted => ("D", t.deleted),
                                FileStatus::Renamed => ("R", t.renamed),
                                FileStatus::Modified => ("M", t.modified),
                            };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(letter).monospace().color(colour));
                                let path = match &f.old_path {
                                    Some(old) => format!("{} -> {}", old.display(), f.path.display()),
                                    None => f.path.display(),
                                };
                                if ui.add(egui::Button::selectable(false, path).truncate()).clicked() {
                                    action = Some(CompareAction::File {
                                        base: base.as_str().to_owned(),
                                        file: f.clone(),
                                    });
                                }
                                if let Some((a, d)) = f.lines {
                                    ui.label(RichText::new(format!("+{a} -{d}")).color(t.text_dim));
                                }
                            });
                        }
                    });
            });
        action
    }
}
