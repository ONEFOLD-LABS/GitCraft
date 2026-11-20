//! The toolbar's action buttons (docs/ux/layout.md): an icon above a label. Icons are drawn as
//! vectors with the brand's single-colour stroke (no icon fonts, no emoji). Actions that do not
//! exist yet are shown disabled, with the version that brings them in the tooltip.

use egui::{Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Undo,
    Redo,
    Pull,
    Push,
    Branch,
    Stash,
    Pop,
}

/// An action of the toolbar and the version that brings it.
pub(crate) const ACTIONS: [(Icon, &str, &str); 7] = [
    (Icon::Undo, "Rückgängig", "0.4"),
    (Icon::Redo, "Wiederholen", "0.4"),
    (Icon::Pull, "Pull", "0.5"),
    (Icon::Push, "Push", "0.5"),
    (Icon::Branch, "Branch", "0.4"),
    (Icon::Stash, "Stash", "0.4"),
    (Icon::Pop, "Pop", "0.4"),
];

const BUTTON: egui::Vec2 = egui::Vec2::new(58.0, 46.0);

/// A large toolbar button; returns its response (clicks only when enabled).
pub(crate) fn button(
    ui: &mut egui::Ui,
    t: &Tokens,
    icon: Icon,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let label_w = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(11.5),
            Color32::WHITE,
        )
        .size()
        .x;
    let size = vec2(BUTTON.x.max(label_w + 14.0), BUTTON.y);
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, t.button_hi);
    }
    let color = if enabled {
        t.text
    } else {
        t.text_dim.gamma_multiply(0.6)
    };
    let center = pos2(rect.center().x, rect.top() + 16.0);
    draw(ui.painter(), icon, center, color);
    ui.painter().text(
        pos2(rect.center().x, rect.bottom() - 9.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.5),
        color,
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    resp
}

/// The disabled buttons of actions that come in later versions.
pub(crate) fn coming(ui: &mut egui::Ui, t: &Tokens) {
    for (icon, label, version) in ACTIONS {
        button(ui, t, icon, tr(label), false)
            .on_hover_text(trf("Kommt mit Version {v}", &[("v", &version)]));
    }
}

fn draw(p: &egui::Painter, icon: Icon, c: Pos2, color: Color32) {
    let s = Stroke::new(1.6, color);
    let at = |x: f32, y: f32| pos2(c.x + x, c.y + y);
    let poly = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            pts.iter().map(|&(x, y)| at(x, y)).collect(),
            s,
        ));
    };
    match icon {
        Icon::Undo => {
            poly(&[(7.0, 6.0), (7.0, -1.0), (-6.0, -1.0)]);
            poly(&[(-2.0, -5.0), (-6.0, -1.0), (-2.0, 3.0)]);
        }
        Icon::Redo => {
            poly(&[(-7.0, 6.0), (-7.0, -1.0), (6.0, -1.0)]);
            poly(&[(2.0, -5.0), (6.0, -1.0), (2.0, 3.0)]);
        }
        Icon::Pull => {
            poly(&[(0.0, -8.0), (0.0, 4.0)]);
            poly(&[(-4.0, 0.0), (0.0, 4.0), (4.0, 0.0)]);
            poly(&[(-7.0, 8.0), (7.0, 8.0)]);
        }
        Icon::Push => {
            poly(&[(0.0, 4.0), (0.0, -8.0)]);
            poly(&[(-4.0, -4.0), (0.0, -8.0), (4.0, -4.0)]);
            poly(&[(-7.0, 8.0), (7.0, 8.0)]);
        }
        Icon::Branch => {
            poly(&[(-3.0, -6.0), (-3.0, 6.0)]);
            poly(&[(-3.0, 4.0), (-3.0, 1.0), (4.0, -1.0), (4.0, -4.0)]);
            for (x, y) in [(-3.0, -7.0), (-3.0, 7.0), (4.0, -6.0)] {
                p.circle_filled(at(x, y), 2.2, color);
            }
        }
        Icon::Stash => {
            poly(&[(-7.0, 1.0), (-7.0, 8.0), (7.0, 8.0), (7.0, 1.0)]);
            poly(&[(0.0, -8.0), (0.0, 4.0)]);
            poly(&[(-3.5, 0.5), (0.0, 4.0), (3.5, 0.5)]);
        }
        Icon::Pop => {
            poly(&[(-7.0, 1.0), (-7.0, 8.0), (7.0, 8.0), (7.0, 1.0)]);
            poly(&[(0.0, 4.0), (0.0, -8.0)]);
            poly(&[(-3.5, -4.5), (0.0, -8.0), (3.5, -4.5)]);
        }
    }
}

/// A caption above a value (repository, branch) on the left of the toolbar.
pub(crate) fn caption(ui: &mut egui::Ui, t: &Tokens, caption: &str, value: &str, hover: &str) {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(caption_width(ui, value), BUTTON.y), Sense::hover());
    let p = ui.painter();
    p.text(
        pos2(rect.left(), rect.top() + 12.0),
        egui::Align2::LEFT_CENTER,
        caption.to_uppercase(),
        egui::FontId::proportional(10.5),
        t.text_dim,
    );
    p.with_clip_rect(Rect::from_min_size(rect.min, rect.size()))
        .text(
            pos2(rect.left(), rect.top() + 31.0),
            egui::Align2::LEFT_CENTER,
            value,
            egui::FontId::proportional(15.0),
            t.text_strong,
        );
    resp.on_hover_text(hover);
}

fn caption_width(ui: &egui::Ui, value: &str) -> f32 {
    let w = ui
        .painter()
        .layout_no_wrap(
            value.to_owned(),
            egui::FontId::proportional(15.0),
            Color32::WHITE,
        )
        .size()
        .x;
    w.clamp(60.0, 260.0) + 8.0
}
