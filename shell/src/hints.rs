//! Coaching text shows only when the player seems stuck: key help and
//! gesture hints stay hidden until a screen has sat without a press for
//! a while, then fade in and stay until the screen changes. Information
//! (record details, prompts, warnings) never waits.

use macroquad::prelude::Color;
use oxide_protocol::RawEvent;
use std::cell::Cell;

/// How long a screen sits without a press before its coaching appears.
const DELAY_SECS: f32 = 15.0;
/// How long the coaching takes to fade in.
const FADE_SECS: f32 = 0.5;

thread_local! {
    /// This frame's coaching opacity. Full by default, so tests and
    /// headless runs, which never run the clock, still see every hint.
    static ALPHA: Cell<f32> = const { Cell::new(1.0) };
}

/// Publishes this frame's coaching opacity for the draw code.
pub(crate) fn set_alpha(alpha: f32) {
    ALPHA.with(|cell| cell.set(alpha));
}

/// This frame's coaching opacity, from 0 (hidden) to 1.
pub(crate) fn alpha() -> f32 {
    ALPHA.with(Cell::get)
}

/// Whether any coaching shows this frame.
pub(crate) fn showing() -> bool {
    alpha() > 0.0
}

/// `color` at this frame's coaching opacity.
pub(crate) fn fade(color: Color) -> Color {
    Color {
        a: color.a * alpha(),
        ..color
    }
}

/// Whether an event is the player acting: a press, a scroll, or typing.
/// Pointer motion alone doesn't count, so resting a hand on the mouse
/// never keeps the help away.
pub(crate) fn is_press(event: &RawEvent) -> bool {
    matches!(
        event,
        RawEvent::KeyDown { .. }
            | RawEvent::MouseDown { .. }
            | RawEvent::TouchDown { .. }
            | RawEvent::Wheel { .. }
            | RawEvent::Text { .. }
    )
}

/// When a screen's coaching shows. A new screen or any press restarts the
/// wait; once shown, the coaching stays until the screen changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HintClock {
    mode: &'static str,
    idle: f32,
    shown: bool,
    alpha: f32,
}

impl Default for HintClock {
    fn default() -> Self {
        Self {
            mode: "",
            idle: 0.0,
            shown: false,
            alpha: 0.0,
        }
    }
}

impl HintClock {
    /// Advances the clock by `dt` seconds on screen `mode` and returns the
    /// coaching opacity. `reduced` skips the fade.
    pub(crate) fn observe(
        &mut self,
        mode: &'static str,
        pressed: bool,
        dt: f32,
        reduced: bool,
    ) -> f32 {
        if mode != self.mode {
            *self = Self {
                mode,
                ..Self::default()
            };
        }
        // Before the coaching shows, a press restarts the wait; after, the
        // clock only runs on, so shown coaching stays.
        if pressed && !self.shown {
            self.idle = 0.0;
        } else {
            self.idle += dt;
        }
        self.shown = self.idle >= DELAY_SECS;
        self.alpha = match (self.shown, reduced) {
            (false, _) => 0.0,
            (true, true) => 1.0,
            (true, false) => ((self.idle - DELAY_SECS) / FADE_SECS).min(1.0),
        };
        self.alpha
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coaching_waits_for_an_idle_screen_then_stays() {
        let mut clock = HintClock::default();
        assert_eq!(clock.observe("home", false, 10.0, false), 0.0);
        assert_eq!(
            clock.observe("home", true, 0.1, false),
            0.0,
            "a press restarts the wait"
        );
        assert_eq!(clock.observe("home", false, 14.0, false), 0.0);
        let fading = clock.observe("home", false, 1.25, false);
        assert!(fading > 0.0 && fading < 1.0, "it fades in: {fading}");
        assert_eq!(clock.observe("home", false, 1.0, false), 1.0);
        assert_eq!(
            clock.observe("home", true, 0.1, false),
            1.0,
            "shown hints stay"
        );
        assert_eq!(
            clock.observe("settings", false, 0.1, false),
            0.0,
            "a new screen hides them"
        );
    }

    #[test]
    fn reduced_motion_skips_the_fade() {
        let mut clock = HintClock::default();
        assert_eq!(clock.observe("home", false, 15.5, true), 1.0);
    }

    #[test]
    fn hints_show_by_default_where_no_clock_runs() {
        assert_eq!(alpha(), 1.0);
        assert!(showing());
    }

    #[test]
    fn only_acting_counts_as_input() {
        assert!(is_press(&RawEvent::TouchDown {
            id: 1,
            x: 0.0,
            y: 0.0
        }));
        assert!(!is_press(&RawEvent::MouseMove { x: 1.0, y: 1.0 }));
        assert!(!is_press(&RawEvent::TouchMove {
            id: 1,
            x: 0.0,
            y: 0.0
        }));
    }
}
