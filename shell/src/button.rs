//! The labeled action button full-screen surfaces share, and the top-left
//! BACK button built from it.

use crate::numeric;
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
    let dims = measure_text(label, None, numeric::font_size(size), 1.0);
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
/// ones it leaves alone, so a press on it never reaches a row. It lives
/// outside the rows, so list rebuilds do not disarm it.
#[derive(Default)]
pub(crate) struct BackButton {
    press: Press<()>,
}

impl BackButton {
    /// Drops a half-made press, as when the screen beneath changes.
    pub(crate) fn cancel(&mut self) {
        self.press.cancel();
    }

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
mod tests;
