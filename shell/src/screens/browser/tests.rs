use super::*;
use oxide_protocol::MouseButton;

#[test]
fn the_map_hint_speaks_touch_on_touch_only_builds() {
    assert!(browser_hint(false).contains("{confirm}"));
    crate::platform::assert_touch_copy(browser_hint(true));
}

fn entry(label: &str, seats: usize) -> ScenarioEntry {
    ScenarioEntry {
        seats,
        label: label.to_string(),
        blurb: None,
        path: Some(std::path::PathBuf::from(format!("{label}.json"))),
        theme: String::new(),
    }
}

fn shelf() -> Vec<ScenarioEntry> {
    let mut v: Vec<ScenarioEntry> = (0..9).map(|i| entry(&format!("m{i}"), 2)).collect();
    v.push(entry("t0", 4));
    v.push(entry("t1", 4));
    v
}

fn press(b: &mut Browser, entries: &[ScenarioEntry], key: Key) -> Out {
    let mut mouse = vec2(0.0, 0.0);
    b.handle(entries, &[RawEvent::KeyDown { key }], &mut mouse)
}

#[test]
fn arrows_walk_the_grid_by_row_and_column() {
    let entries = shelf();
    let mut b = Browser::new();
    press(&mut b, &entries, Key::Right);
    assert_eq!(b.selected, 1);
    press(&mut b, &entries, Key::Down);
    assert_eq!(b.selected, 5, "down keeps the column (4-wide grid)");
    press(&mut b, &entries, Key::Down);
    assert_eq!(b.selected, 8, "a short row clamps the column");
    press(&mut b, &entries, Key::Down);
    assert_eq!(b.selected, 9, "and the next step crosses the section");
    press(&mut b, &entries, Key::Up);
    assert_eq!(b.selected, 8);
    press(&mut b, &entries, Key::Up);
    assert_eq!(
        b.selected, 4,
        "the column narrows through a one-wide row (standard grid feel)"
    );
}

#[test]
fn the_grid_wraps_at_its_edges() {
    let entries = shelf();
    let mut b = Browser::new();
    press(&mut b, &entries, Key::Left);
    assert_eq!(b.selected, 10, "left from the first card wraps to the last");
    press(&mut b, &entries, Key::Right);
    assert_eq!(b.selected, 0, "right from the last card wraps to the first");
    press(&mut b, &entries, Key::Right);
    press(&mut b, &entries, Key::Up);
    assert_eq!(
        b.selected, 10,
        "up from the top row wraps to the bottom row"
    );
    press(&mut b, &entries, Key::Down);
    assert_eq!(b.selected, 1, "down from the bottom row wraps to the top");
    press(&mut b, &entries, Key::Right);
    press(&mut b, &entries, Key::Right);
    press(&mut b, &entries, Key::Right);
    assert_eq!(
        b.selected, 4,
        "right from a row's end takes the next row's first"
    );
}

#[test]
fn paging_moves_whole_rows_and_stops_at_the_ends() {
    let entries = shelf();
    let mut b = Browser::new();
    press(&mut b, &entries, Key::PageDown);
    assert!(b.selected > 0);
    for _ in 0..4 {
        press(&mut b, &entries, Key::PageDown);
    }
    assert!(b.selected >= 9, "paging settles on the bottom row");
    let bottom = b.selected;
    press(&mut b, &entries, Key::PageDown);
    assert_eq!(b.selected, bottom, "and stops there");
    for _ in 0..5 {
        press(&mut b, &entries, Key::PageUp);
    }
    assert_eq!(b.selected, 0, "paging back stops on the top row");
}

#[test]
fn wheel_scroll_moves_the_window_and_only_the_window() {
    let entries = shelf();
    let mut b = Browser::new();
    let mut mouse = vec2(0.0, 0.0);
    // Scroll far past the first section across separate frames, so a
    // per-frame snap-back to the selection would show. Browsing must not
    // retarget Enter.
    for _ in 0..8 {
        b.handle(&entries, &[RawEvent::Wheel { delta: -1.0 }], &mut mouse);
        b.handle(&entries, &[], &mut mouse);
        assert_eq!(b.selected, 0, "the wheel chose a map");
    }
    // The window moved (and stopped with its tail against the
    // viewport — see `the_wheel_stops_at_the_last_full_screenful`).
    assert!(b.scroll_y > 0.0, "the window actually moved");

    // Enter with the selection off screen scrolls it back first;
    // only the second Enter commits — it never fires blind.
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    let GridMetrics {
        top: shelf_top,
        bottom: shelf_bottom,
        ..
    } = metrics(view, ui);
    assert!(
        !b.layout(&entries, view, ui)
            .cards
            .iter()
            .any(|(e, rect)| *e == b.selected
                && rect.y >= shelf_top
                && rect.y + rect.h <= shelf_bottom),
        "precondition: the selection is not fully visible"
    );
    let out = b.handle(
        &entries,
        &[RawEvent::KeyDown { key: Key::Enter }],
        &mut mouse,
    );
    assert_eq!(out, Out::Stay, "the first Enter only scrolls back");
    assert!(
        b.layout(&entries, view, ui)
            .cards
            .iter()
            .any(|(e, rect)| *e == b.selected
                && rect.y >= shelf_top
                && rect.y + rect.h <= shelf_bottom),
        "the selection is fully back on screen"
    );
    let out = b.handle(
        &entries,
        &[RawEvent::KeyDown { key: Key::Enter }],
        &mut mouse,
    );
    assert_eq!(out, Out::Pick(0), "the second Enter commits");
}

#[test]
fn the_wheel_stops_at_the_last_full_screenful() {
    let entries = shelf();
    let mut b = Browser::new();
    let mut mouse = vec2(0.0, 0.0);
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    let full = b.layout(&entries, view, ui).cards.len();
    assert!(full >= 4, "precondition: the window shows several cards");
    // Scroll far past the end: the wheel must stop with the tail at the
    // bottom, never parking the last row alone at the top of an empty
    // screen.
    for _ in 0..60 {
        b.handle(&entries, &[RawEvent::Wheel { delta: -1.0 }], &mut mouse);
    }
    let end = b.layout(&entries, view, ui);
    assert!(!end.more_below, "the shelf's tail is on screen");
    let shelf_bottom = metrics(view, ui).bottom;
    let tail_bottom = end
        .cards
        .iter()
        .map(|(_, rect)| rect.y + rect.h)
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        (tail_bottom - shelf_bottom).abs() < 0.01,
        "the tail sits against the viewport instead of above empty space"
    );
    assert_eq!(
        end.scroll_offset, end.scroll_max,
        "the pixel clamp lands exactly at the shelf tail"
    );
    // Moving back by any visible amount puts the tail below the
    // viewport again.
    b.handle(&entries, &[RawEvent::Wheel { delta: 1.0 }], &mut mouse);
    assert!(
        b.layout(&entries, view, ui).more_below,
        "one line above the clamp, the tail is off screen"
    );
    // The keyboard still reaches the last row.
    b.handle(&entries, &[RawEvent::KeyDown { key: Key::End }], &mut mouse);
    assert_eq!(b.selected, entries.len() - 1);
    assert!(
        b.layout(&entries, view, ui)
            .cards
            .iter()
            .any(|(e, _)| *e == b.selected),
        "End shows the last card"
    );
}

#[test]
fn trackpad_fractions_scroll_immediately_and_smoothly() {
    let entries = shelf();
    let mut b = Browser::new();
    let mut mouse = vec2(0.0, 0.0);
    let before = b.layout(
        &entries,
        crate::render::viewport(),
        crate::render::ui_scale(),
    );
    b.handle(&entries, &[RawEvent::Wheel { delta: -0.11 }], &mut mouse);
    let first = b.scroll_y;
    assert!(first > 0.0, "a fractional trackpad event moves immediately");
    assert!(
        first < before.viewport_height / 4.0,
        "a tiny gesture cannot jump a substantial part of the shelf"
    );
    b.handle(&entries, &[RawEvent::Wheel { delta: -0.11 }], &mut mouse);
    assert!(
        b.scroll_y > first && b.scroll_y < first * 2.1,
        "equal fractional events produce continuous pixel motion"
    );
}

#[test]
fn a_resize_scrolls_the_window_back_to_the_selection() {
    let entries = shelf();
    let mut b = Browser::new();
    let mut mouse = vec2(0.0, 0.0);
    b.handle(&entries, &[RawEvent::KeyDown { key: Key::End }], &mut mouse);
    // The window shrinks out from under the tail selection; the
    // next frame must scroll back to it, or Enter fires a card
    // the player cannot see.
    crate::render::set_viewport(640.0, 400.0);
    b.handle(
        &entries,
        &[RawEvent::MouseMove { x: 0.0, y: 0.0 }],
        &mut mouse,
    );
    let ui = crate::render::ui_scale();
    let visible: Vec<usize> = b
        .layout(&entries, vec2(640.0, 400.0), ui)
        .cards
        .iter()
        .map(|(e, _)| *e)
        .collect();
    assert!(
        visible.contains(&b.selected),
        "the selection is back on screen (visible {visible:?}, selected {})",
        b.selected
    );
}

#[test]
fn a_small_window_at_max_scale_still_shows_cards() {
    let entries = shelf();
    let b = Browser::new();
    // 640x400 at the 150% user scale must still fit a card row, or
    // headings draw while Enter fires a card nobody can see.
    let layout = b.layout(&entries, vec2(640.0, 400.0), 1.5);
    assert!(
        !layout.cards.is_empty(),
        "at least one card row fits every supported window"
    );
    for (_, r) in &layout.cards {
        assert!(r.h > 20.0, "cards stay tall enough to read and click");
    }
}

#[test]
fn enter_picks_and_escape_backs_out() {
    let entries = shelf();
    let mut b = Browser::new();
    assert_eq!(press(&mut b, &entries, Key::End), Out::Stay);
    assert_eq!(b.selected, 10);
    assert_eq!(press(&mut b, &entries, Key::Enter), Out::Pick(10));
    assert_eq!(press(&mut b, &entries, Key::Escape), Out::Back);
}

#[test]
fn clicks_commit_on_release_inside_the_same_card() {
    let entries = shelf();
    let mut b = Browser::new();
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    let layout = b.layout(&entries, view, ui);
    let (target, rect) = layout.cards[2];
    let (cx, cy) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    let mut mouse = vec2(0.0, 0.0);
    let click = [
        RawEvent::MouseDown {
            button: MouseButton::Left,
            x: cx,
            y: cy,
        },
        RawEvent::MouseUp {
            button: MouseButton::Left,
            x: cx,
            y: cy,
        },
    ];
    // The first click on a non-selected card selects it; browsing by
    // pointer must not misfire a launch.
    let out = b.handle(&entries, &click, &mut mouse);
    assert_eq!(out, Out::Stay, "the first click only selects");
    assert_eq!(b.selected, target);
    // The second click on the now-selected card commits.
    let out = b.handle(&entries, &click, &mut mouse);
    assert_eq!(out, Out::Pick(target));
    // Dragging away cancels.
    let out = b.handle(
        &entries,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: cx,
                y: cy,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: cx + rect.w * 2.0,
                y: cy,
            },
        ],
        &mut mouse,
    );
    assert_eq!(out, Out::Stay);
}

#[test]
fn touch_drag_scrolls_while_touch_taps_select_and_commit() {
    let entries = shelf();
    let mut b = Browser::new();
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    let (_, rect) = b.layout(&entries, view, ui).cards[2];
    let (x, y) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    let mut mouse = vec2(0.0, 0.0);

    b.handle(&entries, &[RawEvent::TouchDown { id: 1, x, y }], &mut mouse);
    let out = b.handle(
        &entries,
        &[RawEvent::TouchMove {
            id: 1,
            x,
            y: y - 30.0,
        }],
        &mut mouse,
    );
    assert_eq!(out, Out::Stay);
    assert!(b.scroll_y > 0.0, "an upward drag moves down the shelf");
    let out = b.handle(
        &entries,
        &[RawEvent::TouchUp {
            id: 1,
            x,
            y: y - 30.0,
        }],
        &mut mouse,
    );
    assert_eq!(out, Out::Stay, "a drag cannot activate a card");
    assert_eq!(b.selected, 0, "a drag cannot retarget keyboard focus");

    b.scroll_y = 0.0;
    let tap = [
        RawEvent::TouchDown { id: 2, x, y },
        RawEvent::TouchUp { id: 2, x, y },
    ];
    assert_eq!(b.handle(&entries, &tap, &mut mouse), Out::Stay);
    assert_eq!(b.selected, 2, "the first tap selects");
    assert_eq!(
        b.handle(&entries, &tap, &mut mouse),
        Out::Pick(2),
        "a second tap commits"
    );
}

#[test]
fn the_remembered_pick_is_found_by_path() {
    let entries = shelf();
    let mut b = Browser::new();
    b.select_path(&entries, entries[7].path.as_deref());
    assert_eq!(b.selected, 7);
    b.select_path(&entries, Some(std::path::Path::new("gone.json")));
    assert_eq!(b.selected, 7, "a vanished file keeps the old ground");
}

#[test]
fn enter_commits_a_card_scrolled_flush_with_the_shelf_bottom() {
    crate::render::set_viewport(640.0, 400.0);
    let entries: Vec<ScenarioEntry> = (0..33).map(|i| entry(&format!("m{i}"), 2)).collect();
    let mut b = Browser::new();
    press(&mut b, &entries, Key::End);
    let out = (0..2)
        .map(|_| press(&mut b, &entries, Key::Enter))
        .find(|out| *out != Out::Stay);
    crate::render::set_viewport(1280.0, 800.0);
    assert_eq!(out, Some(Out::Pick(32)));
}
