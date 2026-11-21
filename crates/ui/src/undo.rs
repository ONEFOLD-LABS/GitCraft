//! Undo and redo of GitCraft's own operations (issue #84). Every operation that has a safe way
//! back is recorded as a [`Step`] with the commands that undo and redo it; the commands check
//! that nothing moved in between, and none of them discards uncommitted work. Commits that a
//! remote already has are refused.

use egui::RichText;
use gitcraft_core::{Command, Outcome};
use gitcraft_git::{HeadPos, Oid, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Steps kept.
const KEEP: usize = 50;

#[derive(Debug, Clone)]
pub(crate) struct Step {
    /// What was done, e.g. "Commit „Fix parser“".
    pub(crate) label: String,
    pub(crate) undo: Vec<Command>,
    pub(crate) redo: Vec<Command>,
}

#[derive(Default)]
pub(crate) struct History {
    done: Vec<Step>,
    undone: Vec<Step>,
}

impl History {
    pub(crate) fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }

    pub(crate) fn push(&mut self, step: Step) {
        if self.done.len() >= KEEP {
            self.done.remove(0);
        }
        self.done.push(step);
        self.undone.clear();
    }

    pub(crate) fn next_undo(&self) -> Option<&Step> {
        self.done.last()
    }

    pub(crate) fn next_redo(&self) -> Option<&Step> {
        self.undone.last()
    }

    /// Takes the step to undo (it moves to the redo list).
    pub(crate) fn take_undo(&mut self) -> Option<Step> {
        let step = self.done.pop()?;
        self.undone.push(step.clone());
        Some(step)
    }

    /// Takes the step to redo (it moves back to the undo list).
    pub(crate) fn take_redo(&mut self) -> Option<Step> {
        let step = self.undone.pop()?;
        self.done.push(step.clone());
        Some(step)
    }
}

fn switch_back(from: &HeadPos) -> Option<Command> {
    match from {
        HeadPos::Branch(name) => Some(Command::SwitchBranch(name.clone())),
        HeadPos::Detached(id) => Some(Command::SwitchDetached(id.clone())),
        // Git cannot switch back to a branch without commits.
        HeadPos::Unborn(_) => None,
    }
}

fn first_line(message: &str) -> String {
    escape(message.lines().next().unwrap_or_default())
}

/// The step for a finished operation, when it has a safe way back.
pub(crate) fn record(command: &Command, outcome: &Outcome) -> Option<Step> {
    let step =
        |label: String, undo: Vec<Command>, redo: Vec<Command>| Some(Step { label, undo, redo });
    match (command, outcome) {
        (
            Command::Commit { message, amend, .. },
            Outcome::Committed {
                before,
                after: Some(after),
                ..
            },
        ) => step(
            if *amend {
                trf(
                    "Commit ändern „{titel}“",
                    &[("titel", &first_line(message))],
                )
            } else {
                trf("Commit „{titel}“", &[("titel", &first_line(message))])
            },
            vec![Command::MoveHead {
                expected: Some(after.clone()),
                to: before.clone(),
                refuse_pushed: true,
            }],
            vec![Command::MoveHead {
                expected: before.clone(),
                to: Some(after.clone()),
                refuse_pushed: false,
            }],
        ),
        (Command::SwitchBranch(name), Outcome::Switched(from)) => step(
            trf("Wechsel zu {ziel}", &[("ziel", &escape(name))]),
            vec![switch_back(from)?],
            vec![command.clone()],
        ),
        (Command::SwitchDetached(id), Outcome::Switched(from)) => step(
            trf("Wechsel zu {ziel}", &[("ziel", &id.short())]),
            vec![switch_back(from)?],
            vec![command.clone()],
        ),
        (Command::CreateBranch { name, checkout, .. }, Outcome::BranchCreated { tip, from }) => {
            let mut undo = Vec::new();
            if let Some(from) = from {
                undo.push(switch_back(from)?);
            }
            undo.push(Command::DeleteRefIf {
                full: format!("refs/heads/{name}"),
                expected: tip.clone(),
            });
            step(
                trf("Branch {name} anlegen", &[("name", &escape(name))]),
                undo,
                vec![Command::CreateBranch {
                    name: name.clone(),
                    start: tip.as_str().to_owned(),
                    checkout: *checkout,
                }],
            )
        }
        (Command::RenameBranch { old, new }, Outcome::Written(_)) => step(
            trf(
                "Branch {alt} in {neu} umbenennen",
                &[("alt", &escape(old)), ("neu", &escape(new))],
            ),
            vec![Command::RenameBranch {
                old: new.clone(),
                new: old.clone(),
            }],
            vec![command.clone()],
        ),
        (Command::DeleteBranch(name), Outcome::Deleted(tip)) => step(
            trf("Branch {name} löschen", &[("name", &escape(name))]),
            vec![Command::CreateBranch {
                name: name.clone(),
                start: tip.as_str().to_owned(),
                checkout: false,
            }],
            vec![Command::DeleteRefIf {
                full: format!("refs/heads/{name}"),
                expected: tip.clone(),
            }],
        ),
        (Command::CreateTag { name, .. }, Outcome::Tagged(object)) => step(
            trf("Tag {name} anlegen", &[("name", &escape(name))]),
            vec![Command::DeleteRefIf {
                full: format!("refs/tags/{name}"),
                expected: object.clone(),
            }],
            vec![Command::RestoreTag {
                name: name.clone(),
                object: object.clone(),
            }],
        ),
        (Command::DeleteTag(name), Outcome::Deleted(object)) => step(
            trf("Tag {name} löschen", &[("name", &escape(name))]),
            vec![Command::RestoreTag {
                name: name.clone(),
                object: object.clone(),
            }],
            vec![Command::DeleteRefIf {
                full: format!("refs/tags/{name}"),
                expected: object.clone(),
            }],
        ),
        (Command::StashPush { .. }, Outcome::Stashed(Some(id))) => step(
            tr("Stash anlegen").to_owned(),
            vec![Command::StashApply {
                selector: "stash@{0}".into(),
                id: id.clone(),
                pop: true,
            }],
            vec![command.clone()],
        ),
        (Command::StashDrop { id, message, .. }, Outcome::Deleted(_)) => step(
            tr("Stash löschen").to_owned(),
            vec![Command::StashStore {
                id: id.clone(),
                message: message.clone(),
            }],
            vec![Command::StashDrop {
                selector: "stash@{0}".into(),
                id: id.clone(),
                message: message.clone(),
            }],
        ),
        (
            Command::Discard(_) | Command::DiscardLines { .. } | Command::DeleteUntracked(_),
            Outcome::Saved(saved),
        ) => step(
            match command {
                Command::DeleteUntracked(_) => tr("Dateien löschen").to_owned(),
                _ => tr("Änderungen verwerfen").to_owned(),
            },
            vec![Command::RestoreDiscarded(saved.clone())],
            vec![command.clone()],
        ),
        _ => None,
    }
}

fn short(id: &Oid) -> &str {
    id.short()
}

/// The git command a step runs, as the confirmation shows it.
pub(crate) fn describe(command: &Command) -> String {
    match command {
        Command::MoveHead { to: Some(to), .. } => format!("git reset --soft {}", short(to)),
        Command::MoveHead { to: None, .. } => "git update-ref -d HEAD".to_owned(),
        Command::DeleteRefIf { full, expected } => {
            format!("git update-ref -d {} {}", escape(full), short(expected))
        }
        Command::SwitchBranch(name) => format!("git switch {}", escape(name)),
        Command::SwitchDetached(id) => format!("git switch --detach {}", short(id)),
        Command::CreateBranch {
            name,
            start,
            checkout,
        } => {
            let start = Oid::parse(start).map_or_else(|| escape(start), |o| short(&o).to_owned());
            if *checkout {
                format!("git switch -c {} {start}", escape(name))
            } else {
                format!("git branch {} {start}", escape(name))
            }
        }
        Command::RenameBranch { old, new } => {
            format!("git branch -m {} {}", escape(old), escape(new))
        }
        Command::RestoreTag { name, object } => {
            format!(
                "git update-ref refs/tags/{} {}",
                escape(name),
                short(object)
            )
        }
        Command::StashApply { selector, pop, .. } => {
            format!(
                "git stash {} {selector}",
                if *pop { "pop" } else { "apply" }
            )
        }
        Command::StashStore { id, .. } => format!("git stash store {}", short(id)),
        Command::StashDrop { selector, .. } => format!("git stash drop {selector}"),
        Command::StashPush { .. } => "git stash push".to_owned(),
        Command::RestoreDiscarded(id) => trf(
            "(gesicherte Dateien aus {ref} zurückschreiben)",
            &[("ref", &escape(id))],
        ),
        Command::Discard(_) => "git restore --worktree …".to_owned(),
        Command::DiscardLines { .. } => "git apply --reverse …".to_owned(),
        Command::DeleteUntracked(_) => tr("(unversionierte Dateien löschen)").to_owned(),
        other => format!("{other:?}"),
    }
}

/// Confirmation before undoing or redoing; `Some(true)` runs it.
pub(crate) fn confirm(ctx: &egui::Context, t: &Tokens, step: &Step, redo: bool) -> Option<bool> {
    let mut choice = None;
    let commands = if redo { &step.redo } else { &step.undo };
    let modal = egui::Modal::new(egui::Id::unique("gitcraft-undo")).show(ctx, |ui| {
        ui.set_width(480.0);
        let title = if redo { tr("Wiederholen?") } else { tr("Rückgängig machen?") };
        ui.label(RichText::new(title).color(t.text_strong).size(16.0));
        ui.add_space(4.0);
        ui.label(&step.label);
        ui.add_space(4.0);
        ui.label(RichText::new(tr("GitCraft führt dazu aus:")).color(t.text_dim));
        for c in commands {
            ui.label(RichText::new(describe(c)).monospace());
        }
        ui.label(
            RichText::new(tr(
                "Nicht committete Änderungen bleiben unberührt. Hat sich seitdem etwas verschoben, bricht GitCraft ab, statt etwas zu überschreiben.",
            ))
            .color(t.text_dim),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            // Not "Wiederholen": that is the toolbar button's name.
            let label = if redo {
                tr("Erneut ausführen")
            } else {
                tr("Rückgängig machen")
            };
            if ui.button(label).clicked() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use gitcraft_git::Written;

    fn oid(c: char) -> Oid {
        Oid::parse(&c.to_string().repeat(40)).unwrap()
    }

    #[test]
    fn a_commit_is_undone_softly_and_refuses_pushed_ones() {
        let s = record(
            &Command::Commit {
                message: "Fix parser\n\nmehr".into(),
                amend: false,
                no_verify: false,
            },
            &Outcome::Committed {
                output: Written::default(),
                before: Some(oid('a')),
                after: Some(oid('b')),
            },
        )
        .unwrap();
        assert_eq!(s.label, "Commit „Fix parser“");
        assert!(matches!(
            &s.undo[..],
            [Command::MoveHead { expected: Some(e), to: Some(t), refuse_pushed: true }] if *e == oid('b') && *t == oid('a')
        ));
        assert_eq!(describe(&s.undo[0]), "git reset --soft aaaaaaa");
    }

    #[test]
    fn a_new_checked_out_branch_switches_back_then_deletes() {
        let s = record(
            &Command::CreateBranch {
                name: "x".into(),
                start: "HEAD".into(),
                checkout: true,
            },
            &Outcome::BranchCreated {
                tip: oid('c'),
                from: Some(HeadPos::Branch("main".into())),
            },
        )
        .unwrap();
        let lines: Vec<String> = s.undo.iter().map(describe).collect();
        assert_eq!(
            lines,
            ["git switch main", "git update-ref -d refs/heads/x ccccccc"]
        );
    }

    #[test]
    fn history_moves_steps_between_undo_and_redo() {
        let step = |l: &str| Step {
            label: l.into(),
            undo: vec![],
            redo: vec![],
        };
        let mut h = History::default();
        h.push(step("a"));
        h.push(step("b"));
        assert_eq!(h.take_undo().unwrap().label, "b");
        assert_eq!(h.next_redo().unwrap().label, "b");
        assert_eq!(h.take_redo().unwrap().label, "b");
        h.take_undo();
        h.push(step("c"));
        assert!(h.next_redo().is_none(), "a new step clears redo");
    }
}
