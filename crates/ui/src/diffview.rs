//! The diff of one file, shown in the centre in place of the history (docs/ux/layout.md):
//! hunk headers, old and new line numbers, added and removed lines on tinted backgrounds,
//! virtualised and scrollable in both directions. Very large diffs ask first.
//!
//! Diffs of the working tree can be staged in parts (issues #22, #23): each hunk has a button,
//! and changed lines can be selected (click, Ctrl+click, Shift+click; arrow keys, Shift+arrow
//! keys) and staged or unstaged with the header button or Enter.

use std::collections::BTreeSet;

use egui::{FontId, Key, Modifiers, Rect, RichText, Sense, Stroke, pos2, vec2};
use gitcraft_git::{FileDiff, LineKind, escape, escape_bytes};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Rows shown without asking.
const LARGE: usize = 50_000;
/// Characters per line drawn at most.
const MAX_CHARS: usize = 2_000;

/// What a diff row shows, prepared once when the diff arrives.
enum Row {
    Hunk {
        text: String,
        index: usize,
    },
    Line {
        kind: LineKind,
        old: Option<u32>,
        new: Option<u32>,
        text: String,
        /// Hunk and line index in the diff.
        at: (usize, usize),
    },
    NoNewline,
}

/// What the user asked for in the diff's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffAction {
    None,
    /// Back to the history.
    Back,
    /// The header's action button (stage or unstage the file).
    Act,
    /// Stage or unstage these (hunk, line) positions.
    Lines(Vec<(usize, usize)>),
    /// Discard the file's changes (or delete it, when untracked); asks first.
    Discard,
    /// Discard these (hunk, line) positions; asks first.
    DiscardLines(Vec<(usize, usize)>),
}

/// Which side of the index a working-tree diff shows; decides the button labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// Working tree against the index: parts can be staged.
    Unstaged,
    /// Index against HEAD: parts can be unstaged.
    Staged,
}

impl Side {
    fn hunk_label(self) -> &'static str {
        match self {
            Side::Unstaged => "Hunk vormerken",
            Side::Staged => "Hunk zurücknehmen",
        }
    }

    fn lines_label(self) -> &'static str {
        match self {
            Side::Unstaged => "Auswahl vormerken ({n})",
            Side::Staged => "Auswahl zurücknehmen ({n})",
        }
    }
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
    /// The diff as git reported it; partial staging checks it is still current.
    pub diff: FileDiff,
    /// Selected rows (changed lines only).
    selected: BTreeSet<usize>,
    /// Where a Shift selection starts and where the keyboard cursor is.
    anchor: Option<usize>,
    cursor: Option<usize>,
    /// Bring the cursor row into view at the next frame.
    scroll_to_cursor: bool,
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
        for (hi, h) in diff.hunks.iter().enumerate() {
            rows.push(Row::Hunk {
                text: format!(
                    "@@ -{},{} +{},{} @@ {}",
                    h.old_start,
                    h.old_len,
                    h.new_start,
                    h.new_len,
                    escape(&h.section)
                ),
                index: hi,
            });
            for (li, l) in h.lines.iter().enumerate() {
                let text = shown_line(&l.text);
                widest = widest.max(text.chars().count());
                rows.push(Row::Line {
                    kind: l.kind,
                    old: l.old,
                    new: l.new,
                    text,
                    at: (hi, li),
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
            diff: diff.clone(),
            selected: BTreeSet::new(),
            anchor: None,
            cursor: None,
            scroll_to_cursor: false,
        }
    }

    /// Parts of this diff can be staged: text, complete, not a submodule.
    fn partial(&self, side: Option<Side>) -> Option<Side> {
        side.filter(|_| !self.binary && !self.diff.truncated && !self.diff.submodule)
    }

    fn changed(&self, row: usize) -> bool {
        matches!(self.rows.get(row), Some(Row::Line { kind, .. }) if *kind != LineKind::Context)
    }

    /// The (hunk, line) positions of the selected rows.
    fn selected_lines(&self) -> Vec<(usize, usize)> {
        self.selected
            .iter()
            .filter_map(|r| match self.rows.get(*r) {
                Some(Row::Line { at, .. }) => Some(*at),
                _ => None,
            })
            .collect()
    }

    /// The changed lines of hunk `index`.
    fn hunk_lines(&self, index: usize) -> Vec<(usize, usize)> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::Line { kind, at, .. } if at.0 == index && *kind != LineKind::Context => {
                    Some(*at)
                }
                _ => None,
            })
            .collect()
    }

    /// A click on a changed row: alone, added/removed with Ctrl, a range with Shift.
    fn click(&mut self, row: usize, modifiers: Modifiers) {
        if modifiers.shift
            && let Some(anchor) = self.anchor
        {
            let (a, b) = (anchor.min(row), anchor.max(row));
            if !modifiers.command {
                self.selected.clear();
            }
            let range: Vec<usize> = (a..=b).filter(|r| self.changed(*r)).collect();
            self.selected.extend(range);
        } else if modifiers.command {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
            self.anchor = Some(row);
        } else {
            self.selected.clear();
            self.selected.insert(row);
            self.anchor = Some(row);
        }
        self.cursor = Some(row);
    }

    /// Arrow keys move the cursor to the next changed line; with Shift they extend the
    /// selection.
    fn step(&mut self, forward: bool, extend: bool) {
        let start = self.cursor;
        let next = if forward {
            let from = start.map_or(0, |c| c + 1);
            (from..self.rows.len()).find(|r| self.changed(*r))
        } else {
            let to = start.unwrap_or(self.rows.len());
            (0..to).rev().find(|r| self.changed(*r))
        };
        let Some(row) = next else {
            return;
        };
        let modifiers = Modifiers {
            shift: extend,
            ..Modifiers::NONE
        };
        if extend && self.anchor.is_none() {
            self.anchor = start.or(Some(row));
        }
        self.click(row, modifiers);
        self.scroll_to_cursor = true;
    }

    /// Draws the diff with an optional action button in its header (`action` is its German
    /// label).
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        action: Option<&'static str>,
        side: Option<Side>,
        discard: Option<&'static str>,
    ) -> DiffAction {
        let partial = self.partial(side);
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
            if let Some(label) = discard
                && ui
                    .button(RichText::new(tr(label)).color(t.deleted))
                    .clicked()
            {
                back = DiffAction::Discard;
            }
            ui.label(RichText::new(&self.title).color(t.text_strong).size(15.0));
            ui.label(RichText::new(format!("+{}", self.added)).color(t.added));
            ui.label(RichText::new(format!("-{}", self.removed)).color(t.deleted));
            for n in &self.notes {
                ui.label(RichText::new(n).color(t.text_dim));
            }
            if let Some(side) = partial
                && !self.selected.is_empty()
            {
                let n = self.selected.len();
                if ui
                    .button(trf(side.lines_label(), &[("n", &n)]))
                    .on_hover_text(tr("Enter"))
                    .clicked()
                {
                    back = DiffAction::Lines(self.selected_lines());
                }
                if side == Side::Unstaged
                    && ui
                        .button(
                            RichText::new(trf("Auswahl verwerfen ({n}) …", &[("n", &n)]))
                                .color(t.deleted),
                        )
                        .clicked()
                {
                    back = DiffAction::DiscardLines(self.selected_lines());
                }
                if ui.button(tr("Auswahl aufheben")).clicked() {
                    self.selected.clear();
                    self.anchor = None;
                }
            }
        });
        if partial.is_some() && ui.memory(|m| m.focused().is_none()) {
            let (down, up, enter) = ui.input_mut(|i| {
                let shift = Modifiers::SHIFT;
                (
                    // Shift first: the plain pattern would also match Shift+arrow.
                    if i.consume_key(shift, Key::ArrowDown) {
                        Some(true)
                    } else if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                        Some(false)
                    } else {
                        None
                    },
                    // Shift first: the plain pattern would also match Shift+arrow.
                    if i.consume_key(shift, Key::ArrowUp) {
                        Some(true)
                    } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                        Some(false)
                    } else {
                        None
                    },
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            });
            if let Some(extend) = down {
                self.step(true, extend);
            }
            if let Some(extend) = up {
                self.step(false, extend);
            }
            if enter && !self.selected.is_empty() {
                back = DiffAction::Lines(self.selected_lines());
            }
        }
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
        if let Some(a) = self.body(ui, t, partial) {
            back = a;
        }
        back
    }

    fn body(&mut self, ui: &mut egui::Ui, t: &Tokens, partial: Option<Side>) -> Option<DiffAction> {
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
        // Room for the hunk button in front of the hunk header.
        let hunk_x = if partial.is_some() { 150.0 } else { 8.0 };
        let width = (text_x + self.widest as f32 * char_w + 24.0).max(ui.available_width());
        let mut action = None;
        let mut clicked: Option<(usize, Modifiers)> = None;
        let mut scroll = egui::ScrollArea::both()
            .id_salt("diff")
            .auto_shrink([false, false]);
        if self.scroll_to_cursor
            && let Some(c) = self.cursor
        {
            self.scroll_to_cursor = false;
            let visible = ui.available_height();
            scroll = scroll.vertical_scroll_offset((c as f32 * row_h - visible / 2.0).max(0.0));
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            scroll.show_rows(ui, row_h, self.rows.len(), |ui, range| {
                for i in range {
                    let row = &self.rows[i];
                    let sense = match row {
                        Row::Line { kind, .. }
                            if partial.is_some() && *kind != LineKind::Context =>
                        {
                            Sense::click()
                        }
                        _ => Sense::hover(),
                    };
                    let (rect, resp) = ui.allocate_exact_size(vec2(width, row_h), sense);
                    self.paint_row(ui, t, row, rect, &font, gutter, text_x, hunk_x);
                    if self.selected.contains(&i) {
                        // A light tint keeps the added/removed colour readable; the bar marks
                        // the selection on its own.
                        ui.painter()
                            .rect_filled(rect, 0.0, t.selection.gamma_multiply(0.3));
                        ui.painter().rect_filled(
                            Rect::from_min_size(rect.left_top(), vec2(4.0, rect.height())),
                            0.0,
                            t.accent,
                        );
                    }
                    if self.cursor == Some(i) {
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(1.0, t.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    match row {
                        Row::Hunk { index, .. } => {
                            if let Some(side) = partial {
                                let button = Rect::from_min_size(
                                    rect.left_top() + vec2(4.0, 1.0),
                                    vec2(hunk_x - 10.0, row_h - 2.0),
                                );
                                if ui
                                    .put(button, egui::Button::new(tr(side.hunk_label())).small())
                                    .clicked()
                                {
                                    action = Some(DiffAction::Lines(self.hunk_lines(*index)));
                                }
                            }
                        }
                        Row::Line { text, .. } => {
                            if sense == Sense::click() {
                                resp.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::Role::CheckBox,
                                        true,
                                        self.selected.contains(&i),
                                        text,
                                    )
                                });
                                if resp.clicked() {
                                    clicked = Some((i, ui.input(|inp| inp.modifiers)));
                                }
                            }
                        }
                        Row::NoNewline => {}
                    }
                }
            });
        });
        if let Some((row, modifiers)) = clicked {
            self.click(row, modifiers);
        }
        action
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
        hunk_x: f32,
    ) {
        let p = ui.painter();
        let y = rect.center().y;
        let left = |x: f32| pos2(rect.left() + x, y);
        match row {
            Row::Hunk { text, .. } => {
                p.rect_filled(rect, 0.0, t.toolbar_bg);
                p.line_segment(
                    [rect.left_top(), rect.right_top()],
                    Stroke::new(1.0, t.border),
                );
                p.text(
                    left(hunk_x),
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
                ..
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
