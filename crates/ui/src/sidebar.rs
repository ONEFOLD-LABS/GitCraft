//! The sidebar (docs/ux/layout.md): local branches, remote branches, tags and stashes with a
//! filter field. Selecting a branch or tag selects its commit in the history.

use egui::{Color32, RichText};
use gitcraft_git::{Oid, Ref, RefKind, Remote, Stash, Tracking, check_url, escape, redact_url};

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
    AddRemote,
    EditRemote(String),
    RenameRemote(String),
    RemoveRemote(String),
    MoveCredentials(String),
}

/// What the sidebar lists.
#[derive(Clone, Copy, Default)]
pub(crate) struct Lists<'a> {
    pub(crate) refs: &'a [Ref],
    pub(crate) remotes: &'a [Remote],
    pub(crate) stashes: &'a [Stash],
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
        lists: Lists<'_>,
        wip: Option<usize>,
        lane: &dyn Fn(&Oid) -> Option<Color32>,
    ) -> Option<Pick> {
        let Lists {
            refs,
            remotes,
            stashes,
        } = lists;
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
                    let count = if kind == RefKind::Remote {
                        remotes.len()
                    } else {
                        items.len()
                    };
                    let heading = format!("{}  {}", tr(title).to_uppercase(), count);
                    let section =
                        egui::CollapsingHeader::new(RichText::new(heading).color(t.text_strong))
                            .id_salt(title)
                            .default_open(open)
                            .show(ui, |ui| {
                                if kind != RefKind::Remote {
                                    if let Some(p) = ref_list(ui, t, items, lane, None) {
                                        picked = Some(p);
                                    }
                                    return;
                                }
                                for (remote, branches) in by_remote(remotes, items) {
                                    let Some(remote) = remote else {
                                        if let Some(p) = ref_list(ui, t, branches, lane, None) {
                                            picked = Some(p);
                                        }
                                        continue;
                                    };
                                    let header = remote_header(t, remote);
                                    let group = egui::CollapsingHeader::new(header)
                                        .id_salt(("remote", &remote.name))
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            ref_list(ui, t, branches, lane, Some(&remote.name))
                                        });
                                    if let Some(Some(p)) = group.body_returned {
                                        picked = Some(p);
                                    }
                                    let resp =
                                        group.header_response.on_hover_text(remote_hover(remote));
                                    resp.context_menu(|ui| {
                                        if let Some(p) = remote_menu(ui, remote) {
                                            picked = Some(p);
                                            ui.close();
                                        }
                                    });
                                }
                                if ui
                                    .add(egui::Button::new(tr("Remote hinzufügen …")).frame(false))
                                    .clicked()
                                {
                                    picked = Some(Pick::AddRemote);
                                }
                            });
                    if kind == RefKind::Remote {
                        section.header_response.context_menu(|ui| {
                            if ui.button(tr("Remote hinzufügen …")).clicked() {
                                picked = Some(Pick::AddRemote);
                                ui.close();
                            }
                        });
                    }
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

/// Entries of branches or tags; returns what was picked.
fn ref_list(
    ui: &mut egui::Ui,
    t: &Tokens,
    items: Vec<&Ref>,
    lane: &dyn Fn(&Oid) -> Option<Color32>,
    remote: Option<&str>,
) -> Option<Pick> {
    let mut picked = None;
    for r in items {
        let color = lane(&r.target).unwrap_or(t.text_dim);
        // Under its remote's heading a remote branch is shown without the remote's name.
        let shown = remote
            .and_then(|n| r.name.strip_prefix(n))
            .and_then(|tail| tail.strip_prefix('/'))
            .unwrap_or(&r.name);
        let resp = ref_entry(ui, t, r, shown, color);
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
    picked
}

/// Remote branches grouped by the remote they belong to (the longest remote name that prefixes
/// them, since names may contain `/`), remotes in configuration order; branches of no
/// configured remote come last under `None`.
fn by_remote<'a>(
    remotes: &'a [Remote],
    items: Vec<&'a Ref>,
) -> Vec<(Option<&'a Remote>, Vec<&'a Ref>)> {
    let mut groups: Vec<(Option<&Remote>, Vec<&Ref>)> =
        remotes.iter().map(|r| (Some(r), Vec::new())).collect();
    let mut rest = Vec::new();
    for r in items {
        let owner = remotes
            .iter()
            .enumerate()
            .filter(|(_, remote)| {
                r.name
                    .strip_prefix(remote.name.as_str())
                    .is_some_and(|tail| tail.starts_with('/'))
            })
            .max_by_key(|(_, remote)| remote.name.len())
            .map(|(i, _)| i);
        match owner {
            Some(i) => groups[i].1.push(r),
            None => rest.push(r),
        }
    }
    if !rest.is_empty() {
        groups.push((None, rest));
    }
    groups
}

/// A remote's name, in the warning colour when its URL carries credentials or does not
/// encrypt (the tooltip says which).
fn remote_header(t: &Tokens, remote: &Remote) -> RichText {
    let warn = remote.has_credentials() || !encrypted(remote);
    RichText::new(escape(&remote.name)).color(if warn { t.modified } else { t.text })
}

fn encrypted(remote: &Remote) -> bool {
    remote
        .urls
        .iter()
        .chain(&remote.push_urls)
        .all(|u| check_url(u).is_ok_and(|k| k.is_encrypted()))
}

fn remote_hover(remote: &Remote) -> String {
    let url = remote.url().map(redact_url).unwrap_or_default();
    let mut text = escape(&url);
    if let Some(push) = remote.push_urls.first() {
        text.push('\n');
        text.push_str(&trf("Push: {url}", &[("url", &escape(&redact_url(push)))]));
    }
    if remote.has_credentials() {
        text.push('\n');
        text.push_str(tr("Die URL enthält Zugangsdaten im Klartext."));
    }
    if !encrypted(remote) {
        text.push('\n');
        text.push_str(tr("Unverschlüsselte Verbindung."));
    }
    text
}

fn remote_menu(ui: &mut egui::Ui, remote: &Remote) -> Option<Pick> {
    let mut picked = None;
    if ui.button(tr("URL bearbeiten …")).clicked() {
        picked = Some(Pick::EditRemote(remote.name.clone()));
    }
    if ui.button(tr("Umbenennen …")).clicked() {
        picked = Some(Pick::RenameRemote(remote.name.clone()));
    }
    if remote.has_credentials()
        && ui
            .button(tr("Zugangsdaten in den Credential-Helper verschieben …"))
            .clicked()
    {
        picked = Some(Pick::MoveCredentials(remote.name.clone()));
    }
    if ui.button(tr("Entfernen …")).clicked() {
        picked = Some(Pick::RemoveRemote(remote.name.clone()));
    }
    picked
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

fn ref_entry(
    ui: &mut egui::Ui,
    t: &Tokens,
    r: &Ref,
    shown: &str,
    color: Color32,
) -> egui::Response {
    let name = escape(&r.name);
    let mut text = RichText::new(escape(shown));
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
