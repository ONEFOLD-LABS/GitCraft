//! Fetch and pull in the window (issue #43): the running operation with its progress and a
//! cancel button, and what came in afterwards.

use egui::RichText;
use gitcraft_core::OperationId;
use gitcraft_git::{Progress, RefUpdate, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Ref updates named in a notice at most.
const NAMED: usize = 5;

/// A fetch or pull that is running.
#[derive(Debug, Clone)]
pub(crate) struct Running {
    pub(crate) id: OperationId,
    /// What runs ("Fetch origin").
    pub(crate) title: String,
    pub(crate) progress: Option<Progress>,
}

impl Running {
    /// Title, progress bar and cancel button (in the toolbar, laid out right to left).
    /// Returns true when the user cancels.
    pub(crate) fn show(&self, ui: &mut egui::Ui, t: &Tokens) -> bool {
        let cancel = ui.button(tr("Abbrechen")).clicked();
        let (fraction, detail) = match &self.progress {
            Some(p) => {
                let phase = phase_text(p);
                let detail = match p.total {
                    Some(total) => format!("{phase} {}/{}", p.done, total),
                    None => format!("{phase} {}", p.done),
                };
                (p.fraction(), detail)
            }
            None => (None, tr("verbinde …").to_owned()),
        };
        let bar = match fraction {
            Some(f) => egui::ProgressBar::new(f),
            None => egui::ProgressBar::new(0.0).animate(true),
        };
        ui.add(bar.desired_width(140.0).desired_height(8.0))
            .on_hover_text(&detail);
        ui.label(RichText::new(format!("{} · {detail}", self.title)).color(t.text_dim));
        cancel
    }
}

/// git's phase names in the UI language; the server's own (possibly localised) names as they
/// are.
fn phase_text(p: &Progress) -> String {
    let known = match p.phase.as_str() {
        "Enumerating objects" => Some("Objekte aufzählen"),
        "Counting objects" => Some("Objekte zählen"),
        "Compressing objects" => Some("Objekte komprimieren"),
        "Receiving objects" => Some("Objekte empfangen"),
        "Unpacking objects" => Some("Objekte entpacken"),
        "Resolving deltas" => Some("Deltas auflösen"),
        "Writing objects" => Some("Objekte senden"),
        "Updating files" => Some("Dateien aktualisieren"),
        "Checking out files" => Some("Dateien auschecken"),
        _ => None,
    };
    let text = known.map_or_else(|| escape(&p.phase), |k| tr(k).to_owned());
    if p.remote {
        trf("Remote: {phase}", &[("phase", &text)])
    } else {
        text
    }
}

/// The phase names behind variables (for the translation test).
#[cfg(test)]
pub(crate) const PHASES: [&str; 9] = [
    "Objekte aufzählen",
    "Objekte zählen",
    "Objekte komprimieren",
    "Objekte empfangen",
    "Objekte entpacken",
    "Deltas auflösen",
    "Objekte senden",
    "Dateien aktualisieren",
    "Dateien auschecken",
];

/// What a fetch brought: per branch the new commits, new and removed branches, tags.
pub(crate) fn fetch_summary(updates: &[RefUpdate]) -> String {
    if updates.is_empty() {
        return tr("Fetch fertig: nichts Neues.").to_owned();
    }
    let mut parts: Vec<String> = Vec::new();
    let branches: Vec<&RefUpdate> = updates.iter().filter(|u| !u.tag).collect();
    for u in branches.iter().take(NAMED) {
        let name = escape(&u.name);
        parts.push(match (&u.old, &u.new, u.commits) {
            (None, _, _) => trf("{name} neu", &[("name", &name)]),
            (_, None, _) => trf("{name} entfernt", &[("name", &name)]),
            _ if u.forced => trf("{name} neu geschrieben", &[("name", &name)]),
            (_, _, Some(1)) => trf("{name} +1 Commit", &[("name", &name)]),
            (_, _, Some(n)) => trf("{name} +{n} Commits", &[("name", &name), ("n", &n)]),
            (_, _, None) => trf("{name} aktualisiert", &[("name", &name)]),
        });
    }
    if branches.len() > NAMED {
        parts.push(trf(
            "{n} weitere Branches",
            &[("n", &(branches.len() - NAMED))],
        ));
    }
    let tags = updates.iter().filter(|u| u.tag).count();
    match tags {
        0 => {}
        1 => parts.push(tr("1 Tag").to_owned()),
        n => parts.push(trf("{n} Tags", &[("n", &n)])),
    }
    trf("Fetch fertig: {was}.", &[("was", &parts.join(" · "))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitcraft_git::Oid;

    fn update(name: &str, old: bool, new: bool, commits: Option<u32>) -> RefUpdate {
        let id = Oid::parse("1111111111111111111111111111111111111111");
        RefUpdate {
            name: name.into(),
            tag: false,
            old: if old { id.clone() } else { None },
            new: if new { id } else { None },
            commits,
            forced: false,
        }
    }

    #[test]
    fn summaries_name_what_came_in() {
        crate::i18n::set_language(crate::i18n::Language::German);
        assert_eq!(fetch_summary(&[]), "Fetch fertig: nichts Neues.");
        let text = fetch_summary(&[
            update("origin/main", true, true, Some(3)),
            update("origin/neu", false, true, None),
            update("origin/alt", true, false, None),
            update("origin/eins", true, true, Some(1)),
        ]);
        assert_eq!(
            text,
            "Fetch fertig: origin/main +3 Commits · origin/neu neu · origin/alt entfernt · origin/eins +1 Commit."
        );
        let many: Vec<RefUpdate> = (0..8)
            .map(|i| update(&format!("o/{i}"), true, true, Some(2)))
            .collect();
        assert!(fetch_summary(&many).contains("3 weitere Branches"));
    }
}
