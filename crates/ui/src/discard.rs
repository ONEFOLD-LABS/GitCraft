//! Confirmation before discarding or deleting, and the "Zuletzt verworfen" window (issues #24,
//! #25). The confirmation names what is lost; nothing destructive runs without it.

use egui::RichText;
use gitcraft_core::Command;
use gitcraft_git::{Discarded, FileDiff, RepoPath};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Paths listed in a confirmation at most.
const LISTED: usize = 10;

/// What waits for confirmation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pending {
    /// Unstaged changes of tracked files.
    Files(Vec<RepoPath>),
    /// Some lines of one file's unstaged diff.
    Lines {
        path: RepoPath,
        shown: Box<FileDiff>,
        lines: Vec<(usize, usize)>,
    },
    /// Untracked files.
    Delete(Vec<RepoPath>),
}

/// The confirmation dialog's state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Confirm {
    pub(crate) pending: Pending,
    /// Changed lines of the files, once counted.
    pub(crate) lines: Option<u64>,
}

impl Confirm {
    pub(crate) fn new(pending: Pending) -> Confirm {
        let lines = match &pending {
            Pending::Lines { lines, .. } => Some(lines.len() as u64),
            _ => None,
        };
        Confirm { pending, lines }
    }

    /// The command that runs once confirmed.
    pub(crate) fn command(&self) -> Command {
        match &self.pending {
            Pending::Files(p) => Command::Discard(p.clone()),
            Pending::Lines { path, shown, lines } => Command::DiscardLines {
                path: path.clone(),
                shown: shown.clone(),
                lines: lines.clone(),
            },
            Pending::Delete(p) => Command::DeleteUntracked(p.clone()),
        }
    }

    /// Title, what happens, and the button's label.
    pub(crate) fn texts(&self) -> (String, String, String) {
        let count = |n: usize, one: &'static str, many: &'static str| {
            if n == 1 {
                tr(one).to_owned()
            } else {
                trf(many, &[("n", &n)])
            }
        };
        let lines = match self.lines {
            None => tr("… Zeilen").to_owned(),
            Some(1) => tr("1 Zeile").to_owned(),
            Some(n) => trf("{n} Zeilen", &[("n", &n)]),
        };
        match &self.pending {
            Pending::Files(p) => (
                tr("Änderungen verwerfen?").to_owned(),
                trf(
                    if p.len() == 1 {
                        "Verwirft die Änderungen an 1 Datei ({zeilen})."
                    } else {
                        "Verwirft die Änderungen an {n} Dateien ({zeilen})."
                    },
                    &[("n", &p.len()), ("zeilen", &lines)],
                ),
                tr("Verwerfen").to_owned(),
            ),
            Pending::Lines { path, lines: l, .. } => (
                tr("Zeilen verwerfen?").to_owned(),
                if l.len() == 1 {
                    trf(
                        "Verwirft 1 geänderte Zeile in {datei}.",
                        &[("datei", &path.display())],
                    )
                } else {
                    trf(
                        "Verwirft {n} geänderte Zeilen in {datei}.",
                        &[("n", &l.len()), ("datei", &path.display())],
                    )
                },
                tr("Verwerfen").to_owned(),
            ),
            Pending::Delete(p) => (
                tr("Dateien löschen?").to_owned(),
                count(
                    p.len(),
                    "Löscht 1 unversionierte Datei.",
                    "Löscht {n} unversionierte Dateien.",
                ),
                tr("Löschen").to_owned(),
            ),
        }
    }

    fn paths(&self) -> Vec<String> {
        match &self.pending {
            Pending::Files(p) | Pending::Delete(p) => p.iter().map(RepoPath::display).collect(),
            Pending::Lines { .. } => Vec::new(),
        }
    }

    /// Draws the dialog: `Some(true)` confirmed, `Some(false)` cancelled.
    pub(crate) fn show(&self, ctx: &egui::Context, t: &Tokens, days: u32) -> Option<bool> {
        let (title, body, button) = self.texts();
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("gitcraft-verwerfen")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(RichText::new(title).color(t.text_strong).size(16.0));
            ui.add_space(4.0);
            ui.label(body);
            let paths = self.paths();
            if !paths.is_empty() {
                ui.add_space(4.0);
                for p in paths.iter().take(LISTED) {
                    ui.label(RichText::new(p).monospace());
                }
                if paths.len() > LISTED {
                    ui.label(trf("und {n} weitere", &[("n", &(paths.len() - LISTED))]));
                }
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(trf(
                    "GitCraft sichert die Inhalte vorher; unter „Zuletzt verworfen“ lassen sie sich {tage} Tage lang wiederherstellen.",
                    &[("tage", &days)],
                ))
                .color(t.text_dim),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let danger = egui::Button::new(RichText::new(button).color(t.error_text))
                    .fill(t.error_bg);
                if ui.add(danger).clicked() {
                    choice = Some(true);
                }
                if ui.button(tr("Abbrechen")).clicked() {
                    choice = Some(false);
                }
            });
        });
        if choice.is_none() && modal.should_close() {
            choice = Some(false);
        }
        choice
    }
}

/// The "Zuletzt verworfen" window; returns the entry to restore.
pub(crate) fn discarded_window(
    ctx: &egui::Context,
    open: &mut bool,
    t: &Tokens,
    list: &[Discarded],
    days: u32,
) -> Option<String> {
    let mut restore = None;
    egui::Window::new(tr("Zuletzt verworfen"))
        .open(open)
        .collapsible(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
        .show(ctx, |ui| {
            ui.label(
                RichText::new(trf(
                    "Verworfene und gelöschte Inhalte der letzten {tage} Tage.",
                    &[("tage", &days)],
                ))
                .color(t.text_dim),
            );
            if list.is_empty() {
                ui.label(RichText::new(tr("Keine")).color(t.text_dim));
            }
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    for d in list {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(crate::history::absolute_date(d.time))
                                    .color(t.text_strong),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button(tr("Wiederherstellen")).clicked() {
                                        restore = Some(d.id.clone());
                                    }
                                },
                            );
                        });
                        let names: Vec<String> = d.files.iter().map(RepoPath::display).collect();
                        let gone: Vec<String> = d.absent.iter().map(RepoPath::display).collect();
                        for n in names.iter().take(LISTED) {
                            ui.label(RichText::new(n).monospace());
                        }
                        for n in gone.iter().take(LISTED) {
                            ui.label(
                                RichText::new(trf("{datei} (war gelöscht)", &[("datei", n)]))
                                    .monospace()
                                    .color(t.text_dim),
                            );
                        }
                        let more = (names.len() + gone.len()).saturating_sub(2 * LISTED);
                        if names.len() > LISTED || gone.len() > LISTED {
                            ui.label(trf("und {n} weitere", &[("n", &more.max(1))]));
                        }
                    }
                });
        });
    restore
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: &str) -> RepoPath {
        RepoPath::from_bytes(n.as_bytes())
    }

    #[test]
    fn confirmations_name_the_consequence() {
        let mut c = Confirm::new(Pending::Files(vec![p("a"), p("b"), p("c")]));
        assert_eq!(
            c.texts().1,
            "Verwirft die Änderungen an 3 Dateien (… Zeilen)."
        );
        c.lines = Some(42);
        assert_eq!(
            c.texts().1,
            "Verwirft die Änderungen an 3 Dateien (42 Zeilen)."
        );
        let one = Confirm {
            pending: Pending::Files(vec![p("a")]),
            lines: Some(1),
        };
        assert_eq!(
            one.texts().1,
            "Verwirft die Änderungen an 1 Datei (1 Zeile)."
        );
        let lines = Confirm::new(Pending::Lines {
            path: p("src/x.rs"),
            shown: Box::default(),
            lines: vec![(0, 1), (0, 2)],
        });
        assert_eq!(lines.texts().1, "Verwirft 2 geänderte Zeilen in src/x.rs.");
        let del = Confirm::new(Pending::Delete(vec![p("x\ny")]));
        let (title, body, button) = del.texts();
        assert_eq!(
            (title.as_str(), body.as_str(), button.as_str()),
            (
                "Dateien löschen?",
                "Löscht 1 unversionierte Datei.",
                "Löschen"
            )
        );
        assert_eq!(del.paths(), ["x\\ny"], "control characters shown escaped");
        assert!(matches!(del.command(), Command::DeleteUntracked(_)));
    }
}
