use super::*;

#[test]
fn instruction_helpers_speak_touch_on_touch_only_builds() {
    assert_eq!(tap_or_click_capitalized(false), "Click");
    assert_touch_copy(tap_or_click_capitalized(true));
}
