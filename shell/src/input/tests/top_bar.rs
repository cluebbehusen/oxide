//! The control-group strip, by click and by tap.

use super::*;
use crate::layout::GroupSlot;
use macroquad::math::Rect;

/// Publishes a strip whose slot `i` does `slots[i]`, and returns the
/// slot rects.
fn publish_strip(game: &mut Game, slots: [Option<GroupSlot>; 5]) -> [Rect; 5] {
    let mut layout = top_bar_layout();
    let rects: [Rect; 5] =
        std::array::from_fn(|i| Rect::new(900.0 + i as f32 * 44.0, 3.0, 40.0, 34.0));
    layout.group_slots = std::array::from_fn(|i| slots[i].map(|slot| (rects[i], slot)));
    game.presentation.layout.set(layout);
    rects
}

fn own_units(game: &Game) -> Vec<oxide_sim::UnitId> {
    game.state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .map(|u| u.id)
        .collect()
}

#[test]
fn a_saved_group_recalls_by_click_and_a_quick_second_click_centers_it() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (fighter, _) = own_fighter(&game);
    input.groups[0] = vec![fighter];
    let rects = publish_strip(
        &mut game,
        [Some(GroupSlot::Recall(1)), None, None, None, None],
    );
    let far = vec2(2.0, 2.0);
    game.presentation.camera.center = far;
    input.now = 10.0;
    let at = rects[0].center();
    apply_events(&mut game, &mut input, &click(at.x, at.y));
    assert_eq!(game.presentation.selection.units, vec![fighter]);
    assert_eq!(
        game.presentation.camera.center, far,
        "one recall leaves the camera"
    );
    input.now = 10.2;
    apply_events(&mut game, &mut input, &click(at.x, at.y));
    assert_ne!(
        game.presentation.camera.center, far,
        "a quick second recall centers"
    );
    assert!(game.pending.is_empty());
}

#[test]
fn ctrl_click_saves_the_selection_to_any_slot_and_plus_saves_it_plainly() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let own = own_units(&game);
    game.presentation.selection.units = vec![own[0]];
    let rects = publish_strip(
        &mut game,
        [
            None,
            Some(GroupSlot::Assign(2)),
            None,
            Some(GroupSlot::Empty(4)),
            Some(GroupSlot::Empty(5)),
        ],
    );
    let at = rects[4].center();
    apply_events(&mut game, &mut input, &click(at.x, at.y));
    assert!(
        input.groups[4].is_empty(),
        "a plain click on an empty place does nothing"
    );

    apply_events(&mut game, &mut input, &[key_down(Key::Ctrl)]);
    let at = rects[3].center();
    apply_events(&mut game, &mut input, &click(at.x, at.y));
    apply_events(&mut game, &mut input, &[key_up(Key::Ctrl)]);
    assert_eq!(
        input.groups[3],
        vec![own[0]],
        "Ctrl-click saves to that slot"
    );

    let at = rects[1].center();
    apply_events(&mut game, &mut input, &click(at.x, at.y));
    assert_eq!(input.groups[1], vec![own[0]], "the plus saves it too");
}

#[test]
fn the_strip_counts_live_own_units_and_offers_only_a_new_selection() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let own = own_units(&game);
    let foreign = game
        .state
        .units()
        .iter()
        .find(|u| u.player != game.presentation.human)
        .expect("an enemy unit")
        .id;
    input.groups[0] = vec![own[0], foreign];
    assert_eq!(input.group_counts(&game.view()), [1, 0, 0, 0, 0]);

    assert_eq!(input.group_on_offer(&game.view()), None, "nothing selected");
    game.presentation.selection.units = vec![own[0]];
    assert_eq!(input.selected_group(&game.view()), Some(1));
    assert_eq!(
        input.group_on_offer(&game.view()),
        None,
        "the selection already is group 1"
    );
    game.presentation.selection.units = vec![own[1]];
    assert_eq!(input.selected_group(&game.view()), None);
    assert_eq!(
        input.group_on_offer(&game.view()),
        Some(2),
        "lowest empty group"
    );
}

#[test]
fn a_tap_on_a_slot_recalls_like_a_click() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (fighter, _) = own_fighter(&game);
    input.groups[2] = vec![fighter];
    let rects = publish_strip(
        &mut game,
        [None, None, Some(GroupSlot::Recall(3)), None, None],
    );
    // A fingertip just under the slot still lands on its pad.
    tap(
        &mut game,
        &mut input,
        vec2(rects[2].center().x, rects[2].y + rects[2].h + 3.0),
    );
    assert_eq!(game.presentation.selection.units, vec![fighter]);
}

#[test]
fn a_long_press_on_a_slot_saves_the_selection_there_without_recalling() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let own = own_units(&game);
    input.groups[0] = vec![own[0]];
    game.presentation.selection.units = vec![own[1]];
    let rects = publish_strip(
        &mut game,
        [Some(GroupSlot::Recall(1)), None, None, None, None],
    );
    let at = rects[0].center();
    input.now = 5.0;
    apply_events(&mut game, &mut input, &[touch_down(1, at)]);
    input.now = 5.25;
    let (_, progress) = group_press_progress(&input).expect("the slot charges");
    assert!(progress > 0.0 && progress < 1.0);
    assert_eq!(
        long_press_progress(&input),
        None,
        "no world ring over chrome"
    );
    input.now = 6.0;
    update_touch(&mut game, &mut input);
    assert_eq!(input.groups[0], vec![own[1]], "the selection took the slot");
    apply_events(&mut game, &mut input, &[touch_up(1, at)]);
    assert_eq!(
        input.last_recall, None,
        "the lift after the hold recalls nothing"
    );
    assert_eq!(game.presentation.selection.units, vec![own[1]]);

    // With nothing selected, the hold clears the group.
    game.presentation.selection.units.clear();
    input.now = 10.0;
    apply_events(&mut game, &mut input, &[touch_down(2, at)]);
    input.now = 11.0;
    update_touch(&mut game, &mut input);
    apply_events(&mut game, &mut input, &[touch_up(2, at)]);
    assert!(input.groups[0].is_empty());
}
