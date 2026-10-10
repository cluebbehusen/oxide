//! The pointer gesture menus and screen buttons share: a press arms the
//! zone under it, and only a release on that same zone commits. Dragging
//! away cancels, and the first finger down owns a touch gesture until it
//! lifts or the platform cancels it.
//!
//! A screen supplies its own hit test and feeds each pointer event through
//! [`Press::feed`]; the type only remembers what was armed. A scrolling
//! list feeds [`ScrollPress`] instead, which also lets the first finger
//! drag the list once it travels past [`DRAG_SLOP`].

use macroquad::prelude::{Vec2, vec2};
use oxide_protocol::{MouseButton, RawEvent};

/// Travel, in logical px at 1x, past which a finger scrolls a list instead
/// of tapping what it landed on.
pub const DRAG_SLOP: f32 = 8.0;

/// Where a pointer event happened, if it has a position.
pub fn position(event: &RawEvent) -> Option<Vec2> {
    match *event {
        RawEvent::MouseMove { x, y }
        | RawEvent::MouseDown { x, y, .. }
        | RawEvent::MouseUp { x, y, .. }
        | RawEvent::TouchDown { x, y, .. }
        | RawEvent::TouchMove { x, y, .. }
        | RawEvent::TouchUp { x, y, .. } => Some(vec2(x, y)),
        _ => None,
    }
}

/// What one pointer event meant to a screen's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed<Z> {
    /// No button claims the event; the screen handles it.
    Ignored,
    /// A button claims the event (it armed, is tracking, or the press
    /// was released elsewhere); the screen must not also act on it.
    Held,
    /// A button was pressed and released in place.
    Activated(Z),
}

/// Armed pointer state over a screen's hit zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press<Z> {
    mouse: Option<Z>,
    touch: Option<(u64, Z)>,
}

impl<Z> Default for Press<Z> {
    fn default() -> Self {
        Self {
            mouse: None,
            touch: None,
        }
    }
}

impl<Z: Copy + PartialEq> Press<Z> {
    /// Arms the zone under a mouse press, or disarms when it hit nothing.
    fn mouse_down(&mut self, zone: Option<Z>) {
        self.mouse = zone;
    }

    /// Resolves the mouse press: the armed zone, if the release landed on it.
    fn mouse_up(&mut self, zone: Option<Z>) -> Option<Z> {
        self.mouse.take().filter(|armed| zone == Some(*armed))
    }

    /// Whether a new finger may start a gesture. A touch that armed nothing
    /// owns nothing, so the next finger is free to try.
    fn touch_free(&self) -> bool {
        self.touch.is_none()
    }

    /// Arms the zone under a fresh touch. Call only while [`Self::touch_free`].
    fn touch_down(&mut self, id: u64, zone: Option<Z>) {
        self.touch = zone.map(|zone| (id, zone));
    }

    /// Whether `id` is the finger that armed the current touch gesture.
    pub fn owns(&self, id: u64) -> bool {
        self.touch.is_some_and(|(finger, _)| finger == id)
    }

    /// Resolves the touch gesture: the armed zone, if the owning finger
    /// lifted on it. Call only for the finger that [`Self::owns`] it.
    fn touch_up(&mut self, zone: Option<Z>) -> Option<Z> {
        let (_, armed) = self.touch.take()?;
        (zone == Some(armed)).then_some(armed)
    }

    /// Routes one pointer event through the gesture. `zone_at` is the
    /// screen's hit test; its flag says whether the pointer is a finger,
    /// for screens that pad fingertip targets.
    pub fn feed(&mut self, event: &RawEvent, zone_at: impl Fn(Vec2, bool) -> Option<Z>) -> Fed<Z> {
        let claimed = |zone: Option<Z>| match zone {
            Some(zone) => Fed::Activated(zone),
            None => Fed::Held,
        };
        match *event {
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            } => {
                let zone = zone_at(vec2(x, y), false);
                self.mouse_down(zone);
                if zone.is_some() {
                    Fed::Held
                } else {
                    Fed::Ignored
                }
            }
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x,
                y,
            } if self.mouse.is_some() => claimed(self.mouse_up(zone_at(vec2(x, y), false))),
            // Some platforms re-report every live finger when another
            // lands; the owning finger's repeat start changes nothing.
            RawEvent::TouchDown { id, .. } if self.owns(id) => Fed::Held,
            RawEvent::TouchDown { id, x, y } if self.touch_free() => {
                let zone = zone_at(vec2(x, y), true);
                self.touch_down(id, zone);
                if zone.is_some() {
                    Fed::Held
                } else {
                    Fed::Ignored
                }
            }
            RawEvent::TouchMove { id, .. } if self.owns(id) => Fed::Held,
            RawEvent::TouchUp { id, x, y } if self.owns(id) => {
                claimed(self.touch_up(zone_at(vec2(x, y), true)))
            }
            RawEvent::TouchCancel { id } if self.owns(id) => {
                self.touch = None;
                Fed::Held
            }
            _ => Fed::Ignored,
        }
    }

    /// Drops whatever is armed.
    pub fn cancel(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub fn armed_touch(&self) -> Option<(u64, Z)> {
        self.touch
    }
}

/// What one pointer event meant to a scrolling list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Swipe<Z> {
    /// As [`Fed::Ignored`].
    Ignored,
    /// As [`Fed::Held`].
    Held,
    /// As [`Fed::Activated`].
    Activated(Z),
    /// The dragging finger moved the list `dy` px down since the last
    /// report. `began` marks the move that crossed the slop: it carries
    /// the whole displacement since touchdown and cancelled the press.
    Scrolled { dy: f32, began: bool },
}

/// The first finger down on a scrolling list.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    id: u64,
    origin: f32,
    last: f32,
    scrolling: bool,
}

/// [`Press`] for a list a finger can also drag: the first finger down
/// taps a zone unless it travels past [`DRAG_SLOP`], after which it
/// scrolls the list until it lifts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollPress<Z> {
    press: Press<Z>,
    drag: Option<Drag>,
}

impl<Z> Default for ScrollPress<Z> {
    fn default() -> Self {
        Self {
            press: Press::default(),
            drag: None,
        }
    }
}

impl<Z: Copy + PartialEq> ScrollPress<Z> {
    /// Routes one pointer event, as [`Press::feed`] does, while watching
    /// the first finger for a drag. `ui` scales the slop.
    pub fn feed(
        &mut self,
        event: &RawEvent,
        ui: f32,
        zone_at: impl Fn(Vec2, bool) -> Option<Z>,
    ) -> Swipe<Z> {
        match *event {
            // While a finger drags the list, other fingers do nothing.
            RawEvent::TouchDown { id, .. }
            | RawEvent::TouchMove { id, .. }
            | RawEvent::TouchUp { id, .. }
            | RawEvent::TouchCancel { id }
                if self.scrolling() && self.drag.is_some_and(|drag| drag.id != id) =>
            {
                return Swipe::Held;
            }
            RawEvent::TouchDown { id, y, .. } if self.drag.is_none() => {
                self.drag = Some(Drag {
                    id,
                    origin: y,
                    last: y,
                    scrolling: false,
                });
            }
            RawEvent::TouchMove { id, y, .. } => {
                if let Some(drag) = self.drag.as_mut().filter(|drag| drag.id == id) {
                    let began = !drag.scrolling && (y - drag.origin).abs() > DRAG_SLOP * ui;
                    if began {
                        drag.scrolling = true;
                        drag.last = drag.origin;
                        self.press.cancel();
                    }
                    if drag.scrolling {
                        let dy = y - drag.last;
                        drag.last = y;
                        return Swipe::Scrolled { dy, began };
                    }
                }
            }
            RawEvent::TouchUp { id, .. } | RawEvent::TouchCancel { id }
                if self.drag.is_some_and(|drag| drag.id == id) =>
            {
                let scrolled = self.drag.take().is_some_and(|drag| drag.scrolling);
                if scrolled {
                    return Swipe::Held;
                }
            }
            _ => {}
        }
        match self.press.feed(event, zone_at) {
            Fed::Ignored => Swipe::Ignored,
            Fed::Held => Swipe::Held,
            Fed::Activated(zone) => Swipe::Activated(zone),
        }
    }

    /// Whether `id` is the finger that armed the current touch gesture.
    pub fn owns(&self, id: u64) -> bool {
        self.press.owns(id)
    }

    /// Whether a finger is dragging the list.
    pub fn scrolling(&self) -> bool {
        self.drag.is_some_and(|drag| drag.scrolling)
    }

    #[cfg(test)]
    pub fn armed_touch(&self) -> Option<(u64, Z)> {
        self.press.armed_touch()
    }
}

#[cfg(test)]
mod tests;
