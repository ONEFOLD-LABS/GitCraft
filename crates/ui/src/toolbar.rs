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
    Commands,
    Settings,
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
    badge: Option<u32>,
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
    if let Some(n) = badge.filter(|n| *n > 0) {
        // Commits to pull or push, as a small pill at the icon's top right.
        let text = if n > 99 {
            "99+".to_owned()
        } else {
            n.to_string()
        };
        let galley =
            ui.painter()
                .layout_no_wrap(text, egui::FontId::proportional(10.0), t.text_on_accent);
        let size = vec2((galley.size().x + 8.0).max(16.0), 15.0);
        let pill = Rect::from_center_size(pos2(center.x + 12.0, center.y - 8.0), size);
        ui.painter().rect_filled(pill, 7.5, t.accent);
        ui.painter().galley(
            pill.center() - galley.size() / 2.0,
            galley,
            t.text_on_accent,
        );
    }
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

/// The action buttons. Those in `ready` work; the others are disabled with the version that
/// brings them in the tooltip. Pull and Push show how many commits the current branch is behind
/// and ahead of its upstream. Returns the clicked action.
pub(crate) fn actions(
    ui: &mut egui::Ui,
    t: &Tokens,
    behind: u32,
    ahead: u32,
    ready: &[Icon],
) -> Option<Icon> {
    let mut clicked = None;
    for (icon, label, version) in ACTIONS {
        let badge = match icon {
            Icon::Pull => Some(behind),
            Icon::Push => Some(ahead),
            _ => None,
        };
        let enabled = ready.contains(&icon);
        let mut hint = if enabled {
            String::new()
        } else {
            trf("Kommt mit Version {v}", &[("v", &version)])
        };
        if let Some(n) = badge.filter(|n| *n > 0) {
            let what = if icon == Icon::Pull {
                trf("{n} Commits zum Holen", &[("n", &n)])
            } else {
                trf("{n} Commits zum Hochladen", &[("n", &n)])
            };
            hint = format!("{what}. {hint}");
        }
        let resp = button(ui, t, icon, tr(label), enabled, badge);
        let resp = if hint.is_empty() {
            resp
        } else {
            resp.on_hover_text(hint)
        };
        if resp.clicked() {
            clicked = Some(icon);
        }
    }
    clicked
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
        Icon::Commands => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(16.0, 13.0)),
                2.0,
                s,
                egui::StrokeKind::Middle,
            );
            poly(&[(-4.5, -2.0), (-1.5, 0.5), (-4.5, 3.0)]);
            poly(&[(0.5, 3.0), (4.5, 3.0)]);
        }
        Icon::Settings => {
            p.circle_stroke(c, 3.0, s);
            for k in 0..8 {
                let a = k as f32 / 8.0 * std::f32::consts::TAU;
                let (x, y) = (a.cos(), a.sin());
                p.line_segment(
                    [at(x * 5.5, y * 5.5), at(x * 8.0, y * 8.0)],
                    Stroke::new(2.2, color),
                );
            }
            p.circle_stroke(c, 6.0, s);
        }
        Icon::Pop => {
            poly(&[(-7.0, 1.0), (-7.0, 8.0), (7.0, 8.0), (7.0, 1.0)]);
            poly(&[(0.0, 4.0), (0.0, -8.0)]);
            poly(&[(-3.5, -4.5), (0.0, -8.0), (3.5, -4.5)]);
        }
    }
}

/// A caption above a value (repository, branch) on the left of the toolbar.
/// With `clickable`, hovering highlights it and the response reports clicks.
pub(crate) fn caption(
    ui: &mut egui::Ui,
    t: &Tokens,
    caption: &str,
    value: &str,
    hover: &str,
    clickable: bool,
) -> egui::Response {
    let sense = if clickable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(caption_width(ui, value), BUTTON.y), sense);
    if clickable && resp.hovered() {
        ui.painter()
            .rect_filled(rect.expand2(vec2(4.0, 0.0)), 6.0, t.button_hi);
    }
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
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, clickable, format!("{caption}: {value}"))
    });
    resp.on_hover_text(hover)
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
