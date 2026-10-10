use super::*;

#[test]
fn the_back_button_claims_its_presses_and_passes_the_rest() {
    let mut back = BackButton::default();
    for touch in [false, true] {
        let (pressed, rest) = back.route(&press_back(touch));
        assert!(pressed);
        assert!(rest.is_empty(), "the screen never sees the press");
    }
    // Pressed on BACK, released elsewhere: nothing, and the release
    // stays the button's.
    let p = corner_slot(0, crate::render::ui_scale()).center();
    let (pressed, rest) = back.route(&[
        RawEvent::TouchDown {
            id: 1,
            x: p.x,
            y: p.y,
        },
        RawEvent::TouchUp {
            id: 1,
            x: 600.0,
            y: 400.0,
        },
    ]);
    assert!(!pressed);
    assert!(rest.is_empty());
    let elsewhere = [RawEvent::TouchDown {
        id: 2,
        x: 600.0,
        y: 400.0,
    }];
    assert_eq!(back.route(&elsewhere), (false, elsewhere.to_vec()));
}

#[test]
fn corner_slots_are_fingertip_sized_and_never_overlap() {
    for s in [0.75, 1.0, 1.5] {
        let first = corner_slot(0, s);
        let second = corner_slot(1, s);
        assert!(first.h >= crate::theme::MIN_TOUCH_TARGET * s);
        assert!(first.x > 0.0 && first.y > 0.0);
        assert!(first.x + first.w < second.x, "slots keep a gap at {s}");
    }
}
