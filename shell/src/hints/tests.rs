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
