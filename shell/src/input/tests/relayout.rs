//! The HUD lays out again between events of one batch, so a press
//! hit-tests what the earlier events opened.

use super::*;
use crate::render::hud::{self, HudEnv};

/// Lays the HUD out headlessly in the 1280x800 test window.
fn relayout(game: &Game, input: &InputState) {
    hud::refresh(
        &game.view(),
        input,
        &classic(),
        HudEnv {
            viewport: vec2(1280.0, 800.0),
            ui: 1.0,
        },
        None,
        &|_, text: &str, size: f32| text.chars().count() as f32 * size * 0.5,
    );
}

/// A game holding one of the human's harvesters, laid out as a frame
/// would before input.
fn builder_in_hand() -> Game {
    let mut game = headless_game();
    let builder = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .expect("the human starts with a harvester")
        .id;
    game.presentation.selection.units.push(builder);
    relayout(&game, &InputState::new());
    game
}

#[test]
fn a_click_after_the_build_key_in_one_batch_arms_the_card_it_opened() {
    let (card, kind) = {
        let mut game = builder_in_hand();
        let mut input = InputState::new();
        let b = [key_down(Key::B), key_up(Key::B)];
        crate::input::apply_events(&mut game, &mut input, &classic(), &b, relayout);
        let layout = game.presentation.layout.get();
        match layout.cards[0] {
            (rect, crate::panel::CardAction::ArmBuild(kind)) => (rect.center(), kind),
            other => panic!("B opens the construction palette: {other:?}"),
        }
    };
    let batch = [
        key_down(Key::B),
        key_up(Key::B),
        left_down(card),
        left_up(card),
    ];

    let mut game = builder_in_hand();
    let mut input = InputState::new();
    crate::input::apply_events(&mut game, &mut input, &classic(), &batch, relayout);
    assert_eq!(input.placing, Some(kind));

    // Hit-testing the layout from before the batch misses the card.
    let mut game = builder_in_hand();
    let mut input = InputState::new();
    crate::input::apply_events(&mut game, &mut input, &classic(), &batch, |_, _| {});
    assert_ne!(input.placing, Some(kind));
}

#[test]
fn the_hud_lays_out_before_each_press_after_another_event_and_after_the_batch() {
    let p = vec2(640.0, 400.0);
    let count = |events: &[RawEvent]| {
        let mut game = headless_game();
        let mut calls = 0;
        crate::input::apply_events(
            &mut game,
            &mut InputState::new(),
            &classic(),
            events,
            |_, _| calls += 1,
        );
        calls
    };
    let shift = key_down(Key::Shift);
    assert_eq!(
        count(&[
            mouse_move(p),
            mouse_move(p),
            shift,
            left_down(p),
            left_up(p)
        ]),
        3
    );
    assert_eq!(count(&[]), 0);
    assert_eq!(count(&[left_down(p)]), 1);
}
