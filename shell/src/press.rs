//! The pointer gesture every menu surface shares: a press arms the zone
//! under it, and only a release on that same zone commits. Dragging away
//! cancels, and the first finger down owns a touch gesture until it lifts.
//!
//! A screen supplies its own hit test and calls in with whichever zone the
//! pointer is over; this type only remembers what was armed.

use macroquad::prelude::{Vec2, vec2};
use oxide_protocol::{MouseButton, RawEvent};

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
    pub fn mouse_down(&mut self, zone: Option<Z>) {
        self.mouse = zone;
    }

    /// Resolves the mouse press: the armed zone, if the release landed on it.
    pub fn mouse_up(&mut self, zone: Option<Z>) -> Option<Z> {
        self.mouse.take().filter(|armed| zone == Some(*armed))
    }

    /// Whether a new finger may start a gesture. A touch that armed nothing
    /// owns nothing, so the next finger is free to try.
    pub fn touch_free(&self) -> bool {
        self.touch.is_none()
    }

    /// Arms the zone under a fresh touch. Call only while [`Self::touch_free`].
    pub fn touch_down(&mut self, id: u64, zone: Option<Z>) {
        self.touch = zone.map(|zone| (id, zone));
    }

    /// Whether `id` is the finger that armed the current touch gesture.
    pub fn owns(&self, id: u64) -> bool {
        self.touch.is_some_and(|(finger, _)| finger == id)
    }

    /// Resolves the touch gesture: the armed zone, if the owning finger
    /// lifted on it. Call only for the finger that [`Self::owns`] it.
    pub fn touch_up(&mut self, zone: Option<Z>) -> Option<Z> {
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

#[cfg(test)]
mod tests;
