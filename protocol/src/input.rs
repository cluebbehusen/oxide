//! Raw input events — the shell's single input funnel.
//!
//! Every frame the shell turns whatever macroquad reports (and whatever
//! arrived via [`crate::Request::InjectEvent`]) into a list of these, then
//! maps them to camera operations, selection changes, and sim commands.
//! Injected and hardware events take the identical path, which is what makes
//! presentation-layer tests trustworthy without OS-level input faking.
//!
//! Touch variants exist now, unused, so the mobile ports extend this enum
//! instead of growing a second funnel.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Arm attack-move: the next left-click orders it.
    A,
    /// Pause / unpause.
    P,
    /// Activate the highlighted menu item.
    Enter,
    /// Deselect.
    Escape,
    /// Center the camera on your Foundry.
    Space,
    /// Toggle debug overlay.
    F1,
}
