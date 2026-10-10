//! Raw input events: the shell's single input funnel.
//!
//! Every frame the shell turns whatever macroquad reports (and whatever
//! arrived via [`crate::Request::InjectEvent`]) into a list of these, then
//! maps them to camera operations, selection changes, and sim commands.
//! Injected and hardware events take the same path, so presentation-layer
//! tests need no OS-level input faking.
//!
//! Touch variants flow through the shell's touch handling (tap select, drag
//! pan, pinch zoom).

use serde::{Deserialize, Serialize};

/// One input event, in logical window pixels where applicable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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
    /// The platform took a touch away (a system gesture, palm
    /// rejection). The finger ends without completing any gesture.
    TouchCancel {
        /// Stable touch id.
        id: u64,
    },
    /// A typed character, for text entry (save names). The shell emits
    /// these only for printable ASCII, since UI strings stay ASCII and the
    /// menu font is Latin-1. Screens consume them only while a text field
    /// has focus; letters stay semantic everywhere else.
    Text {
        /// The character as typed (layout- and shift-resolved).
        ch: char,
    },
}

#[cfg(test)]
mod tests;

/// Mouse buttons the shell cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Select / drag-select.
    Left,
    /// Context order (move / attack / harvest).
    Right,
    /// Camera drag-pan.
    Middle,
}

/// The keys the shell maps; extend alongside the input mapper. The shell's
/// binding map assigns their actions, so variant docs describe default
/// bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    /// Contextual navigation shortcut.
    Tab,
    /// Pan up.
    Up,
    /// Pan down.
    Down,
    /// Pan left.
    Left,
    /// Pan right.
    Right,
    /// Letter H.
    H,
    /// Letter S.
    S,
    /// Letter A.
    A,
    /// Pause / unpause.
    P,
    /// Arm a patrol route; pressed again, starts it.
    R,
    /// Open the build palette (harvester selected).
    B,
    /// Select and center the next idle own Harvester.
    N,
    /// Stop, scrap the selected construction site, or clear focus.
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
    /// Letter C; physical keys are interpreted by the shell binding map.
    C,
    /// See [`Key::C`].
    D,
    /// See [`Key::C`].
    E,
    /// Arm hunt in the classic profile.
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
    /// Delete the character before the caret (text entry only; not a
    /// bindable game verb).
    Backspace,
}
