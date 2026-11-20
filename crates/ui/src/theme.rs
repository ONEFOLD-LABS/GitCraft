//! GitCraft's look: design tokens for dark and light, egui visuals built from them, and the
//! choice between Dark, Light and System.
//!
//! Colours come from the ONEFOLD LABS brand (ONEFOLD-LABS/brand): surfaces, lines and text from
//! `tokens/tokens.json` (`paper`, `surface`, `surface-sunken`, `line`, `ink`, `ink-muted`),
//! GitCraft's accent `app-gitcraft` (brick) and the file-state colours from
//! `apps/palette.json`, which `tools/make_app_icons.py` computes and contrast-checks. The two
//! button fills of the dark theme and the error banner are derived here in the same way. The
//! widget-state mapping follows StudioCraft's theme (MIT OR Apache-2.0, see NOTICE).
//!
//! Use: call [`install`] once at start-up, read colours with [`tokens`] wherever the UI paints.

use egui::{Color32, Context, CornerRadius, Stroke, Theme, ThemePreference, Visuals};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
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
        window_bg: rgb(0x0b0e10),
        panel_bg: rgb(0x171d20),
        panel_bg2: rgb(0x171d20),
        toolbar_bg: rgb(0x101416),
        border: rgb(0x3a454a),
        border_light: rgb(0x9aa8ae),
        row_alt: rgb(0x101416),
        text: rgb(0xeef2f1),
        text_strong: rgb(0xeef2f1),
        text_dim: rgb(0x9aa8ae),
        text_on_accent: rgb(0x08181b),
        accent: rgb(0xea7a4f),
        accent_dark: rgb(0xff9972),
        button: rgb(0x252d30),
        button_hi: rgb(0x333c40),
        selection: rgb(0x472215),
        added: rgb(0x49964e),
        modified: rgb(0xae7c02),
        deleted: rgb(0xce6862),
        renamed: rgb(0x328bcd),
        conflict: rgb(0xc86d41),
        untracked: rgb(0x868686),
        error_bg: rgb(0x47211d),
        error_text: rgb(0xfebfb7),
    };

    pub const LIGHT: Tokens = Tokens {
        window_bg: rgb(0xeceeeb),
        panel_bg: rgb(0xffffff),
        panel_bg2: rgb(0xffffff),
        toolbar_bg: rgb(0xf6f7f5),
        border: rgb(0xc9d0cf),
        border_light: rgb(0x56636a),
        row_alt: rgb(0xf6f7f5),
        text: rgb(0x111719),
        text_strong: rgb(0x111719),
        text_dim: rgb(0x56636a),
        text_on_accent: rgb(0xffffff),
        accent: rgb(0xc4501d),
        accent_dark: rgb(0xa53b03),
        button: rgb(0xeceeeb),
        button_hi: rgb(0xc9d0cf),
        selection: rgb(0xfee1d6),
        added: rgb(0x2b7932),
        modified: rgb(0x8c6306),
        deleted: rgb(0xad4b47),
        renamed: rgb(0x066fb0),
        conflict: rgb(0xa95023),
        untracked: rgb(0x6a6a6a),
        error_bg: rgb(0xfee0dc),
        error_text: rgb(0x822c27),
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
        // Brand book: borders of buttons and inputs in `ink-muted`, at least 3:1.
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border_light);
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
            assert!(
                contrast(t.border_light, t.panel_bg) >= 3.0,
                "control borders"
            );
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
