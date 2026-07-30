//! The chrome's text and surface tokens, tiered by role.
//!
//! One source for every piece of drawn TEXT: a line picks its tier by
//! what it IS — required instruction, supporting detail, genuinely
//! inactive affordance — never by a raw literal. Before 0.13 four
//! files carried divergent copies of these constants (three meanings
//! of "DIM" alone), so no contrast policy was enforceable; the tests
//! below pin the tiers to WCAG AA (4.5:1) against the house field
//! colors, treating each field as opaque. World decoration (order
//! rings, rally lines, faction art) is NOT text and keeps its own
//! palette in the renderer — retinting the world was never the job.

use macroquad::prelude::{Color, color_u8};

/// Headlines, active labels, selected rows — the brightest bone.
/// ~15:1 on the house fields.
pub const TEXT_PRIMARY: Color = color_u8!(232, 228, 216, 255);

/// Required reading that is not a headline: tutorial lessons, the
/// coaching line, tooltip descriptions, menu rows. ~9:1 composited.
pub const TEXT_BODY: Color = color_u8!(232, 228, 216, 200);

/// Supporting detail: hints, captions, off-cursor dial values,
/// hotkey corners. ~5.6:1 composited — still clear of AA.
pub const TEXT_SECONDARY: Color = color_u8!(232, 228, 216, 150);

/// Genuinely inactive content ONLY — a disabled card's face, never an
/// instruction. Deliberately below AA (~2.8:1): dimness is the signal.
pub const TEXT_DISABLED: Color = color_u8!(232, 228, 216, 90);

/// Screen titles and the selection accent — the rust orange.
pub const TEXT_TITLE: Color = color_u8!(196, 87, 59, 255);

/// Scrap costs and victories — the salvage gold.
pub const TEXT_ACCENT: Color = color_u8!(217, 164, 65, 255);

/// Refusals, warnings, losses.
pub const TEXT_DANGER: Color = color_u8!(217, 82, 74, 255);

/// The HUD's translucent chrome plate: top bar, verdict, overlays.
pub const SURFACE_PANEL: Color = color_u8!(20, 20, 24, 200);

/// Menu rows and setup cards — the same plate, near-opaque.
pub const SURFACE_MENU: Color = color_u8!(20, 20, 24, 230);

/// The tutorial card's field.
pub const SURFACE_CARD: Color = color_u8!(14, 14, 18, 235);

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG channel linearization.
    fn linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// WCAG relative luminance, alpha ignored (callers composite first).
    fn luminance(c: Color) -> f32 {
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    }

    /// sRGB-space alpha blend of `fg` over an opaque `field` — the
    /// same per-channel math the GPU applies when the text draws.
    fn composite(fg: Color, field: Color) -> Color {
        let a = fg.a;
        Color::new(
            fg.r * a + field.r * (1.0 - a),
            fg.g * a + field.g * (1.0 - a),
            fg.b * a + field.b * (1.0 - a),
            1.0,
        )
    }

    /// Contrast ratio of translucent text drawn on an opaque field.
    fn contrast(text: Color, field: Color) -> f32 {
        let field = Color::new(field.r, field.g, field.b, 1.0);
        let l1 = luminance(composite(text, field));
        let l2 = luminance(field);
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    }

    #[test]
    fn tutorial_body_clears_aa_on_its_card() {
        assert!(contrast(TEXT_BODY, SURFACE_CARD) >= 4.5);
    }

    #[test]
    fn body_and_secondary_clear_aa_on_every_house_field() {
        for field in [SURFACE_PANEL, SURFACE_MENU, SURFACE_CARD] {
            assert!(contrast(TEXT_BODY, field) >= 4.5);
            assert!(contrast(TEXT_SECONDARY, field) >= 4.5);
        }
    }

    #[test]
    fn primary_clears_aaa_on_every_house_field() {
        for field in [SURFACE_PANEL, SURFACE_MENU, SURFACE_CARD] {
            assert!(contrast(TEXT_PRIMARY, field) >= 7.0);
        }
    }

    #[test]
    fn disabled_reads_as_disabled_and_the_tiers_stay_ordered() {
        let on_card = |t| contrast(t, SURFACE_CARD);
        assert!(on_card(TEXT_DISABLED) < 4.5, "dimness is the signal");
        assert!(on_card(TEXT_DISABLED) < on_card(TEXT_SECONDARY));
        assert!(on_card(TEXT_SECONDARY) < on_card(TEXT_BODY));
        assert!(on_card(TEXT_BODY) < on_card(TEXT_PRIMARY));
    }
}
