//! The sidebar (docs/ux/layout.md): local branches, remote branches, tags and stashes with a
//! filter field. Selecting a branch or tag selects its commit in the history.

use egui::RichText;
use gitcraft_git::{Oid, Ref, RefKind, Stash, Tracking, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Sidebar state that outlives a frame.
#[derive(Default)]
pub(crate) struct Sidebar {
    filter: String,
}

impl Sidebar {
    /// Draws the sidebar; returns the commit to select when the user picks a ref.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        refs: &[Ref],
        stashes: &[Stash],
    ) -> Option<Oid> {
        let mut picked = None;
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text(tr("Filtern"))
                .desired_width(f32::INFINITY),
        )
        .on_hover_text(tr("Branches, Tags und Stashes filtern"));
        ui.add_space(4.0);
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
                    let heading = format!("{} {}", tr(title), items.len());
                    egui::CollapsingHeader::new(RichText::new(heading).color(t.text_strong))
                        .id_salt(title)
                        .default_open(open)
                        .show(ui, |ui| {
                            for r in items {
                                if ref_entry(ui, t, r).clicked() {
                                    picked = Some(r.target.clone());
                                }
                            }
                        });
                }
                let stashes: Vec<&Stash> = stashes.iter().filter(|s| matches(&s.message)).collect();
                let heading = format!("{} {}", tr("Stashes"), stashes.len());
                egui::CollapsingHeader::new(RichText::new(heading).color(t.text_strong))
                    .id_salt("Stashes")
                    .default_open(false)
                    .show(ui, |ui| {
                        for s in stashes {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(escape(&s.message)).color(t.text_dim),
                                )
                                .truncate(),
                            )
                            .on_hover_text(escape(&s.selector));
                        }
                    });
            });
        picked
    }
}

fn ref_entry(ui: &mut egui::Ui, t: &Tokens, r: &Ref) -> egui::Response {
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
