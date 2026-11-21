//! The sidebar (docs/ux/layout.md): local branches, remote branches, tags and stashes with a
//! filter field. Selecting a branch or tag selects its commit in the history.

use egui::{Color32, RichText};
use gitcraft_git::{Oid, Ref, RefKind, Stash, Tracking, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// What the user picked in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pick {
    Commit(Oid),
    WorkInProgress,
    /// Switch to this local branch (double click or context menu).
    Checkout(String),
    /// A new branch starting at this ref; `suggested` prefills the name.
    NewBranch {
        start: String,
        label: String,
        suggested: String,
    },
    Rename(String),
    Delete(String),
    NewTag {
        target: String,
        label: String,
    },
    /// Compare HEAD with this ref.
    Compare {
        target: String,
        label: String,
    },
    DeleteTag(String),
    /// Show a stash like a commit.
    Stash(Stash),
    ApplyStash {
        stash: Stash,
        pop: bool,
    },
    DropStash(Stash),
}

/// Sidebar state that outlives a frame.
#[derive(Default)]
pub(crate) struct Sidebar {
    filter: String,
}

impl Sidebar {
    /// Draws the sidebar; returns what the user picked. `wip` is the number of changed files
    /// (an entry on top while there are any); `lane` gives a commit's lane colour, for the dot
    /// beside each branch.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        refs: &[Ref],
        stashes: &[Stash],
        wip: Option<usize>,
        lane: &dyn Fn(&Oid) -> Option<Color32>,
    ) -> Option<Pick> {
        let mut picked = None;
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text(tr("Filtern"))
                .desired_width(f32::INFINITY),
        )
        .on_hover_text(tr("Branches, Tags und Stashes filtern"));
        ui.add_space(4.0);
        if let Some(n) = wip {
            let resp = ui
                .horizontal(|ui| {
                    dot(ui, t.modified);
                    let r = ui.add(
                        egui::Button::selectable(false, RichText::new(tr("Offene Änderungen")))
                            .truncate(),
                    );
                    ui.label(RichText::new(n.to_string()).color(t.text_dim).size(12.0));
                    r
                })
                .inner;
            if resp.clicked() {
                picked = Some(Pick::WorkInProgress);
            }
            ui.add_space(2.0);
        }
        let needle = self.filter.to_lowercase();
        let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
        egui::ScrollArea::vertical()
            .id_salt("sidebar")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (kind, title, open) in [
                    (RefKind::Local, "Lokal", true),
                    (RefKind::Remote, "Remote", true),
                    (RefKind::Tag, "Tags", false),
                ] {
                    let items: Vec<&Ref> = refs
                        .iter()
                        .filter(|r| r.kind == kind && matches(&r.name))
                        .collect();
                    let heading = format!("{}  {}", tr(title).to_uppercase(), items.len());
                    egui::CollapsingHeader::new(RichText::new(heading).color(t.text_strong))
                        .id_salt(title)
                        .default_open(open)
                        .show(ui, |ui| {
                            for r in items {
                                let color = lane(&r.target).unwrap_or(t.text_dim);
                                let resp = ref_entry(ui, t, r, color);
                                if resp.double_clicked() && r.kind == RefKind::Local && !r.is_head {
                                    picked = Some(Pick::Checkout(r.name.clone()));
                                } else if resp.clicked() {
                                    picked = Some(Pick::Commit(r.target.clone()));
                                }
                                resp.context_menu(|ui| {
                                    if let Some(p) = ref_menu(ui, r) {
                                        picked = Some(p);
                                        ui.close();
                                    }
                                });
                            }
                        });
                }
                let stashes: Vec<&Stash> = stashes.iter().filter(|s| matches(&s.message)).collect();
                let heading = format!("{}  {}", tr("Stashes").to_uppercase(), stashes.len());
                egui::CollapsingHeader::new(RichText::new(heading).color(t.text_strong))
                    .id_salt("Stashes")
                    .default_open(false)
                    .show(ui, |ui| {
                        for s in stashes {
                            let resp = ui
                                .add(
                                    egui::Button::selectable(
                                        false,
                                        RichText::new(escape(&s.message)),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(format!(
                                    "{}\n{}",
                                    escape(&s.selector),
                                    tr("Doppelklick wendet ihn an")
                                ));
                            if resp.double_clicked() {
                                picked = Some(Pick::ApplyStash {
                                    stash: s.clone(),
                                    pop: false,
                                });
                            } else if resp.clicked() {
                                picked = Some(Pick::Stash(s.clone()));
                            }
                            resp.context_menu(|ui| {
                                if ui.button(tr("Anwenden")).clicked() {
                                    picked = Some(Pick::ApplyStash {
                                        stash: s.clone(),
                                        pop: false,
                                    });
                                    ui.close();
                                }
                                if ui.button(tr("Anwenden und entfernen (Pop)")).clicked() {
                                    picked = Some(Pick::ApplyStash {
                                        stash: s.clone(),
                                        pop: true,
                                    });
                                    ui.close();
                                }
                                if ui.button(tr("Löschen …")).clicked() {
                                    picked = Some(Pick::DropStash(s.clone()));
                                    ui.close();
                                }
                            });
                        }
                    });
            });
        picked
    }
}

/// The context menu of a branch or tag.
fn ref_menu(ui: &mut egui::Ui, r: &Ref) -> Option<Pick> {
    let mut picked = None;
    if r.kind == RefKind::Local && !r.is_head && ui.button(tr("Auschecken")).clicked() {
        picked = Some(Pick::Checkout(r.name.clone()));
    }
    if ui.button(tr("Branch hier anlegen …")).clicked() {
        // A remote branch suggests its own name without the remote.
        let suggested = match r.kind {
            RefKind::Remote => r.name.split_once('/').map_or("", |(_, n)| n).to_owned(),
            _ => String::new(),
        };
        picked = Some(Pick::NewBranch {
            start: r.full_name.clone(),
            label: r.name.clone(),
            suggested,
        });
    }
    if !r.is_head && ui.button(tr("Mit HEAD vergleichen")).clicked() {
        picked = Some(Pick::Compare {
            target: r.full_name.clone(),
            label: r.name.clone(),
        });
    }
    if ui.button(tr("Tag hier anlegen …")).clicked() {
        picked = Some(Pick::NewTag {
            target: r.full_name.clone(),
            label: r.name.clone(),
        });
    }
    if r.kind == RefKind::Tag && ui.button(tr("Löschen …")).clicked() {
        picked = Some(Pick::DeleteTag(r.name.clone()));
    }
    if r.kind == RefKind::Local {
        if ui.button(tr("Umbenennen …")).clicked() {
            picked = Some(Pick::Rename(r.name.clone()));
        }
        if !r.is_head && ui.button(tr("Löschen …")).clicked() {
            picked = Some(Pick::Delete(r.name.clone()));
        }
    }
    picked
}

fn ref_entry(ui: &mut egui::Ui, t: &Tokens, r: &Ref, color: Color32) -> egui::Response {
    let name = escape(&r.name);
    let mut text = RichText::new(&name);
    if r.is_head {
        text = text.color(t.accent).strong();
    }
    let track = match r.tracking {
        Some(Tracking::AheadBehind(0, 0)) | None => String::new(),
        Some(Tracking::AheadBehind(a, b)) => match (a, b) {
            (a, 0) => format!("+{a}"),
            (0, b) => format!("-{b}"),
            (a, b) => format!("+{a} -{b}"),
        },
        Some(Tracking::Gone) => tr("Upstream weg").to_owned(),
    };
    ui.horizontal(|ui| {
        dot(ui, color);
        let resp = ui.add(egui::Button::selectable(false, text).truncate());
        if !track.is_empty() {
            ui.label(RichText::new(&track).color(t.text_dim).size(12.0));
        }
        let hover = match (&r.upstream, r.tracking) {
            (Some(up), Some(Tracking::AheadBehind(a, b))) => trf(
                "{name}: {voraus} voraus, {zurueck} zurück gegenüber {upstream}",
                &[
                    ("name", &name),
                    ("voraus", &a),
                    ("zurueck", &b),
                    ("upstream", &escape(up)),
                ],
            ),
            (Some(up), Some(Tracking::Gone)) => trf(
                "{name}: Upstream {upstream} existiert nicht mehr",
                &[("name", &name), ("upstream", &escape(up))],
            ),
            _ => name.clone(),
        };
        resp.on_hover_text(hover)
    })
    .inner
}

/// A small disc in a lane or state colour before an entry.
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}
