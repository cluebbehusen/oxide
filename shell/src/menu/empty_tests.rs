use super::*;
use macroquad::prelude::vec2;
use oxide_protocol::{Key, RawEvent};

#[test]
fn an_empty_menu_survives_every_key() {
    // A fresh profile's replay shelf has zero rows; wrap-around
    // arithmetic on an empty list once divided by zero.
    let mut menu = Menu::new("EMPTY", Vec::new());
    let mut mouse = vec2(0.0, 0.0);
    for key in [Key::Up, Key::Down, Key::Enter, Key::PageDown, Key::End] {
        let events = [RawEvent::KeyDown { key }];
        assert_eq!(menu.handle(&events, &mut mouse), None);
    }
}

#[test]
fn a_touch_tap_activates_the_visible_row_under_the_finger() {
    crate::render::set_viewport(1280.0, 800.0);
    let mut menu = Menu::new("TOUCH", vec!["one".to_string(), "two".to_string()]);
    let row = menu.item_rect(1).expect("second row is visible");
    let x = row.x + row.w * 0.5;
    let y = row.y + row.h * 0.5;
    let mut mouse = vec2(0.0, 0.0);
    assert_eq!(
        menu.handle(
            &[
                RawEvent::TouchDown { id: 7, x, y },
                RawEvent::TouchUp { id: 7, x, y },
            ],
            &mut mouse,
        ),
        Some(1)
    );
    assert_eq!(menu.selected, 1);
}

fn drag(menu: &mut Menu, id: u64, x: f32, from: f32, to: f32) -> Option<usize> {
    let mut mouse = vec2(0.0, 0.0);
    let mut events = vec![RawEvent::TouchDown { id, x, y: from }];
    let steps = 12;
    for step in 1..=steps {
        let y = from + (to - from) * step as f32 / steps as f32;
        events.push(RawEvent::TouchMove { id, x, y });
    }
    events.push(RawEvent::TouchUp { id, x, y: to });
    menu.handle(&events, &mut mouse)
}

#[test]
fn a_touch_drag_scrolls_a_long_list_without_activating() {
    crate::render::set_viewport(1280.0, 400.0);
    let mut items: Vec<String> = (0..39).map(|i| format!("row {i}")).collect();
    items.push("Back".to_string());
    let mut menu = Menu::new("LONG", items);
    assert!(menu.item_rect(39).is_none(), "Back starts below the window");
    let x = menu.item_rect(0).expect("first row").center().x;
    for _ in 0..10 {
        assert_eq!(
            drag(&mut menu, 7, x, 330.0, 180.0),
            None,
            "a drag never activates"
        );
    }
    let back = menu
        .item_rect(39)
        .expect("dragging up reveals Back")
        .center();
    let mut mouse = vec2(0.0, 0.0);
    let tapped = menu.handle(
        &[
            RawEvent::TouchDown {
                id: 8,
                x: back.x,
                y: back.y,
            },
            RawEvent::TouchUp {
                id: 8,
                x: back.x,
                y: back.y,
            },
        ],
        &mut mouse,
    );
    assert_eq!(tapped, Some(39));
    drag(&mut menu, 9, x, 180.0, 330.0);
    assert_eq!(
        menu.visible_range()[0],
        30,
        "dragging down 150 px of 30 px rows shows five earlier rows"
    );
}

#[test]
fn a_touch_within_the_slop_still_activates_its_row() {
    crate::render::set_viewport(1280.0, 800.0);
    let mut menu = Menu::new("TOUCH", vec!["one".to_string(), "two".to_string()]);
    let row = menu.item_rect(1).expect("second row").center();
    assert_eq!(drag(&mut menu, 7, row.x, row.y, row.y + 3.0), Some(1));
}

#[test]
fn a_drag_that_began_on_a_row_never_commits_it() {
    crate::render::set_viewport(1280.0, 800.0);
    let mut menu = Menu::new("TOUCH", vec!["one".to_string(), "two".to_string()]);
    let row = menu.item_rect(1).expect("second row").center();
    let mut mouse = vec2(0.0, 0.0);
    let events = [
        RawEvent::TouchDown {
            id: 7,
            x: row.x,
            y: row.y,
        },
        RawEvent::TouchMove {
            id: 7,
            x: row.x,
            y: row.y + 40.0,
        },
        RawEvent::TouchMove {
            id: 7,
            x: row.x,
            y: row.y,
        },
        RawEvent::TouchUp {
            id: 7,
            x: row.x,
            y: row.y,
        },
    ];
    assert_eq!(menu.handle(&events, &mut mouse), None);
}

#[test]
fn a_touch_gesture_belongs_to_its_first_finger_and_armed_row() {
    crate::render::set_viewport(1280.0, 800.0);
    let mut menu = Menu::new(
        "TOUCH",
        vec!["one".to_string(), "two".to_string(), "three".to_string()],
    );
    let first = menu.item_rect(0).expect("first row").center();
    let second = menu.item_rect(1).expect("second row").center();
    let mut mouse = vec2(0.0, 0.0);

    assert_eq!(
        menu.handle(
            &[RawEvent::TouchDown {
                id: 7,
                x: second.x,
                y: second.y,
            }],
            &mut mouse,
        ),
        None
    );
    assert_eq!(menu.press.armed_touch(), Some((7, 1)));

    // A second finger cannot steal or resolve the first finger's press.
    assert_eq!(
        menu.handle(
            &[
                RawEvent::TouchDown {
                    id: 8,
                    x: first.x,
                    y: first.y,
                },
                RawEvent::TouchUp {
                    id: 8,
                    x: first.x,
                    y: first.y,
                },
            ],
            &mut mouse,
        ),
        None
    );
    assert_eq!(menu.press.armed_touch(), Some((7, 1)));
    assert_eq!(menu.selected, 0);

    // The owning finger resolves on another row, so the gesture cancels.
    assert_eq!(
        menu.handle(
            &[
                RawEvent::TouchMove {
                    id: 7,
                    x: first.x,
                    y: first.y,
                },
                RawEvent::TouchUp {
                    id: 7,
                    x: first.x,
                    y: first.y,
                },
            ],
            &mut mouse,
        ),
        None
    );
    assert_eq!(menu.press.armed_touch(), None);
    assert_eq!(menu.selected, 0);
}
