//! Raw input events — the shell's single input funnel.
//!
//! Every frame the shell turns whatever macroquad reports (and whatever
//! arrived via [`crate::Request::InjectEvent`]) into a list of these, then
//! maps them to camera operations, selection changes, and sim commands.
//! Injected and hardware events take the identical path, which is what makes
//! presentation-layer tests trustworthy without OS-level input faking.
//!
//! Touch variants flow through the shell's real touch handling (tap
//! select, drag pan, pinch zoom) — one funnel for every pointer
//! species, sized for the mobile ports.

use serde::{Deserialize, Serialize};

/// One input event, in window pixel coordinates where applicable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RawEvent {
    /// Cursor moved.
    MouseMove {
        /// Window x in pixels.
        x: f32,
        /// Window y in pixels.
        y: f32,
    },
    /// Button pressed.
    MouseDown {
        /// Which button.
        button: MouseButton,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Button released.
    MouseUp {
        /// Which button.
        button: MouseButton,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Scroll wheel; positive is away from the user (zoom in).
    Wheel {
        /// Scroll amount in notches.
        delta: f32,
    },
    /// Key pressed.
    KeyDown {
        /// Which key.
        key: Key,
    },
    /// Key released.
    KeyUp {
        /// Which key.
        key: Key,
    },
    /// Touch began (mobile; unused on desktop).
    TouchDown {
        /// Stable touch id.
        id: u64,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Touch moved.
    TouchMove {
        /// Stable touch id.
        id: u64,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Touch ended.
    TouchUp {
        /// Stable touch id.
        id: u64,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assert_every_tag_sampled;

    fn roundtrip(event: RawEvent) -> RawEvent {
        let json = serde_json::to_string(&event).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    /// Contiguous index per [`RawEvent`] variant, in declaration order.
    fn event_tag(event: &RawEvent) -> usize {
        match event {
            RawEvent::MouseMove { .. } => 0,
            RawEvent::MouseDown { .. } => 1,
            RawEvent::MouseUp { .. } => 2,
            RawEvent::Wheel { .. } => 3,
            RawEvent::KeyDown { .. } => 4,
            RawEvent::KeyUp { .. } => 5,
            RawEvent::TouchDown { .. } => 6,
            RawEvent::TouchMove { .. } => 7,
            RawEvent::TouchUp { .. } => 8,
        }
    }

    const EVENT_VARIANTS: usize = 9;

    #[test]
    fn every_raw_event_variant_including_touch_survives_a_roundtrip() {
        // The touch trio ships ahead of the mobile funnel that will read it,
        // so nothing else exercises it; the wire contract is pinned here.
        let events = [
            RawEvent::MouseMove { x: 1.5, y: 2.5 },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: 3.0,
                y: 4.0,
            },
            RawEvent::MouseUp {
                button: MouseButton::Right,
                x: 5.0,
                y: 6.0,
            },
            RawEvent::Wheel { delta: -2.0 },
            RawEvent::KeyDown { key: Key::Escape },
            RawEvent::KeyUp { key: Key::Shift },
            RawEvent::TouchDown {
                id: 7,
                x: 8.0,
                y: 9.0,
            },
            RawEvent::TouchMove {
                id: 7,
                x: 10.0,
                y: 11.0,
            },
            RawEvent::TouchUp {
                id: 7,
                x: 12.0,
                y: 13.0,
            },
        ];
        assert_every_tag_sampled(events.iter().map(event_tag), EVENT_VARIANTS, "raw event");
        for event in events {
            assert_eq!(
                roundtrip(event),
                event,
                "raw event did not survive: {event:?}"
            );
        }
    }

    #[test]
    fn the_middle_mouse_button_survives_a_roundtrip() {
        let event = RawEvent::MouseDown {
            button: MouseButton::Middle,
            x: 0.0,
            y: 0.0,
        };
        assert_eq!(roundtrip(event), event);
    }
}

/// Mouse buttons the shell cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Select / drag-select.
    Left,
    /// Context order (move / attack / harvest).
    Right,
    /// Unused, reserved.
    Middle,
}

/// The keys the shell maps. Deliberately only what the game uses — extend
/// alongside the input mapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    /// Pan up.
    Up,
    /// Pan down.
    Down,
    /// Pan left.
    Left,
    /// Pan right.
    Right,
    /// Train a Harvester at the selected (or first) Foundry.
    H,
    /// Train a Sentinel.
    S,
    /// Jump the camera to the last alert (classic profile). Formerly
    /// armed attack-move; reassigned when alerts landed.
    A,
    /// Pause / unpause.
    P,
    /// Arm a patrol route; pressed again, starts it.
    R,
    /// Open the build palette (harvester selected).
    B,
    /// Reserved. Formerly armed fabricator placement; the build palette
    /// superseded it. Kept for wire compatibility.
    N,
    /// Scrap the selected construction site (partial refund).
    X,
    /// Activate the highlighted menu item.
    Enter,
    /// Deselect.
    Escape,
    /// Center the camera on your Foundry.
    Space,
    /// Toggle debug overlay.
    F1,
    /// Modifier: additive selection (either physical shift key).
    Shift,
    /// Modifier: control-group assignment (either physical ctrl key).
    Ctrl,
    /// Control group 1 (recall; with Ctrl, assign).
    Num1,
    /// Control group 2.
    Num2,
    /// Control group 3.
    Num3,
    /// Control group 4.
    Num4,
    /// Control group 5.
    Num5,
    /// Sixth contextual digit (build palette / production slots; no
    /// control group behind it).
    Num6,
    /// Seventh contextual digit.
    Num7,
    /// Eighth contextual digit.
    Num8,
    /// Ninth contextual digit.
    Num9,
    /// Page up (menu scrolling).
    PageUp,
    /// Page down (menu scrolling).
    PageDown,
    /// Home (jump to list start).
    Home,
    /// End (jump to list end).
    End,
    /// Camera bookmark keys.
    F5,
    /// Camera bookmark keys.
    F6,
    /// Camera bookmark keys.
    F7,
    /// Camera bookmark keys.
    F8,
    /// Unbound by default; present so remapping can reach the full
    /// letter row (WASD panning, custom profiles).
    C,
    /// See [`Key::C`].
    D,
    /// See [`Key::C`].
    E,
    /// See [`Key::C`].
    F,
    /// See [`Key::C`].
    G,
    /// See [`Key::C`].
    I,
    /// See [`Key::C`].
    J,
    /// See [`Key::C`].
    K,
    /// See [`Key::C`].
    L,
    /// See [`Key::C`].
    M,
    /// See [`Key::C`].
    O,
    /// See [`Key::C`].
    Q,
    /// See [`Key::C`].
    T,
    /// See [`Key::C`].
    U,
    /// See [`Key::C`].
    V,
    /// See [`Key::C`].
    W,
    /// See [`Key::C`].
    Y,
    /// See [`Key::C`].
    Z,
}
