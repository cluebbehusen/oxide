//! Platform facts the shell's presentation depends on, and the hands the
//! player is using right now.

use std::cell::Cell;

use crate::input::Pointer;

/// Whether this build runs on a touch device, where a finger is always
/// at hand even if a keyboard or trackpad is attached. Platform facts
/// (no Quit, the on-screen keyboard) and fingertip sizing follow it;
/// wording and hover follow [`Hands`] instead. Code branches on this
/// constant rather than `#[cfg]` so both sides compile, lint, and test on
/// every target; pure helpers take it as a parameter.
pub(crate) const TOUCH_ONLY: bool = cfg!(target_os = "ios");

/// The pointer and keys the player is using, which decide how copy
/// speaks: tap or click, and whether it names keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hands {
    /// The pointer used last.
    pub pointer: Pointer,
    /// Whether a hardware key has been pressed.
    pub keys: bool,
}

impl Hands {
    /// What a build assumes before any input: a finger alone on a
    /// touch device, a mouse and keyboard elsewhere.
    pub const BUILD: Self = if TOUCH_ONLY {
        Self {
            pointer: Pointer::Touch,
            keys: false,
        }
    } else {
        Self {
            pointer: Pointer::Mouse,
            keys: true,
        }
    };

    /// Whether the pointer in use is a finger.
    pub fn touch(self) -> bool {
        self.pointer == Pointer::Touch
    }
}

thread_local! {
    /// This frame's hands. The build's default, so tests and headless
    /// runs see the copy they always have.
    static HANDS: Cell<Hands> = const { Cell::new(Hands::BUILD) };
}

/// Publishes this frame's hands for the copy code.
pub(crate) fn set_hands(hands: Hands) {
    HANDS.with(|cell| cell.set(hands));
}

/// This frame's hands.
pub(crate) fn hands() -> Hands {
    HANDS.with(Cell::get)
}

/// The pointer verb opening an instruction: a touch player taps what a
/// mouse player clicks.
pub(crate) fn tap_or_click_capitalized(touch_only: bool) -> &'static str {
    if touch_only { "Tap" } else { "Click" }
}

/// Fails when copy for a player without a keyboard names a key or an
/// unexpanded key token.
#[cfg(test)]
pub(crate) fn assert_keyless_copy(text: &str) {
    let lower = text.to_ascii_lowercase().replace("long-press", "");
    for word in ["press", "enter", "esc", "shift", "ctrl", "{", "["] {
        assert!(
            !lower.contains(word),
            "keyless copy {text:?} mentions {word:?}"
        );
    }
    assert!(text.is_ascii(), "copy {text:?} is not ASCII");
}

/// Fails when copy for a finger names a mouse action.
#[cfg(test)]
pub(crate) fn assert_mouseless_copy(text: &str) {
    let lower = text.to_ascii_lowercase();
    for word in ["click", "hover", "wheel", "middle drag"] {
        assert!(
            !lower.contains(word),
            "touch copy {text:?} mentions {word:?}"
        );
    }
    assert!(text.is_ascii(), "copy {text:?} is not ASCII");
}

/// Fails when touch-facing copy names a key, a mouse action, or an
/// unexpanded key token.
#[cfg(test)]
pub(crate) fn assert_touch_copy(text: &str) {
    assert_keyless_copy(text);
    assert_mouseless_copy(text);
}

/// Every pairing of pointer and keyboard.
#[cfg(test)]
pub(crate) const ALL_HANDS: [Hands; 4] = [
    Hands {
        pointer: Pointer::Mouse,
        keys: true,
    },
    Hands {
        pointer: Pointer::Mouse,
        keys: false,
    },
    Hands {
        pointer: Pointer::Touch,
        keys: true,
    },
    Hands {
        pointer: Pointer::Touch,
        keys: false,
    },
];

/// Fails when copy names what `hands` lack: a key without a keyboard, or
/// a mouse action under a finger.
#[cfg(test)]
pub(crate) fn assert_copy_fits(hands: Hands, text: &str) {
    if !hands.keys {
        assert_keyless_copy(text);
    }
    if hands.touch() {
        assert_mouseless_copy(text);
    }
    assert!(text.is_ascii(), "copy {text:?} is not ASCII");
}

#[cfg(test)]
mod tests;
