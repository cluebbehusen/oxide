//! The chrome's text and surface colors, tiered by role.
//!
//! Drawn text picks its tier by what it is (required instruction,
//! supporting detail, inactive affordance) rather than a raw color
//! literal. The tests pin the readable tiers to WCAG AA (4.5:1) against
//! the house surfaces, treating each surface as opaque. World decoration
//! (order rings, rally lines, faction art) is not text and keeps its own
//! palette in the renderer.

use macroquad::prelude::{Color, color_u8};

/// Headlines, active labels, and selected rows; clears AAA on every
/// house surface.
pub const TEXT_PRIMARY: Color = color_u8!(232, 228, 216, 255);

/// Required reading that is not a headline: tutorial lessons, the
/// coaching line, tooltip descriptions, menu rows.
pub const TEXT_BODY: Color = color_u8!(232, 228, 216, 200);

/// Supporting detail: hints, captions, off-cursor dial values, hotkey
/// corners. Still clears AA.
pub const TEXT_SECONDARY: Color = color_u8!(232, 228, 216, 150);

/// Inactive content only, such as a disabled card's face; never an
/// instruction. Deliberately below AA so dimness reads as disabled.
pub const TEXT_DISABLED: Color = color_u8!(232, 228, 216, 90);

/// Screen titles and the selection accent.
pub const TEXT_TITLE: Color = color_u8!(196, 87, 59, 255);

/// Scrap costs and victories.
pub const TEXT_ACCENT: Color = color_u8!(217, 164, 65, 255);

/// Refusals, warnings, losses.
pub const TEXT_DANGER: Color = color_u8!(217, 82, 74, 255);

/// The HUD's translucent chrome plate: top bar, verdict, overlays.
pub const SURFACE_PANEL: Color = color_u8!(20, 20, 24, 200);

/// Menu rows and setup cards: the panel plate, near-opaque.
pub const SURFACE_MENU: Color = color_u8!(20, 20, 24, 230);

/// The tutorial card's field.
pub const SURFACE_CARD: Color = color_u8!(14, 14, 18, 235);

#[cfg(test)]
mod tests;
