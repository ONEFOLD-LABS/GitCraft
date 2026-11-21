//! The command log panel (issue #15): every git call GitCraft made for the open repository,
//! newest at the bottom, so that each action can be understood and repeated in a terminal
//! (spec: Git first). A click copies the call as a shell line; the context menu also copies it
//! with a change into its directory. Credentials are redacted when calls are recorded.

use std::time::{Duration, UNIX_EPOCH};

use egui::{Align, Layout, RichText, Sense};
use gitcraft_git::{CommandLog, LogEntry, escape};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

#[derive(Default)]
pub(crate) struct LogPanel {
    pub(crate) open: bool,
    pub(crate) writes_only: bool,
    /// [`CommandLog::recorded`] when `entries` were copied.
    seen: Option<u64>,
    entries: Vec<LogEntry>,
}

/// `HH:MM:SS UTC`.
fn clock(entry: &LogEntry) -> String {
    let secs = entry
        .time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn duration(d: Duration) -> String {
    if d < Duration::from_secs(1) {
        format!("{} ms", d.as_millis())
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

impl LogPanel {
    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Takes new calls from `log` (only when something was recorded).
    fn refresh(&mut self, log: &CommandLog) {
        let now = log.recorded();
        if self.seen != Some(now) {
            self.entries = log.entries();
            self.seen = Some(now);
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, t: &Tokens, log: Option<&CommandLog>) {
        if let Some(log) = log {
            self.refresh(log);
        } else {
            self.entries.clear();
            self.seen = None;
        }
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Befehlsprotokoll")).color(t.text_strong));
            ui.label(
                RichText::new(trf("{n} Aufrufe", &[("n", &self.entries.len())])).color(t.text_dim),
            );
            ui.checkbox(&mut self.writes_only, tr("Nur ändernde Befehle"));
            ui.label(
                RichText::new(tr(
                    "Klick kopiert den Befehl; Zugangsdaten sind geschwärzt.",
                ))
                .color(t.text_dim),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = ui.small_button(tr("Schließen")).clicked();
            });
        });
        ui.separator();
        let shown: Vec<&LogEntry> = self
            .entries
            .iter()
            .filter(|e| !self.writes_only || e.write)
            .collect();
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(18.0) + 2.0;
        egui::ScrollArea::vertical()
            .id_salt("befehlsprotokoll")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_rows(ui, row_h, shown.len(), |ui, range| {
                for e in &shown[range] {
                    let line = e.shell_line();
                    let shown_line = escape(&line);
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, t.row_alt);
                    }
                    let mut cell = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(Layout::left_to_right(Align::Center)),
                    );
                    let dim = |s: String| egui::Label::new(RichText::new(s).monospace().color(t.text_dim)).selectable(false);
                    cell.add(dim(clock(e)));
                    cell.add_sized([64.0, row_h], dim(duration(e.duration)));
                    let (code, colour) = match e.code {
                        Some(0) => ("0".to_owned(), t.text_dim),
                        Some(c) => (c.to_string(), t.error_text),
                        None => ("–".to_owned(), t.conflict),
                    };
                    cell.add_sized(
                        [32.0, row_h],
                        egui::Label::new(RichText::new(code).monospace().color(colour)).selectable(false),
                    );
                    let marker = if e.write { tr("ändert") } else { "" };
                    cell.add_sized(
                        [52.0, row_h],
                        egui::Label::new(RichText::new(marker).color(t.accent)).selectable(false),
                    );
                    cell.add(
                        egui::Label::new(RichText::new(&shown_line).monospace().color(t.text))
                            .selectable(false)
                            .truncate(),
                    );
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, &shown_line)
                    });
                    let resp = resp.on_hover_ui(|ui| {
                        ui.label(RichText::new(&shown_line).monospace());
                        ui.label(trf(
                            "Verzeichnis: {pfad}",
                            &[("pfad", &escape(&e.cwd.to_string_lossy()))],
                        ));
                        let ended = match e.code {
                            Some(c) => trf("Exit-Code {code}", &[("code", &c)]),
                            None => tr("Von GitCraft beendet (abgebrochen, Zeitlimit oder nicht mehr gebraucht)").to_owned(),
                        };
                        ui.label(ended);
                        if !e.stderr.is_empty() {
                            ui.label(RichText::new(escape(&e.stderr)).monospace().color(t.text_dim));
                        }
                    });
                    if resp.clicked() {
                        ui.ctx().copy_text(line.clone());
                    }
                    resp.context_menu(|ui| {
                        if ui.button(tr("Befehl kopieren")).clicked() {
                            ui.ctx().copy_text(line.clone());
                            ui.close();
                        }
                        if ui.button(tr("Mit Verzeichniswechsel kopieren")).clicked() {
                            ui.ctx().copy_text(e.shell_line_in_dir());
                            ui.close();
                        }
                    });
                }
            });
        if close {
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_well() {
        assert_eq!(duration(Duration::from_millis(7)), "7 ms");
        assert_eq!(duration(Duration::from_millis(1530)), "1.5 s");
    }
}
