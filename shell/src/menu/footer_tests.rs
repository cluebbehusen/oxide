use super::*;

#[test]
fn the_footer_names_only_what_the_hands_in_use_have() {
    use crate::platform::{ALL_HANDS, assert_copy_fits};
    assert!(menu_footer(ALL_HANDS[0], true).ends_with("confirm - or click"));
    for hands in ALL_HANDS {
        for scrolls in [false, true] {
            assert_copy_fits(hands, &menu_footer(hands, scrolls));
        }
    }
}

#[test]
fn touch_rows_stay_a_full_fingertip_tall_and_desktop_rows_pack() {
    for s in [1.0, 1.25] {
        let long_list_short_window = (300.0 * s, 20);
        let touch = row_pitch(long_list_short_window.0, long_list_short_window.1, s, true);
        assert!(
            touch - ROW_GAP * s >= crate::theme::MIN_TOUCH_TARGET * s,
            "a touch row's target is at least a fingertip"
        );
        let desktop = row_pitch(long_list_short_window.0, long_list_short_window.1, s, false);
        assert_eq!(desktop, MIN_ROW * s, "desktop packs before it scrolls");
        assert_eq!(row_pitch(600.0 * s, 5, s, false), ITEM_HEIGHT * s);
        assert_eq!(row_pitch(600.0 * s, 5, s, true), touch, "one touch pitch");
    }
}

#[test]
fn a_keyless_footer_offers_a_scroll_only_when_the_list_scrolls() {
    let [_, mouse, _, touch] = crate::platform::ALL_HANDS;
    assert_eq!(menu_footer(touch, false), "tap to choose");
    assert!(menu_footer(touch, true).ends_with("drag to scroll"));
    assert_eq!(menu_footer(mouse, false), "click to choose");
    assert!(menu_footer(mouse, true).ends_with("scroll for more"));
}
