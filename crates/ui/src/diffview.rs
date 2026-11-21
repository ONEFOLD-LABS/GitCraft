//! The diff of one file, shown in the centre in place of the history (docs/ux/layout.md):
//! hunk headers, old and new line numbers, added and removed lines on tinted backgrounds,
//! virtualised and scrollable in both directions. Very large diffs ask first.

use egui::{FontId, Rect, RichText, Sense, Stroke, pos2, vec2};
use gitcraft_git::{FileDiff, LineKind, escape, escape_bytes};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Rows shown without asking.
const LARGE: usize = 50_000;
/// Characters per line drawn at most.
const MAX_CHARS: usize = 2_000;

/// What a diff row shows, prepared once when the diff arrives.
enum Row {
    Hunk(String),
    Line {
        kind: LineKind,
        old: Option<u32>,
        new: Option<u32>,
        text: String,
    },
    NoNewline,
}

/// What the user asked for in the diff's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffAction {
    None,
    /// Back to the history.
    Back,
    /// The header's action button (stage or unstage the file).
    Act,
}

/// A diff ready to draw.
pub(crate) struct Prepared {
    pub title: String,
    pub added: usize,
    pub removed: usize,
    notes: Vec<String>,
    rows: Vec<Row>,
    widest: usize,
    show_anyway: bool,
    binary: bool,
}

impl Prepared {
    pub(crate) fn new(title: String, diff: &FileDiff) -> Prepared {
        let mut notes = Vec::new();
        if diff.new_file {
            notes.push(tr("neue Datei").to_owned());
        }
        if diff.deleted {
            notes.push(tr("gelöscht").to_owned());
        }
        if let Some((old, new)) = &diff.mode_change {
            notes.push(trf(
                "Modus {alt} -> {neu}",
                &[("alt", &escape(old)), ("neu", &escape(new))],
            ));
        }
        if diff.submodule {
            notes.push(tr("Submodul").to_owned());
        }
        if diff.truncated {
            notes.push(tr("gekürzt").to_owned());
        }
        let mut rows = Vec::new();
        let mut widest = 0;
        for h in &diff.hunks {
            rows.push(Row::Hunk(format!(
                "@@ -{},{} +{},{} @@ {}",
                h.old_start,
                h.old_len,
                h.new_start,
                h.new_len,
                escape(&h.section)
            )));
            for l in &h.lines {
                let text = shown_line(&l.text);
                widest = widest.max(text.chars().count());
                rows.push(Row::Line {
                    kind: l.kind,
                    old: l.old,
                    new: l.new,
                    text,
                });
                if l.no_newline {
                    rows.push(Row::NoNewline);
                }
            }
        }
        Prepared {
            title,
            added: diff.added(),
            removed: diff.removed(),
            notes,
            rows,
            widest: widest.min(MAX_CHARS),
            show_anyway: false,
            binary: diff.binary,
        }
    }

    /// Draws the diff with an optional action button in its header (`action` is its German
    /// label).
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        action: Option<&'static str>,
    ) -> DiffAction {
        let mut back = DiffAction::None;
        ui.horizontal(|ui| {
            if ui
                .button(tr("Zurück zum Graphen"))
                .on_hover_text(tr("Esc"))
                .clicked()
            {
                back = DiffAction::Back;
            }
            if let Some(label) = action
                && ui.button(tr(label)).clicked()
            {
                back = DiffAction::Act;
            }
            ui.label(RichText::new(&self.title).color(t.text_strong).size(15.0));
            ui.label(RichText::new(format!("+{}", self.added)).color(t.added));
            ui.label(RichText::new(format!("-{}", self.removed)).color(t.deleted));
            for n in &self.notes {
                ui.label(RichText::new(n).color(t.text_dim));
            }
        });
        ui.separator();
        if self.binary {
            ui.label(
                RichText::new(tr("Binärdatei, nicht als Text darstellbar.")).color(t.text_dim),
            );
            return back;
        }
        if self.rows.is_empty() {
            ui.label(RichText::new(tr("Keine Änderungen am Inhalt.")).color(t.text_dim));
            return back;
        }
        if self.rows.len() > LARGE && !self.show_anyway {
            ui.label(trf(
                "Dieser Diff hat {n} Zeilen.",
                &[("n", &self.rows.len())],
            ));
            if ui.button(tr("Trotzdem anzeigen")).clicked() {
                self.show_anyway = true;
            }
            return back;
        }
        self.body(ui, t);
        back
    }

    fn body(&self, ui: &mut egui::Ui, t: &Tokens) {
        let font = FontId::monospace(13.0);
        let (char_w, row_h) = ui
            .ctx()
            .fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font) + 3.0));
        let digits = self
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Line { old, new, .. } => (*old).max(*new),
                _ => None,
            })
            .max()
            .unwrap_or(1)
            .to_string()
            .len()
            .max(3) as f32;
        let gutter = digits * char_w + 12.0;
        let text_x = gutter * 2.0 + char_w * 2.0 + 8.0;
        let width = (text_x + self.widest as f32 * char_w + 24.0).max(ui.available_width());
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::both()
                .id_salt("diff")
                .auto_shrink([false, false])
                .show_rows(ui, row_h, self.rows.len(), |ui, range| {
                    for row in &self.rows[range] {
                        let (rect, _) = ui.allocate_exact_size(vec2(width, row_h), Sense::hover());
                        self.paint_row(ui, t, row, rect, &font, gutter, text_x);
                    }
                });
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &self,
        ui: &egui::Ui,
        t: &Tokens,
        row: &Row,
        rect: Rect,
        font: &FontId,
        gutter: f32,
        text_x: f32,
    ) {
        let p = ui.painter();
        let y = rect.center().y;
        let left = |x: f32| pos2(rect.left() + x, y);
        match row {
            Row::Hunk(text) => {
                p.rect_filled(rect, 0.0, t.toolbar_bg);
                p.line_segment(
                    [rect.left_top(), rect.right_top()],
                    Stroke::new(1.0, t.border),
                );
                p.text(
                    left(8.0),
                    egui::Align2::LEFT_CENTER,
                    text,
                    font.clone(),
                    t.text_dim,
                );
            }
            Row::NoNewline => {
                p.text(
                    left(text_x),
                    egui::Align2::LEFT_CENTER,
                    tr("Kein Zeilenumbruch am Dateiende"),
                    font.clone(),
                    t.text_dim,
                );
            }
            Row::Line {
                kind,
                old,
                new,
                text,
            } => {
                let (bg, sign, sign_color) = match kind {
                    LineKind::Added => (Some(t.diff_added_bg), "+", t.added),
                    LineKind::Removed => (Some(t.diff_removed_bg), "-", t.deleted),
                    LineKind::Context => (None, " ", t.text_dim),
                };
                if let Some(bg) = bg {
                    p.rect_filled(rect, 0.0, bg);
                }
                let number = |n: &Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                p.text(
                    left(gutter - 6.0),
                    egui::Align2::RIGHT_CENTER,
                    number(old),
                    font.clone(),
                    t.text_dim,
                );
                p.text(
                    left(gutter * 2.0 - 6.0),
                    egui::Align2::RIGHT_CENTER,
                    number(new),
                    font.clone(),
                    t.text_dim,
                );
                p.text(
                    left(gutter * 2.0 + 4.0),
                    egui::Align2::LEFT_CENTER,
                    sign,
                    font.clone(),
                    sign_color,
                );
                p.text(
                    left(text_x),
                    egui::Align2::LEFT_CENTER,
                    text,
                    font.clone(),
                    t.text,
                );
            }
        }
    }
}

/// A line of file content for display: tabs as four spaces, other control and invisible
/// characters escaped, invalid bytes as `\xNN`, at most [`MAX_CHARS`] characters.
fn shown_line(bytes: &[u8]) -> String {
    let parts: Vec<String> = bytes.split(|b| *b == b'\t').map(escape_bytes).collect();
    let line = parts.join("    ");
    if line.chars().count() > MAX_CHARS {
        let mut cut: String = line.chars().take(MAX_CHARS).collect();
        cut.push('…');
        cut
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_shown_safely() {
        assert_eq!(shown_line(b"\tx"), "    x");
        assert_eq!(shown_line(b"a\x1b[31mb"), "a\\u{1b}[31mb");
        assert_eq!(shown_line(b"\xff"), "\\xff");
        assert_eq!(shown_line(&vec![b'a'; 5000]).chars().count(), MAX_CHARS + 1);
    }
}
