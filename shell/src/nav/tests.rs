use super::*;

fn all(_: usize) -> bool {
    true
}

#[test]
fn keys_decode_to_navigation_and_other_events_do_not() {
    assert_eq!(
        Nav::decode(&RawEvent::KeyDown { key: Key::Up }),
        Some(Nav::Up)
    );
    assert_eq!(
        Nav::decode(&RawEvent::KeyDown { key: Key::Enter }),
        Some(Nav::Confirm)
    );
    assert_eq!(
        Nav::decode(&RawEvent::KeyDown { key: Key::Escape }),
        Some(Nav::Back)
    );
    assert_eq!(Nav::decode(&RawEvent::KeyUp { key: Key::Up }), None);
    assert_eq!(Nav::decode(&RawEvent::KeyDown { key: Key::A }), None);
}

#[test]
fn line_arrows_wrap_at_both_ends() {
    assert_eq!(step_line(4, 3, Nav::Down, Axis::Vertical, 2, all), Some(0));
    assert_eq!(step_line(4, 0, Nav::Up, Axis::Vertical, 2, all), Some(3));
    assert_eq!(step_line(4, 1, Nav::Down, Axis::Vertical, 2, all), Some(2));
    assert_eq!(
        step_line(3, 2, Nav::Right, Axis::Horizontal, 1, all),
        Some(0)
    );
    assert_eq!(step_line(3, 0, Nav::Left, Axis::Both, 1, all), Some(2));
    assert_eq!(step_line(3, 0, Nav::Down, Axis::Both, 1, all), Some(1));
}

#[test]
fn line_arrows_across_the_axis_and_confirm_do_not_move() {
    assert_eq!(step_line(4, 1, Nav::Left, Axis::Vertical, 2, all), None);
    assert_eq!(step_line(4, 1, Nav::Up, Axis::Horizontal, 2, all), None);
    assert_eq!(step_line(4, 1, Nav::Confirm, Axis::Both, 2, all), None);
    assert_eq!(step_line(0, 0, Nav::Down, Axis::Vertical, 2, all), None);
}

#[test]
fn line_arrows_skip_dead_cells_including_across_the_wrap() {
    let live = |cell: usize| cell != 0 && cell != 3;
    assert_eq!(step_line(5, 2, Nav::Down, Axis::Vertical, 1, live), Some(4));
    assert_eq!(step_line(5, 4, Nav::Down, Axis::Vertical, 1, live), Some(1));
    assert_eq!(step_line(5, 1, Nav::Up, Axis::Vertical, 1, live), Some(4));
    assert_eq!(
        step_line(5, 2, Nav::Down, Axis::Vertical, 1, |_| false),
        None
    );
}

#[test]
fn home_end_and_paging_land_on_live_cells_and_stop_at_the_ends() {
    let live = |cell: usize| cell != 0 && cell != 9;
    assert_eq!(
        step_line(10, 5, Nav::Home, Axis::Vertical, 3, live),
        Some(1)
    );
    assert_eq!(step_line(10, 5, Nav::End, Axis::Vertical, 3, live), Some(8));
    assert_eq!(
        step_line(10, 5, Nav::PageDown, Axis::Vertical, 3, live),
        Some(8)
    );
    assert_eq!(
        step_line(10, 8, Nav::PageDown, Axis::Vertical, 3, live),
        Some(8)
    );
    assert_eq!(
        step_line(10, 2, Nav::PageUp, Axis::Vertical, 3, live),
        Some(1)
    );
    assert_eq!(
        step_line(10, 6, Nav::PageUp, Axis::Vertical, 3, live),
        Some(3)
    );
}

// A 3-wide grid of eight cells: rows [0 1 2] [3 4 5] [6 7].
const GRID: [usize; 3] = [3, 3, 2];

#[test]
fn grid_left_and_right_walk_reading_order_and_wrap() {
    assert_eq!(step_grid(&GRID, 2, Nav::Right, 1), Some(3));
    assert_eq!(step_grid(&GRID, 7, Nav::Right, 1), Some(0));
    assert_eq!(step_grid(&GRID, 0, Nav::Left, 1), Some(7));
    assert_eq!(step_grid(&GRID, 3, Nav::Left, 1), Some(2));
}

#[test]
fn grid_up_and_down_keep_the_column_and_wrap() {
    assert_eq!(step_grid(&GRID, 1, Nav::Down, 1), Some(4));
    assert_eq!(
        step_grid(&GRID, 5, Nav::Down, 1),
        Some(7),
        "nearest on a short row"
    );
    assert_eq!(step_grid(&GRID, 7, Nav::Down, 1), Some(1));
    assert_eq!(step_grid(&GRID, 2, Nav::Up, 1), Some(7));
    assert_eq!(step_grid(&GRID, 6, Nav::Up, 1), Some(3));
}

#[test]
fn grid_paging_stops_at_the_ends_and_home_end_jump() {
    assert_eq!(step_grid(&GRID, 1, Nav::PageDown, 2), Some(7));
    assert_eq!(step_grid(&GRID, 7, Nav::PageDown, 2), Some(7));
    assert_eq!(step_grid(&GRID, 5, Nav::PageUp, 5), Some(2));
    assert_eq!(step_grid(&GRID, 4, Nav::Home, 1), Some(0));
    assert_eq!(step_grid(&GRID, 4, Nav::End, 1), Some(7));
    assert_eq!(step_grid(&GRID, 4, Nav::Confirm, 1), None);
    assert_eq!(step_grid(&[], 0, Nav::Down, 1), None);
}

#[test]
fn grid_rows_of_sections_skip_empty_rows() {
    // A section heading contributes an empty row between card rows.
    let rows = [2, 0, 3];
    assert_eq!(step_grid(&rows, 1, Nav::Down, 1), Some(3));
    assert_eq!(step_grid(&rows, 4, Nav::Down, 1), Some(1));
}
