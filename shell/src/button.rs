//! The labeled action button full-screen surfaces share, so every screen's
//! buttons read, highlight, and hit the same way.

use crate::press::{Fed, Press};
use crate::theme;
use macroquad::prelude::{
    Rect, Vec2, draw_rectangle, draw_rectangle_lines, draw_text, measure_text,
};
use oxide_protocol::RawEvent;

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

/// The top-left BACK button full-screen menus leave through. It sees a
/// frame's pointer events first, and the screen beneath gets only the
/// ones it leaves alone, so a press on it never reaches a row. Living
/// outside the rows, it survives every list rebuild.
#[derive(Default)]
pub(crate) struct BackButton {
    press: Press<()>,
}

impl BackButton {
    /// Whether this frame pressed the button, and the events it left
    /// for the screen.
    pub(crate) fn route(&mut self, events: &[RawEvent]) -> (bool, Vec<RawEvent>) {
        let rect = corner_slot(0, crate::render::ui_scale());
        let mut rest = Vec::with_capacity(events.len());
        for event in events {
            match self
                .press
                .feed(event, |p, _| rect.contains(p).then_some(()))
            {
                Fed::Activated(()) => return (true, rest),
                Fed::Held => {}
                Fed::Ignored => rest.push(*event),
            }
        }
        (false, rest)
    }
}

/// Draws the top-left BACK button, lit under the mouse.
pub(crate) fn draw_back(mouse: Vec2) {
    let s = crate::render::ui_scale();
    let rect = corner_slot(0, s);
    draw(rect, "BACK", rect.contains(mouse), s);
}

/// A click (or, with `touch`, a tap) on the BACK button.
#[cfg(test)]
pub(crate) fn press_back(touch: bool) -> Vec<RawEvent> {
    let p = corner_slot(0, crate::render::ui_scale()).center();
    if touch {
        vec![
            RawEvent::TouchDown {
                id: 9,
                x: p.x,
                y: p.y,
            },
            RawEvent::TouchUp {
                id: 9,
                x: p.x,
                y: p.y,
            },
        ]
    } else {
        let button = oxide_protocol::MouseButton::Left;
        vec![
            RawEvent::MouseDown {
                button,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button,
                x: p.x,
                y: p.y,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_back_button_claims_its_presses_and_passes_the_rest() {
        let mut back = BackButton::default();
        for touch in [false, true] {
            let (pressed, rest) = back.route(&press_back(touch));
            assert!(pressed);
            assert!(rest.is_empty(), "the screen never sees the press");
        }
        // Pressed on BACK, released elsewhere: nothing, and the release
        // stays the button's.
        let p = corner_slot(0, crate::render::ui_scale()).center();
        let (pressed, rest) = back.route(&[
            RawEvent::TouchDown {
                id: 1,
                x: p.x,
                y: p.y,
            },
            RawEvent::TouchUp {
                id: 1,
                x: 600.0,
                y: 400.0,
            },
        ]);
        assert!(!pressed);
        assert!(rest.is_empty());
        let elsewhere = [RawEvent::TouchDown {
            id: 2,
            x: 600.0,
            y: 400.0,
        }];
        assert_eq!(back.route(&elsewhere), (false, elsewhere.to_vec()));
    }

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
