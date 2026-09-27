//! Platform facts the shell's presentation depends on.

/// Whether this build's only pointer is a fingertip: no hardware keys,
/// mouse, hover, or wheel reach the app. Code branches on this constant
/// rather than `#[cfg]` so both sides compile, lint, and test on every
/// target; pure helpers take it as a parameter.
pub(crate) const TOUCH_ONLY: bool = cfg!(target_os = "ios");

/// The pointer verb opening an instruction: a touch player taps what a
/// mouse player clicks.
pub(crate) fn tap_or_click_capitalized(touch_only: bool) -> &'static str {
    if touch_only { "Tap" } else { "Click" }
}

/// Fails when touch-facing copy names a key, a mouse action, or an
/// unexpanded key token.
#[cfg(test)]
pub(crate) fn assert_touch_copy(text: &str) {
    let lower = text.to_ascii_lowercase().replace("long-press", "");
    for word in [
        "click", "press", "enter", "esc", "hover", "wheel", "shift", "ctrl", "{", "[",
    ] {
        assert!(
            !lower.contains(word),
            "touch copy {text:?} mentions {word:?}"
        );
    }
    assert!(text.is_ascii(), "touch copy {text:?} is not ASCII");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_helpers_speak_touch_on_touch_only_builds() {
        assert_eq!(tap_or_click_capitalized(false), "Click");
        assert_touch_copy(tap_or_click_capitalized(true));
    }
}
