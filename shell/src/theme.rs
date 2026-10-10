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

/// The tutorial card and hover tooltips.
pub const SURFACE_CARD: Color = color_u8!(14, 14, 18, 235);

/// Opaque chrome plates: the menu button, the control-group column, the
/// orders dock and the QUEUE toggle.
pub const SURFACE_PLATE: Color = color_u8!(20, 20, 24, 255);

/// Caption strips floating over the battlefield: the performance readout
/// and the final-map and replay help and transport bars.
pub const SURFACE_CAPTION: Color = color_u8!(15, 15, 19, 230);

/// The command band beneath the panel's cards.
pub const SURFACE_BAND: Color = color_u8!(20, 24, 26, 255);

/// The darkening plate under full-screen menus and the match report.
pub const VEIL: Color = color_u8!(10, 10, 15, 245);

/// A command card at rest.
pub const CARD_IDLE: Color = color_u8!(29, 35, 38, 255);

/// A command card under the pointer.
pub const CARD_HOVER: Color = color_u8!(48, 57, 58, 255);

/// An armed or selected command card.
pub const CARD_ARMED: Color = color_u8!(67, 57, 37, 255);

/// Small buttons and chips: queue chips, the Stop button, setup chips.
pub const CHIP: Color = color_u8!(36, 36, 46, 255);

/// The top bar's idle and alert badges and the menu button.
pub const BADGE: Color = color_u8!(57, 45, 30, 255);

/// The plate behind a capability or stat icon.
pub const ICON_PLATE: Color = color_u8!(13, 13, 18, 217);

/// A health bar's empty track and its fill.
pub const HEALTH_TRACK: Color = color_u8!(49, 61, 49, 255);
pub const HEALTH_FILL: Color = color_u8!(165, 180, 142, 255);

/// The command band's top edge and the match report's frame.
pub const EDGE_WARM: Color = color_u8!(119, 107, 79, 180);

/// A command card's edge at rest.
pub const EDGE_CARD: Color = color_u8!(55, 65, 66, 180);

/// A chip's edge at rest.
pub const EDGE_CHIP: Color = color_u8!(115, 115, 133, 204);

/// The edge of whatever has focus: the hovered button, the selected chip.
pub const EDGE_FOCUS: Color = TEXT_PRIMARY;

/// Plates' and panels' edges.
pub const BORDER_STRONG: Color = color_u8!(153, 153, 166, 102);

/// Rules, empty slots and disabled cards.
pub const BORDER_FAINT: Color = color_u8!(153, 153, 166, 64);

/// Alerts and the stalled-unit toasts: a warmer red than refusals.
pub const TEXT_ALERT: Color = color_u8!(235, 128, 115, 255);

/// The smallest target a fingertip can reliably hit, in logical px.
pub const MIN_TOUCH_TARGET: f32 = 44.0;

/// The chrome's stroke weights, in logical px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stroke {
    /// Rules and resting outlines.
    Hairline,
    /// Plates' and chips' edges.
    Edge,
    /// Focus and selection.
    Focus,
    /// The primary call to action's selection.
    Heavy,
}

impl Stroke {
    /// The stroke at UI scale `ui`, never thinner than a device pixel.
    pub fn at(self, ui: f32) -> f32 {
        let width = match self {
            Self::Hairline => 1.0,
            Self::Edge => 1.5,
            Self::Focus => 2.0,
            Self::Heavy => 3.0,
        };
        (width * ui).max(1.0)
    }
}

#[cfg(test)]
mod tests;
