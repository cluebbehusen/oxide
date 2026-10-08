use super::*;
#[test]
fn resource_markers_age_with_world_memories_and_refresh_on_sight() {
    use chassis::grid::TilePos;
    let scenario = serde_json::from_str(include_str!("../../../scenarios/skirmish.json")).unwrap();
    let game = crate::game::Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let hidden = TilePos::new(30, 18);
    assert!(!game.my_vision().visible(hidden));
    for (age, expected) in [(0.0, 0.4), (45.0, 0.29), (90.0, 0.18), (900.0, 0.18)] {
        game.presentation
            .last_seen
            .borrow_mut()
            .insert((hidden.x, hidden.y), game.presentation.fx_time() - age);
        assert!((resource_marker_opacity(&game.view(), hidden) - expected).abs() < 0.0001);
        assert!(
            (resource_marker_opacity(&game.view(), hidden)
                - 0.4 * crate::render::resource_memory_opacity(&game.view(), hidden))
            .abs()
                < 0.0001
        );
    }
    let visible = TilePos::new(8, 4);
    assert!(game.my_vision().visible(visible));
    game.presentation
        .last_seen
        .borrow_mut()
        .insert((visible.x, visible.y), game.presentation.fx_time() - 90.0);
    assert_eq!(resource_marker_opacity(&game.view(), visible), 1.0);
    assert_eq!(
        game.presentation.last_seen.borrow()[&(visible.x, visible.y)],
        game.presentation.fx_time()
    );
}

#[test]
fn extractor_frame_markers_share_exploration_and_claim_visibility() {
    use chassis::grid::TilePos;
    let scenario: oxide_sim::scenario::Scenario =
        serde_json::from_str(include_str!("../../../scenarios/skirmish.json")).unwrap();
    let mut game = crate::game::Game::with_viewport(scenario.clone(), vec2(1280.0, 800.0)).unwrap();
    let near = TilePos::new(8, 4);
    let far = TilePos::new(30, 18);
    assert!(game.state.map().is_extractor_frame(near));
    assert!(game.state.map().is_extractor_frame(far));
    assert!(extractor_frame_visible(&game.view(), near));
    assert!(!extractor_frame_visible(&game.view(), far));
    game.presentation.overlay = true;
    assert!(extractor_frame_visible(&game.view(), far));

    let mut claimed = scenario;
    for (player, anchor) in [(0, near), (1, far)] {
        claimed.buildings.push(oxide_sim::scenario::BuildingSpec {
            player,
            kind: oxide_sim::BuildingKind::Extractor,
            x: anchor.x,
            y: anchor.y,
        });
    }
    let mut game = crate::game::Game::with_viewport(claimed, vec2(1280.0, 800.0)).unwrap();
    assert!(!extractor_frame_visible(&game.view(), near));
    assert!(!extractor_frame_visible(&game.view(), far));
    game.presentation.overlay = true;
    assert!(!extractor_frame_visible(&game.view(), near));
    assert!(!extractor_frame_visible(&game.view(), far));
}

#[test]
fn marker_scaling_preserves_the_screen_position_and_top_left_origin() {
    use macroquad::camera::Camera;
    let viewport = vec2(1280.0, 800.0);
    for scale in [0.75, 1.0, 1.5] {
        let camera = marker_camera(viewport, scale);
        let top_left = camera.matrix().transform_point3(vec3(0.0, 0.0, 0.0));
        assert!((top_left.x + 1.0).abs() < 0.0001);
        assert!((top_left.y - 1.0).abs() < 0.0001);
        let p = vec2(330.0, 140.0);
        let projected = camera.matrix().transform_point3((p / scale).extend(0.0));
        assert!((projected.x - (p.x / 640.0 - 1.0)).abs() < 0.0001);
        assert!((projected.y - (1.0 - p.y / 400.0)).abs() < 0.0001);
    }
}
#[test]
fn enemy_mine_marker_stays_hidden_on_visible_ground() {
    let mut scenario: oxide_sim::scenario::Scenario =
        serde_json::from_str(include_str!("../../../scenarios/skirmish.json")).unwrap();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 1,
        kind: oxide_sim::BuildingKind::ScuttleCharge,
        x: 10,
        y: 4,
    });
    let game = crate::game::Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let mine = game
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::BuildingKind::ScuttleCharge)
        .unwrap();
    assert!(game.my_vision().visible(mine.anchor));
    assert!(!building_visible(&game.view(), mine));
}
#[test]
fn earlier_transition_reaches_full_opacity_before_the_old_transition_starts() {
    assert_eq!(transition(24.0, 24.0, 16.0), 0.0);
    assert_eq!(transition(20.0, 24.0, 16.0), 0.5);
    assert_eq!(transition(16.0, 24.0, 16.0), 1.0);
    assert_eq!(transition(16.0, 14.0, 10.0), 0.0);
}
#[test]
fn markers_do_not_reveal_unseen_buildings_or_resources() {
    let scenario: oxide_sim::scenario::Scenario =
        serde_json::from_str(include_str!("../../../scenarios/skirmish.json")).unwrap();
    let game = crate::game::Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    for b in game.state.buildings() {
        assert_eq!(
            building_visible(&game.view(), b),
            b.player == game.presentation.human
        );
    }
    let mut hidden_nodes = 0;
    for y in 0..game.state.map().height() {
        for x in 0..game.state.map().width() {
            let p = chassis::grid::TilePos::new(x, y);
            if !game.my_vision().explored(p) && game.state.map().tile(p).unwrap().scrap > 0 {
                assert_eq!(known_resource(&game.view(), p), 0);
                hidden_nodes += 1;
            }
        }
    }
    assert!(hidden_nodes > 0);
}
#[test]
fn marker_transition_has_a_continuous_bounded_fade() {
    assert_eq!(marker_alpha(32.0), 0.0);
    assert_eq!(marker_alpha(20.0), 0.5);
    assert_eq!(marker_alpha(8.0), 1.0);
}

#[test]
fn strategic_markers_obey_player_sight_and_explicit_debug_visibility() {
    let source = include_str!("../../../scenarios/skirmish.json");
    let scenario: oxide_sim::scenario::Scenario = serde_json::from_str(source).unwrap();
    let mut game = crate::game::Game::with_viewport(scenario.clone(), vec2(1280.0, 800.0)).unwrap();
    assert!(visible(&game.view(), &game.state.units()[0]));
    assert!(!visible(&game.view(), &game.state.units()[4]));
    game.presentation.overlay = true;
    assert!(visible(&game.view(), &game.state.units()[4]));
    let mut nearby = scenario;
    nearby.units[4].x = 10;
    nearby.units[4].y = 9;
    let game = crate::game::Game::with_viewport(nearby, vec2(1280.0, 800.0)).unwrap();
    assert!(visible(&game.view(), &game.state.units()[4]));
}
