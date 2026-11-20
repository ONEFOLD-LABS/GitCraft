//! GitCraft's look: design tokens for dark and light, egui visuals built from them, and the
//! choice between Dark, Light and System.
//!
//! Surfaces and the widget-state mapping follow StudioCraft's theme (`crates/ui/src/theme.rs`,
//! MIT OR Apache-2.0, itself based on SoundCraft's theme, see NOTICE). GitCraft's accent is
//! teal; the status colours (added, modified, deleted, renamed, conflict) are its own.
//!
//! Use: call [`install`] once at start-up, read colours with [`tokens`] wherever the UI paints.

use egui::{Color32, Context, CornerRadius, Stroke, Theme, ThemePreference, Visuals};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn rgba(hex: u32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, alpha)
}

/// Every colour the GitCraft UI paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    // surfaces
    pub window_bg: Color32,
    pub panel_bg: Color32,
    pub panel_bg2: Color32,
    pub toolbar_bg: Color32,
    pub border: Color32,
    pub border_light: Color32,
    pub row_alt: Color32,
    // text
    pub text: Color32,
    /// Headings and names that stand out (egui's `strong` would use the accent text colour).
    pub text_strong: Color32,
    pub text_dim: Color32,
    pub text_on_accent: Color32,
    // accent and controls
    pub accent: Color32,
    pub accent_dark: Color32,
    pub button: Color32,
    pub button_hi: Color32,
    pub selection: Color32,
    // file states
    pub added: Color32,
    pub modified: Color32,
    pub deleted: Color32,
    pub renamed: Color32,
    pub conflict: Color32,
    pub untracked: Color32,
    // messages
    pub error_bg: Color32,
    pub error_text: Color32,
}

impl Tokens {
    pub const DARK: Tokens = Tokens {
        window_bg: rgb(0x1e1e1e),
        panel_bg: rgb(0x2a2a2a),
        panel_bg2: rgb(0x323232),
        toolbar_bg: rgb(0x242424),
        border: rgb(0x121212),
        border_light: rgb(0x454545),
        row_alt: rgb(0x2f2f2f),
        text: rgb(0xd8d8d8),
        text_strong: rgb(0xf4f4f4),
        text_dim: rgb(0x9a9a9a),
        text_on_accent: rgb(0xffffff),
        accent: rgb(0x2f9e8f),
        accent_dark: rgb(0x1f6f65),
        button: rgb(0x3c3c3c),
        button_hi: rgb(0x4e4e4e),
        selection: rgba(0x2f9e8f, 77),
        added: rgb(0x6cc070),
        modified: rgb(0xd9b44a),
        deleted: rgb(0xe57c77),
        renamed: rgb(0x6aa8e8),
        conflict: rgb(0xff7a45),
        untracked: rgb(0xa8a8a8),
        error_bg: rgb(0x4a2424),
        error_text: rgb(0xffc2ba),
    };

    pub const LIGHT: Tokens = Tokens {
        window_bg: rgb(0xbcbcbc),
        panel_bg: rgb(0xcfcfcf),
        panel_bg2: rgb(0xdadada),
        toolbar_bg: rgb(0xc5c5c5),
        border: rgb(0x6a6a6a),
        border_light: rgb(0xa0a0a0),
        row_alt: rgb(0xc8c8c8),
        text: rgb(0x1a1a1a),
        text_strong: rgb(0x000000),
        text_dim: rgb(0x484848),
        text_on_accent: rgb(0xffffff),
        accent: rgb(0x1f7a6e),
        accent_dark: rgb(0x175c53),
        button: rgb(0xdddddd),
        button_hi: rgb(0xcbcbcb),
        selection: rgba(0x1f7a6e, 51),
        added: rgb(0x175c22),
        modified: rgb(0x684d00),
        deleted: rgb(0x98261f),
        renamed: rgb(0x1d5195),
        conflict: rgb(0x912f0b),
        untracked: rgb(0x4a4a4a),
        error_bg: rgb(0xf0cfca),
        error_text: rgb(0x6e1a12),
    };

    pub const fn of(theme: Theme) -> &'static Tokens {
        match theme {
            Theme::Dark => &Tokens::DARK,
            Theme::Light => &Tokens::LIGHT,
        }
    }

    /// egui visuals for this token set; `dark` picks egui's matching base.
    pub fn visuals(&self, dark: bool) -> Visuals {
        let mut v = if dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.panel_fill = self.panel_bg;
        v.window_fill = self.panel_bg2;
        v.window_stroke = Stroke::new(1.0, self.border_light);
        v.extreme_bg_color = self.window_bg;
        v.faint_bg_color = self.row_alt;
        v.code_bg_color = self.toolbar_bg;
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.selection;
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.override_text_color = Some(self.text);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = CornerRadius::same(4);
        let radius = CornerRadius::same(3);
        for (w, fill, fg) in [
            (&mut v.widgets.noninteractive, self.panel_bg, self.text_dim),
            (&mut v.widgets.inactive, self.button, self.text),
            (&mut v.widgets.hovered, self.button_hi, self.text),
            (&mut v.widgets.active, self.accent, self.text_on_accent),
            (&mut v.widgets.open, self.button_hi, self.text),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.corner_radius = radius;
            w.fg_stroke = Stroke::new(w.fg_stroke.width, fg);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.accent);
        v.widgets.active.bg_stroke = Stroke::new(1.0, self.accent_dark);
        v
    }
}

/// Sets GitCraft's dark and light visuals and follows `preference` (System tracks the OS).
pub fn install(ctx: &Context, preference: ThemePreference) {
    ctx.set_visuals_of(Theme::Dark, Tokens::DARK.visuals(true));
    ctx.set_visuals_of(Theme::Light, Tokens::LIGHT.visuals(false));
    ctx.set_theme(preference);
}

/// The tokens for the theme egui is showing right now.
pub fn tokens(ctx: &Context) -> &'static Tokens {
    Tokens::of(ctx.theme())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn luminance(c: Color32) -> f32 {
        let lin = |v: u8| {
            let s = f32::from(v) / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Body text and status letters stay readable (WCAG AA 4.5:1 for text) on the panels.
    #[test]
    fn text_and_status_colours_are_readable() {
        for t in [Tokens::DARK, Tokens::LIGHT] {
            for bg in [t.panel_bg, t.row_alt] {
                for (name, fg) in [
                    ("text", t.text),
                    ("text_strong", t.text_strong),
                    ("added", t.added),
                    ("modified", t.modified),
                    ("deleted", t.deleted),
                    ("renamed", t.renamed),
                    ("conflict", t.conflict),
                    ("untracked", t.untracked),
                ] {
                    let c = contrast(fg, bg);
                    assert!(c >= 4.5, "{name}: contrast {c:.2}");
                }
            }
            assert!(contrast(t.error_text, t.error_bg) >= 4.5);
            assert!(contrast(t.text_on_accent, t.accent) >= 3.0);
        }
    }

    #[test]
    fn install_follows_the_preference() {
        let ctx = Context::default();
        install(&ctx, ThemePreference::Light);
        assert_eq!(tokens(&ctx), &Tokens::LIGHT);
        install(&ctx, ThemePreference::Dark);
        assert_eq!(tokens(&ctx), &Tokens::DARK);
    }
}
