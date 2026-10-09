use super::*;
use macroquad::prelude::vec2;
use oxide_protocol::{Key, RawEvent};

fn sectioned() -> Menu {
    // rows: [H] a b [H] c d
    Menu::with_headers(
        "T",
        ["- one -", "a", "b", "- two -", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect(),
        vec![0, 3],
    )
}

fn press(menu: &mut Menu, key: Key) -> Option<usize> {
    let mut mouse = vec2(0.0, 0.0);
    menu.handle(&[RawEvent::KeyDown { key }], &mut mouse)
}

#[test]
fn the_cursor_never_rests_on_a_header() {
    let mut menu = sectioned();
    assert_eq!(menu.selected, 1, "construction snaps off the header");
    press(&mut menu, Key::Down);
    assert_eq!(menu.selected, 2);
    press(&mut menu, Key::Down);
    assert_eq!(menu.selected, 4, "down hops the section label");
    press(&mut menu, Key::Up);
    assert_eq!(menu.selected, 2, "up hops it too");
    press(&mut menu, Key::Home);
    assert_eq!(menu.selected, 1, "Home lands on the first real row");
    press(&mut menu, Key::End);
    assert_eq!(menu.selected, 5, "End on the last real row");
}

#[test]
fn a_header_never_activates() {
    let mut menu = sectioned();
    menu.select(0);
    assert_eq!(menu.selected, 1, "select snaps forward off the header");
    // Enter activates the snapped row, never the header.
    assert_eq!(press(&mut menu, Key::Enter), Some(1));
}

#[test]
fn an_all_header_menu_refuses_keyboard_activation() {
    let mut menu = Menu::with_headers(
        "HEADERS",
        vec!["one".to_string(), "two".to_string()],
        vec![0, 1],
    );

    assert!(menu.is_header(menu.selected));
    assert_eq!(press(&mut menu, Key::Enter), None);
}

#[test]
fn wheel_scroll_cannot_pin_the_cursor_onto_a_header() {
    // A short window forces the riding clamp; the ride must snap off
    // headers or Enter activates a section label.
    let mut menu = sectioned();
    crate::render::set_viewport(1280.0, 400.0);
    menu.select(5);
    let mut mouse = vec2(0.0, 0.0);
    for _ in 0..6 {
        menu.handle(&[RawEvent::Wheel { delta: 1.0 }], &mut mouse);
        assert!(
            !menu.is_header(menu.selected),
            "the riding cursor rests on header row {}",
            menu.selected
        );
    }
}

#[test]
fn mouse_activation_requires_a_release_on_the_armed_row() {
    crate::render::set_viewport(1280.0, 800.0);
    let mut menu = sectioned();
    let first = menu.item_rect(1).expect("first item").center();
    let second = menu.item_rect(2).expect("second item").center();
    let mut mouse = vec2(0.0, 0.0);

    assert_eq!(
        menu.handle(
            &[RawEvent::MouseMove {
                x: second.x,
                y: second.y,
            }],
            &mut mouse,
        ),
        None
    );
    assert_eq!(menu.hover(), Some(2));
    assert_eq!(menu.selected, 1, "hover is not keyboard selection");

    assert_eq!(
        menu.handle(
            &[
                RawEvent::MouseDown {
                    button: oxide_protocol::MouseButton::Left,
                    x: first.x,
                    y: first.y,
                },
                RawEvent::MouseUp {
                    button: oxide_protocol::MouseButton::Left,
                    x: second.x,
                    y: second.y,
                },
            ],
            &mut mouse,
        ),
        None,
        "dragging to another row cancels the press"
    );
    assert_eq!(
        menu.handle(
            &[
                RawEvent::MouseDown {
                    button: oxide_protocol::MouseButton::Left,
                    x: second.x,
                    y: second.y,
                },
                RawEvent::MouseUp {
                    button: oxide_protocol::MouseButton::Left,
                    x: second.x,
                    y: second.y,
                },
            ],
            &mut mouse,
        ),
        Some(2)
    );
    assert_eq!(menu.selected, 2);
}

#[test]
fn paging_keeps_the_keyboard_target_visible_and_off_section_headers() {
    crate::render::set_viewport(1280.0, 400.0);
    let mut menu = sectioned();
    let mut mouse = vec2(0.0, 0.0);
    for key in [Key::PageDown, Key::PageDown, Key::PageUp, Key::PageUp] {
        menu.handle(&[RawEvent::KeyDown { key }], &mut mouse);
        let [first, end] = menu.visible_range();
        assert!((first..end).contains(&menu.selected));
        assert!(!menu.is_header(menu.selected));
    }
}

#[test]
fn fractional_trackpad_motion_accumulates_before_scrolling_a_row() {
    crate::render::set_viewport(1280.0, 400.0);
    let mut menu = sectioned();
    let mut mouse = vec2(0.0, 0.0);
    let start = menu.visible_range();
    for _ in 0..2 {
        menu.handle(&[RawEvent::Wheel { delta: -0.4 }], &mut mouse);
        assert_eq!(menu.visible_range(), start);
    }
    menu.handle(&[RawEvent::Wheel { delta: -0.4 }], &mut mouse);
    assert_ne!(
        menu.visible_range(),
        start,
        "three partial notches cross one row"
    );
}
