use super::*;
use oxide_sim::{BuildingId, PlayerId};

fn building_game() -> Game {
    let mut map = vec![".".repeat(64); 24];
    map[2].replace_range(2..3, "1");
    map[20].replace_range(58..59, "2");
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Double-click selection", "seed": 17,
            "players": [
                {"name": "You", "faction": "ferrous", "scrap": 1000, "bot": false},
                {"name": "Opponent", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "map": map,
            "units": [],
            "buildings": [
                {"player": 0, "kind": "turret", "x": 8, "y": 8},
                {"player": 0, "kind": "turret", "x": 14, "y": 8},
                {"player": 0, "kind": "turret", "x": 20, "y": 8},
                {"player": 0, "kind": "turret", "x": 40, "y": 8},
                {"player": 0, "kind": "fabricator", "x": 24, "y": 8},
                {"player": 1, "kind": "turret", "x": 10, "y": 12},
                {"player": 1, "kind": "turret", "x": 16, "y": 12},
                {"player": 1, "kind": "turret", "x": 54, "y": 12},
                {"player": 1, "kind": "scuttle_charge", "x": 14, "y": 5}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    game.presentation.camera.center = vec2(14.0, 8.0);
    game.presentation.camera.zoom = 64.0;
    game
}

fn building_at(game: &Game, x: i32, y: i32) -> BuildingId {
    game.state
        .buildings_at(TilePos::new(x, y))
        .next()
        .unwrap()
        .id
}

fn screen_at(game: &Game, x: f32, y: f32) -> Vec2 {
    game.presentation.camera.to_screen(vec2(x, y))
}

fn double_click(game: &mut Game, input: &mut InputState, screen: Vec2) {
    input.now = 10.0;
    apply_events(game, input, &click(screen.x, screen.y));
    input.now = 10.2;
    apply_events(game, input, &click(screen.x, screen.y));
}

#[test]
fn buildings_double_click_matches_kind_owner_and_viewport() {
    let mut game = building_game();
    let expected = [8, 14, 20].map(|x| building_at(&game, x, 8));
    let screen = screen_at(&game, 8.5, 8.5);
    double_click(&mut game, &mut InputState::new(), screen);
    assert_eq!(game.presentation.selection.buildings, expected);
    assert!(game.presentation.selection.units.is_empty());
    assert!(game.pending.is_empty());
}

#[test]
fn buildings_double_click_uses_centers_at_viewport_edges() {
    for zoom in [32.0, 64.0, 96.0] {
        for (right_edge, included) in [(20.49, false), (20.5, true)] {
            let mut game = building_game();
            game.presentation.camera.zoom = zoom;
            game.presentation.camera.center.x = right_edge - 640.0 / zoom;
            let first = building_at(&game, 8, 8);
            let second = building_at(&game, 14, 8);
            let edge = building_at(&game, 20, 8);
            let screen = screen_at(&game, 8.5, 8.5);
            double_click(&mut game, &mut InputState::new(), screen);
            let expected = if included {
                vec![first, second, edge]
            } else {
                vec![first, second]
            };
            assert_eq!(
                game.presentation.selection.buildings, expected,
                "zoom {zoom}, edge {right_edge}"
            );
        }
    }
}

#[test]
fn buildings_double_click_preserves_foreign_visibility() {
    let mut game = building_game();
    game.presentation.camera.zoom = 16.0;
    game.presentation.camera.center = vec2(32.0, 12.0);
    let expected = [10, 16].map(|x| building_at(&game, x, 12));
    for id in expected {
        assert!(
            game.state
                .building(id)
                .unwrap()
                .tiles()
                .any(|t| game.my_vision().visible(t))
        );
    }
    let hidden = game.state.building(building_at(&game, 54, 12)).unwrap();
    assert!(!hidden.tiles().any(|t| game.my_vision().visible(t)));
    let screen = screen_at(&game, 10.5, 12.5);
    double_click(&mut game, &mut InputState::new(), screen);
    assert_eq!(game.presentation.selection.buildings, expected);
    apply_events(&mut game, &mut InputState::new(), &[right_down(screen)]);
    assert!(
        game.pending.is_empty(),
        "foreign buildings remain read-only"
    );

    let screen = screen_at(&game, 54.5, 12.5);
    double_click(&mut game, &mut InputState::new(), screen);
    assert!(game.presentation.selection.buildings.is_empty());
}

#[test]
fn buildings_double_click_cannot_pick_concealed_mines_on_visible_ground() {
    let mut game = building_game();
    let mine = game.state.building(building_at(&game, 14, 5)).unwrap();
    assert!(mine.tiles().any(|t| game.my_vision().visible(t)));
    assert!(!game.state.building_apparent(PlayerId(0), mine));
    let screen = screen_at(&game, 14.5, 5.5);
    double_click(&mut game, &mut InputState::new(), screen);
    assert!(game.presentation.selection.buildings.is_empty());
}

#[test]
fn buildings_double_tap_uses_the_same_sweep() {
    let mut game = building_game();
    let expected = [8, 14, 20].map(|x| building_at(&game, x, 8));
    let screen = screen_at(&game, 8.5, 8.5);
    let mut input = InputState::new();
    for now in [10.0, 10.2] {
        input.now = now;
        apply_events(
            &mut game,
            &mut input,
            &[touch_down(1, screen), touch_up(1, screen)],
        );
    }
    assert_eq!(game.presentation.selection.buildings, expected);
}

#[test]
fn buildings_double_click_keeps_units_ahead_of_buildings() {
    let game = building_game();
    let mut scenario = game.scenario.clone();
    for x in [7, 11] {
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x,
            y: 8,
        });
    }
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    game.presentation.camera.center = vec2(14.0, 8.0);
    let screen = screen_at(&game, 8.05, 8.5);
    double_click(&mut game, &mut InputState::new(), screen);
    assert_eq!(game.presentation.selection.units.len(), 2);
    assert!(game.presentation.selection.buildings.is_empty());
}

#[test]
fn buildings_double_click_groups_across_upgrade_tiers() {
    let mut game = building_game();
    let upgraded = building_at(&game, 8, 8);
    game.state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::UpgradeBuilding { building: upgraded },
    }]);
    assert_eq!(game.state.building(upgraded).unwrap().tier, 1);
    let expected = [8, 14, 20].map(|x| building_at(&game, x, 8));
    assert_eq!(game.state.building(expected[1]).unwrap().tier, 0);
    let screen = screen_at(&game, 8.5, 8.5);
    double_click(&mut game, &mut InputState::new(), screen);
    assert_eq!(game.presentation.selection.buildings, expected);
}
