use super::*;

fn zone(p: Vec2, _touch: bool) -> Option<u8> {
    (p.x < 100.0).then_some(1)
}

#[test]
fn feed_commits_only_a_release_on_the_armed_zone() {
    let mut press = Press::default();
    let down = |x| RawEvent::MouseDown {
        button: MouseButton::Left,
        x,
        y: 0.0,
    };
    let up = |x| RawEvent::MouseUp {
        button: MouseButton::Left,
        x,
        y: 0.0,
    };
    assert_eq!(press.feed(&down(10.0), zone), Fed::Held);
    assert_eq!(press.feed(&up(20.0), zone), Fed::Activated(1));
    assert_eq!(press.feed(&down(10.0), zone), Fed::Held);
    assert_eq!(
        press.feed(&up(500.0), zone),
        Fed::Held,
        "a press dragged away still belongs to the button"
    );
    let touch = RawEvent::TouchDown {
        id: 3,
        x: 10.0,
        y: 0.0,
    };
    assert_eq!(press.feed(&touch, zone), Fed::Held);
    let lift = RawEvent::TouchUp {
        id: 3,
        x: 12.0,
        y: 0.0,
    };
    assert_eq!(press.feed(&lift, zone), Fed::Activated(1));
}

#[test]
fn feed_leaves_unarmed_events_to_the_caller() {
    let mut press = Press::default();
    let away = RawEvent::TouchDown {
        id: 3,
        x: 500.0,
        y: 0.0,
    };
    assert_eq!(press.feed(&away, zone), Fed::Ignored);
    let drag = RawEvent::TouchMove {
        id: 3,
        x: 10.0,
        y: 0.0,
    };
    assert_eq!(press.feed(&drag, zone), Fed::Ignored);
    let release = RawEvent::MouseUp {
        button: MouseButton::Left,
        x: 10.0,
        y: 0.0,
    };
    assert_eq!(press.feed(&release, zone), Fed::Ignored);
}

#[test]
fn feed_ignores_a_re_reported_start_for_the_owning_finger() {
    let mut press = Press::default();
    let start = RawEvent::TouchDown {
        id: 3,
        x: 10.0,
        y: 0.0,
    };
    assert_eq!(press.feed(&start, zone), Fed::Held);
    let repeat = RawEvent::TouchDown {
        id: 3,
        x: 11.0,
        y: 0.0,
    };
    assert_eq!(press.feed(&repeat, zone), Fed::Held);
    assert_eq!(press.armed_touch(), Some((3, 1)));
    let other = RawEvent::TouchDown {
        id: 4,
        x: 10.0,
        y: 0.0,
    };
    assert_eq!(
        press.feed(&other, zone),
        Fed::Ignored,
        "a second finger cannot steal the press"
    );
}

#[test]
fn a_mouse_press_commits_only_when_released_on_the_armed_zone() {
    let mut press = Press::default();
    press.mouse_down(Some(2));
    assert_eq!(press.mouse_up(Some(2)), Some(2));

    press.mouse_down(Some(2));
    assert_eq!(press.mouse_up(Some(3)), None);
    assert_eq!(press.mouse_up(Some(2)), None, "a cancelled press is spent");

    press.mouse_down(None);
    assert_eq!(press.mouse_up(Some(2)), None);
}

#[test]
fn a_touch_gesture_belongs_to_the_finger_that_armed_it() {
    let mut press = Press::default();
    assert!(press.touch_free());
    press.touch_down(7, Some(1));
    assert!(!press.touch_free());
    assert!(press.owns(7));
    assert!(!press.owns(8));

    assert_eq!(press.touch_up(Some(0)), None, "lifting elsewhere cancels");
    assert!(press.touch_free());

    press.touch_down(9, Some(0));
    assert_eq!(press.touch_up(Some(0)), Some(0));
}

#[test]
fn a_touch_that_hits_nothing_leaves_the_gesture_free() {
    let mut press = Press::<usize>::default();
    press.touch_down(7, None);
    assert!(press.touch_free());
    assert!(!press.owns(7));
}

#[test]
fn cancel_disarms_mouse_and_touch() {
    let mut press = Press::default();
    press.mouse_down(Some(4));
    press.touch_down(7, Some(4));
    press.cancel();
    assert_eq!(press.mouse_up(Some(4)), None);
    assert!(press.touch_free());
}
