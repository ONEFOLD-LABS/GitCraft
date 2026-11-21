//! The clone dialog (issue #46): URL (checked while typing), where to put it (the folder name
//! follows the URL until it is edited), optional branch and depth; progress and cancel while
//! it runs.

use std::path::{Path, PathBuf};

use egui::RichText;
use gitcraft_core::{CloneJob, Wake};
use gitcraft_git::{CloneOptions, GitRunner, clone_dir_name, escape};

use crate::i18n::{tr, trf};
use crate::network::phase_text;
use crate::remotes::url_hint;
use crate::theme::Tokens;

pub(crate) struct CloneDialog {
    pub(crate) url: String,
    /// The folder that receives the new one.
    pub(crate) parent: String,
    pub(crate) name: String,
    name_edited: bool,
    pub(crate) branch: String,
    pub(crate) depth: String,
    focus: bool,
    job: Option<CloneJob>,
    error: Option<String>,
}

pub(crate) enum CloneDecision {
    Open,
    Close,
    /// Ask for the parent folder with the system dialog.
    PickParent,
    /// The clone is there.
    Done(PathBuf),
}

impl CloneDialog {
    pub(crate) fn new(parent: &Path) -> CloneDialog {
        CloneDialog {
            url: String::new(),
            parent: parent.display().to_string(),
            name: String::new(),
            name_edited: false,
            branch: String::new(),
            depth: String::new(),
            focus: true,
            job: None,
            error: None,
        }
    }

    pub(crate) fn target(&self) -> PathBuf {
        Path::new(self.parent.trim()).join(self.name.trim())
    }

    /// What blocks starting, as a text.
    fn problem(&self) -> Option<&'static str> {
        if let Some((text, true)) = url_hint(&self.url) {
            return Some(text);
        }
        if self.parent.trim().is_empty() || !Path::new(self.parent.trim()).is_dir() {
            return Some("Den Ordner, in den geklont wird, gibt es nicht.");
        }
        let name = self.name.trim();
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Some("Der Ordnername fehlt oder ist ungültig.");
        }
        let target = self.target();
        if target.exists() && std::fs::read_dir(&target).map_or(true, |mut d| d.next().is_some()) {
            return Some("Den Zielordner gibt es schon, und er ist nicht leer.");
        }
        if self.branch.trim().starts_with('-') {
            return Some("Ein Name darf nicht mit „-“ beginnen.");
        }
        if !self.depth.trim().is_empty()
            && self.depth.trim().parse::<u32>().map_or(true, |d| d == 0)
        {
            return Some("Die Tiefe ist eine Zahl ab 1.");
        }
        None
    }

    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        t: &Tokens,
        runner: &GitRunner,
        wake: Wake,
    ) -> CloneDecision {
        let mut decision = CloneDecision::Open;
        if let Some(result) = self.job.as_mut().and_then(CloneJob::poll) {
            self.job = None;
            match result {
                Ok(dir) => return CloneDecision::Done(dir),
                Err(gitcraft_git::GitError::Cancelled) => self.error = None,
                Err(e) => self.error = Some(crate::error_text(&e)),
            }
        }
        let running = self.job.is_some();
        let modal = egui::Modal::new(egui::Id::unique("gitcraft-clone")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(
                RichText::new(tr("Repository klonen"))
                    .color(t.text_strong)
                    .size(16.0),
            );
            ui.add_space(6.0);
            ui.add_enabled_ui(!running, |ui| {
                let l = ui.label(tr("URL"));
                let url = ui
                    .add(egui::TextEdit::singleline(&mut self.url).desired_width(f32::INFINITY))
                    .labelled_by(l.id);
                if std::mem::take(&mut self.focus) {
                    url.request_focus();
                }
                if url.changed() && !self.name_edited {
                    self.name = if self.url.trim().is_empty() {
                        String::new()
                    } else {
                        clone_dir_name(&self.url)
                    };
                }
                if let Some((text, blocks)) = url_hint(&self.url).filter(|_| !self.url.is_empty())
                {
                    let color = if blocks { t.text_dim } else { t.modified };
                    ui.label(RichText::new(tr(text)).color(color));
                }
                let l = ui.label(tr("Ablegen in"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.parent)
                            .desired_width(ui.available_width() - 90.0),
                    )
                    .labelled_by(l.id);
                    if ui.button(tr("Wählen …")).clicked() {
                        decision = CloneDecision::PickParent;
                    }
                });
                let l = ui.label(tr("Ordnername"));
                if ui
                    .add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY))
                    .labelled_by(l.id)
                    .changed()
                {
                    self.name_edited = true;
                }
                ui.horizontal(|ui| {
                    let l = ui.label(tr("Branch (optional)"));
                    ui.add(egui::TextEdit::singleline(&mut self.branch).desired_width(160.0))
                        .labelled_by(l.id);
                    let l = ui.label(tr("Tiefe (optional)"));
                    ui.add(egui::TextEdit::singleline(&mut self.depth).desired_width(60.0))
                        .labelled_by(l.id)
                        .on_hover_text(tr(
                            "Nur die neuesten Commits holen (flacher Klon); mehr Historie lässt sich später nachladen.",
                        ));
                });
                ui.label(
                    RichText::new(tr(
                        "Submodule werden nicht mitgeklont; GitCraft sagt danach, ob es welche gibt.",
                    ))
                    .color(t.text_dim),
                );
            });
            let problem = self.problem();
            if let Some(job) = &self.job {
                let progress = job.progress();
                let (fraction, text) = match &progress {
                    Some(p) => {
                        let phase = phase_text(p);
                        let text = match p.total {
                            Some(total) => format!("{phase} {}/{}", p.done, total),
                            None => format!("{phase} {}", p.done),
                        };
                        (p.fraction(), text)
                    }
                    None => (None, tr("verbinde …").to_owned()),
                };
                let bar = match fraction {
                    Some(f) => egui::ProgressBar::new(f),
                    None => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add(bar.text(text));
            } else if let Some(p) = problem.filter(|_| !self.url.is_empty()) {
                ui.label(RichText::new(tr(p)).color(t.text_dim));
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(t.error_text));
            }
            ui.horizontal(|ui| {
                if running {
                    if ui.button(tr("Abbrechen")).clicked()
                        && let Some(job) = &self.job
                    {
                        job.cancel();
                    }
                    return;
                }
                let ok = ui.add_enabled(problem.is_none(), egui::Button::new(tr("Klonen")));
                if ok.clicked() && problem.is_none() {
                    let options = CloneOptions {
                        url: self.url.trim().to_owned(),
                        dir: self.target(),
                        branch: Some(self.branch.trim().to_owned()).filter(|b| !b.is_empty()),
                        depth: self.depth.trim().parse().ok(),
                    };
                    self.error = None;
                    match CloneJob::start(runner.clone(), options, wake.clone()) {
                        Ok(job) => self.job = Some(job),
                        Err(e) => self.error = Some(crate::error_text(&e)),
                    }
                }
                if ui.button(tr("Schließen")).clicked() {
                    decision = CloneDecision::Close;
                }
            });
        });
        if matches!(decision, CloneDecision::Open) && modal.should_close() && !running {
            decision = CloneDecision::Close;
        }
        decision
    }

    /// Texts behind variables (for the translation test).
    #[cfg(test)]
    pub(crate) const PROBLEMS: [&str; 5] = [
        "Den Ordner, in den geklont wird, gibt es nicht.",
        "Der Ordnername fehlt oder ist ungültig.",
        "Den Zielordner gibt es schon, und er ist nicht leer.",
        "Die Tiefe ist eine Zahl ab 1.",
        "Ein Name darf nicht mit „-“ beginnen.",
    ];
}

/// The notice after a clone.
pub(crate) fn cloned_notice(dir: &Path) -> String {
    let name = escape(&dir.display().to_string());
    if gitcraft_git::has_submodules(dir) {
        trf(
            "{ordner} geklont. Es hat Submodule; sie wurden nicht mitgeklont (im Terminal: git submodule update --init --recursive).",
            &[("ordner", &name)],
        )
    } else {
        trf("{ordner} geklont.", &[("ordner", &name)])
    }
}
