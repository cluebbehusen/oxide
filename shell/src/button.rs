//! The labeled action button full-screen surfaces share, so every screen's
//! buttons read, highlight, and hit the same way.

use crate::theme;
use macroquad::prelude::{Rect, draw_rectangle, draw_rectangle_lines, draw_text, measure_text};

/// The `index`th button slot along the top-left corner: a fingertip
/// tall, wide enough for a short verb, and clear of the window edge.
pub(crate) fn corner_slot(index: usize, s: f32) -> Rect {
    let width = 104.0 * s;
    Rect::new(
        16.0 * s + index as f32 * (width + 8.0 * s),
        16.0 * s,
        width,
        crate::layout::MIN_TOUCH_TARGET * s,
    )
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_slots_are_fingertip_sized_and_never_overlap() {
        for s in [0.75, 1.0, 1.5] {
            let first = corner_slot(0, s);
            let second = corner_slot(1, s);
            assert!(first.h >= crate::layout::MIN_TOUCH_TARGET * s);
            assert!(first.x > 0.0 && first.y > 0.0);
            assert!(first.x + first.w < second.x, "slots keep a gap at {s}");
        }
    }
}
