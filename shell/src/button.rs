//! The labeled action button full-screen surfaces share, so every screen's
//! buttons read, highlight, and hit the same way.

use crate::theme;
use macroquad::prelude::{Rect, draw_rectangle, draw_rectangle_lines, draw_text, measure_text};

/// Draws one action button; `active` marks hover or keyboard focus.
pub(crate) fn draw(rect: Rect, label: &str, active: bool, s: f32) {
    draw_rectangle(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        if active {
            theme::SURFACE_CARD
        } else {
            theme::SURFACE_PANEL
        },
    );
    draw_rectangle_lines(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        if active { 2.0 * s } else { 1.0 * s },
        if active {
            theme::TEXT_ACCENT
        } else {
            theme::TEXT_DISABLED
        },
    );
    let size = 16.0 * s;
    let dims = measure_text(label, None, size as u16, 1.0);
    draw_text(
        label,
        rect.x + (rect.w - dims.width) * 0.5,
        rect.y + rect.h * 0.62,
        size,
        if active {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_BODY
        },
    );
}
