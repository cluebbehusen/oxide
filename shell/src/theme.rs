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
mod tests;
