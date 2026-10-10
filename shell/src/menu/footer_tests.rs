use super::*;

#[test]
fn the_footer_speaks_touch_on_touch_only_builds() {
    assert!(menu_footer(false, true).ends_with("confirm - or click"));
    crate::platform::assert_touch_copy(&menu_footer(true, true));
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
fn the_touch_footer_offers_a_drag_only_when_the_list_scrolls() {
    assert_eq!(menu_footer(true, false), "tap to choose");
    assert!(menu_footer(true, true).ends_with("drag to scroll"));
}
