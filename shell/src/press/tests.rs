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

fn touch_down(id: u64, y: f32) -> RawEvent {
    RawEvent::TouchDown { id, x: 10.0, y }
}

fn touch_move(id: u64, y: f32) -> RawEvent {
    RawEvent::TouchMove { id, x: 10.0, y }
}

fn touch_up(id: u64, y: f32) -> RawEvent {
    RawEvent::TouchUp { id, x: 10.0, y }
}

#[test]
fn a_finger_within_the_slop_still_taps() {
    let ui = 1.5;
    let mut press = ScrollPress::default();
    assert_eq!(press.feed(&touch_down(3, 100.0), ui, zone), Swipe::Held);
    let inside = 100.0 + 7.9 * ui;
    assert_eq!(press.feed(&touch_move(3, inside), ui, zone), Swipe::Held);
    assert_eq!(
        press.feed(&touch_up(3, inside), ui, zone),
        Swipe::Activated(1)
    );
}

#[test]
fn crossing_the_slop_scrolls_the_whole_way_and_cancels_the_tap() {
    let ui = 1.5;
    let mut press = ScrollPress::default();
    press.feed(&touch_down(3, 100.0), ui, zone);
    let past = 100.0 + 8.1 * ui;
    assert_eq!(
        press.feed(&touch_move(3, past), ui, zone),
        Swipe::Scrolled {
            dy: past - 100.0,
            began: true
        },
        "the first report catches up the travel since touchdown"
    );
    assert_eq!(
        press.feed(&touch_move(3, past - 5.0), ui, zone),
        Swipe::Scrolled {
            dy: -5.0,
            began: false
        }
    );
    assert_eq!(
        press.feed(&touch_up(3, 100.0), ui, zone),
        Swipe::Held,
        "a drag never taps, even back where it began"
    );
    assert!(!press.scrolling());
    assert_eq!(press.feed(&touch_down(4, 100.0), ui, zone), Swipe::Held);
    assert_eq!(
        press.feed(&touch_up(4, 100.0), ui, zone),
        Swipe::Activated(1),
        "the next finger taps afresh"
    );
}

#[test]
fn a_second_finger_neither_taps_nor_scrolls_while_the_first_drags() {
    let mut press = ScrollPress::default();
    press.feed(&touch_down(3, 100.0), 1.0, zone);
    press.feed(&touch_move(3, 140.0), 1.0, zone);
    assert!(press.scrolling());
    assert_eq!(press.feed(&touch_down(4, 100.0), 1.0, zone), Swipe::Held);
    assert_eq!(press.feed(&touch_move(4, 300.0), 1.0, zone), Swipe::Held);
    assert_eq!(press.feed(&touch_up(4, 100.0), 1.0, zone), Swipe::Held);
    assert_eq!(
        press.feed(&touch_move(3, 150.0), 1.0, zone),
        Swipe::Scrolled {
            dy: 10.0,
            began: false
        }
    );
}
