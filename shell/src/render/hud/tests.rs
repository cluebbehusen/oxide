use super::*;
use crate::action::BindingMap;
use crate::game::Game;

/// Every glyph half as wide as the size: deterministic and headless.
fn fixed(_: Face, text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * size * 0.5
}

fn env(w: f32, h: f32) -> HudEnv {
    HudEnv {
        viewport: vec2(w, h),
        ui: 1.0,
    }
}

fn skirmish() -> Game {
    Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap()
}

#[test]
fn a_selected_foundry_lays_its_cards_inside_the_action_region() {
    let mut game = skirmish();
    let human = game.presentation.human;
    let foundry = game
        .state
        .buildings()
        .iter()
        .find(|building| building.player == human)
        .unwrap()
        .id;
    game.presentation.selection.buildings.push(foundry);
    let hud = layout(
        &game.view(),
        &InputState::new(),
        &BindingMap::classic(),
        env(1280.0, 800.0),
        None,
        &fixed,
    );
    let panel = hud.panel.as_ref().expect("a selection has a panel");
    let model = hud.layout;
    assert!(model.card_count > 0);
    assert_eq!(model.card_count, panel.cards.len().min(16));
    let actions = model.panel_regions[1];
    for (rect, _) in &model.cards[..model.card_count] {
        assert!(
            rect.x >= actions.x
                && rect.y >= actions.y
                && rect.right() <= actions.right()
                && rect.bottom() <= actions.bottom(),
            "{rect:?} outside {actions:?}"
        );
    }
    assert_eq!(
        model.minimap,
        minimap_rect_scaled(
            game.state.map().width(),
            game.state.map().height(),
            vec2(1280.0, 800.0),
            1.0
        )
    );
    assert!(model.menu_button.w > 0.0 && model.pause_status.w > 0.0);
}

#[test]
fn a_spectator_has_no_top_bar_controls_but_keeps_the_minimap() {
    let mut game = skirmish();
    game.presentation.spectate = true;
    let hud = layout(
        &game.view(),
        &InputState::new(),
        &BindingMap::classic(),
        env(1280.0, 800.0),
        None,
        &fixed,
    );
    assert_eq!(hud.layout.menu_button.w, 0.0);
    assert_eq!(hud.layout.pause_status.w, 0.0);
    assert!(hud.geometry.top_bar.is_none());
    assert!(hud.layout.minimap.w > 0.0);
}

#[test]
fn a_resize_moves_the_minimap_before_the_next_click() {
    let game = skirmish();
    let input = InputState::new();
    let bindings = BindingMap::classic();
    let view = game.view();
    refresh(&view, &input, &bindings, env(1280.0, 800.0), None, &fixed);
    let small = game.presentation.layout.get().minimap;
    refresh(&view, &input, &bindings, env(1600.0, 1000.0), None, &fixed);
    let large = game.presentation.layout.get().minimap;
    assert_ne!(small, large);
    let center = large.center();
    assert!(
        !small.contains(center),
        "the new minimap's center must be off the old one"
    );
    assert!(crate::render::minimap_world_at(&view, center).is_some());
}
