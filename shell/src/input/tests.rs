//! Integration tests for the input funnel: real events through the real
//! resolver against a real (headless) sim.

use super::*;
use crate::numeric;
use crate::numeric::Fit;
use oxide_sim::scenario::ScenarioMode;
use oxide_sim::{PlayerCommand, UnitKind};

/// The shipped default map: input tests never read a developer's saved
/// bindings.
fn classic() -> BindingMap {
    BindingMap::classic()
}

/// The production funnel under the default map.
fn apply_events(game: &mut Game, input: &mut InputState, events: &[RawEvent]) {
    apply_events_with(game, input, &classic(), events);
}

fn apply_events_with(
    game: &mut Game,
    input: &mut InputState,
    bindings: &BindingMap,
    events: &[RawEvent],
) {
    super::apply_events(game, input, bindings, events);
}

fn update_touch(game: &mut Game, input: &mut InputState) {
    super::update_touch(game, input, &classic());
}

fn dispatch_action(game: &mut Game, input: &mut InputState, action: Action) {
    super::dispatch::dispatch_action(game, input, &classic(), action);
}

fn activate_card(game: &mut Game, input: &mut InputState, action: crate::panel::CardAction) {
    super::activate_card(game, input, &classic(), action);
}

mod double_click;

/// The waypoints `unit` draws in `game`'s current frame.
fn crumbs(game: &Game, unit: &oxide_sim::Unit) -> Vec<(usize, Vec2, macroquad::color::Color)> {
    let view = game.view();
    crate::render::entities::breadcrumb_points(&view, &view.projection(), unit)
}

fn headless_game() -> Game {
    Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0))
        .expect("embedded skirmish builds")
}

#[test]
fn only_the_host_pauses_a_lan_match() {
    use crate::game::network::NetRole;
    let mut duel = oxide_sim::Scenario::skirmish();
    for player in &mut duel.players {
        player.bot = false;
        player.bot_config = None;
    }
    let lan = |seat, role| {
        Game::networked(
            duel.clone(),
            oxide_sim::PlayerId(seat),
            role,
            vec2(1280.0, 800.0),
        )
        .unwrap()
    };
    let (mut host, mut client) = (lan(0, NetRole::Host), lan(1, NetRole::Client));
    let mut input = InputState::new();
    dispatch_action(&mut host, &mut input, Action::TogglePause);
    dispatch_action(&mut client, &mut input, Action::TogglePause);
    assert!(host.clock.paused);
    assert!(!client.clock.paused);
    assert!(
        client
            .presentation
            .toasts
            .iter()
            .any(|toast| toast.text == "Only the host can pause")
    );
}

fn multi_producer_game() -> Game {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("multi-producer fixture builds")
}

fn empty_multi_producer_game() -> Game {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.units.clear();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("empty building fixture builds")
}

fn contested_producer_game() -> Game {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.units.clear();
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Scuttler,
        x: 11,
        y: 3,
    });
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("contested fixture builds")
}

/// A 1280x800 layout with a panel band and no other chrome; each test
/// places the one rect it exercises.
fn bare_layout(panel_top: f32, panel_right: f32) -> crate::layout::LayoutModel {
    let zero = macroquad::math::Rect::new(0.0, 0.0, 0.0, 0.0);
    let none = (zero, crate::panel::CardAction::None);
    crate::layout::LayoutModel::compute(
        vec2(1280.0, 800.0),
        1.0,
        panel_top,
        panel_right,
        zero,
        zero,
        zero,
        zero,
        zero,
        zero,
        [none; 8],
        0,
        [none; 16],
        0,
        [none; 8],
        0,
    )
}

fn left_down(p: Vec2) -> RawEvent {
    RawEvent::MouseDown {
        button: MouseButton::Left,
        x: p.x,
        y: p.y,
    }
}

fn left_up(p: Vec2) -> RawEvent {
    RawEvent::MouseUp {
        button: MouseButton::Left,
        x: p.x,
        y: p.y,
    }
}

fn right_down(p: Vec2) -> RawEvent {
    RawEvent::MouseDown {
        button: MouseButton::Right,
        x: p.x,
        y: p.y,
    }
}

fn mouse_move(p: Vec2) -> RawEvent {
    RawEvent::MouseMove { x: p.x, y: p.y }
}

fn touch_down(id: u64, p: Vec2) -> RawEvent {
    RawEvent::TouchDown { id, x: p.x, y: p.y }
}

fn touch_move(id: u64, p: Vec2) -> RawEvent {
    RawEvent::TouchMove { id, x: p.x, y: p.y }
}

fn touch_up(id: u64, p: Vec2) -> RawEvent {
    RawEvent::TouchUp { id, x: p.x, y: p.y }
}

fn key_down(key: Key) -> RawEvent {
    RawEvent::KeyDown { key }
}

fn key_up(key: Key) -> RawEvent {
    RawEvent::KeyUp { key }
}

fn click(x: f32, y: f32) -> [RawEvent; 2] {
    [left_down(vec2(x, y)), left_up(vec2(x, y))]
}

#[test]
fn performance_panel_swallows_orders_and_selection_without_revealing_fog() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let unit = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player == game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.units.push(unit);
    let mut layout = game.presentation.layout.get();
    layout.performance = macroquad::prelude::Rect::new(1028.0, 46.0, 240.0, 158.0);
    game.presentation.layout.set(layout);
    let pos = layout.performance.center();
    let before = game.state.hash();
    for events in [
        click(pos.x, pos.y).to_vec(),
        vec![right_down(pos)],
        vec![touch_down(23, pos), touch_up(23, pos)],
    ] {
        apply_events(&mut game, &mut input, &events);
        assert_eq!(game.presentation.selection.units, vec![unit]);
        assert!(game.pending.is_empty());
        assert_eq!(game.state.hash(), before);
        assert!(!game.presentation.all_seeing());
    }
}

fn skyhook_interaction_game() -> Game {
    let scenario = oxide_sim::Scenario::from_json(
        "{
        \"name\": \"skyhook interaction\",
        \"players\": [
            {\"name\": \"F\", \"faction\": \"ferrous\", \"scrap\": 100, \"bot\": false},
            {\"name\": \"C\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true}
        ],
        \"map\": [
            \"####################\",
            \"#..................#\",
            \"#.1..............2.#\",
            \"#..................#\",
            \"#..................#\",
            \"#..................#\",
            \"#..................#\",
            \"#..................#\",
            \"####################\"
        ],
        \"units\": [
            {\"player\": 0, \"kind\": \"sentinel\", \"x\": 6, \"y\": 5},
            {\"player\": 0, \"kind\": \"skyhook\", \"x\": 9, \"y\": 5},
            {\"player\": 1, \"kind\": \"skyhook\", \"x\": 12, \"y\": 5}
        ]
    }",
    )
    .expect("inline Skyhook interaction scenario parses");
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("Skyhook interaction builds")
}

#[test]
fn skyhook_visible_edge_selects_the_transport() {
    let mut game = skyhook_interaction_game();
    let mut input = InputState::new();
    let skyhook = game
        .state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook)
        .expect("fixture Skyhook");
    let id = skyhook.id;
    let center = vec2(skyhook.pos.x.to_num::<f32>(), skyhook.pos.y.to_num::<f32>());
    game.presentation.camera.center = center;
    game.presentation.camera.pan(Vec2::ZERO);
    let edge = game.presentation.camera.to_screen(center + vec2(0.9, 0.0));

    apply_events(&mut game, &mut input, &click(edge.x, edge.y));

    assert_eq!(game.presentation.selection.units, vec![id]);
}

#[test]
fn skyhook_visible_edge_accepts_a_load_order() {
    let mut game = skyhook_interaction_game();
    let mut input = InputState::new();
    let sentinel = game
        .state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Sentinel)
        .expect("fixture Sentinel")
        .id;
    let skyhook = game
        .state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook)
        .expect("fixture Skyhook");
    let transport = skyhook.id;
    let center = vec2(skyhook.pos.x.to_num::<f32>(), skyhook.pos.y.to_num::<f32>());
    game.presentation.camera.center = center;
    game.presentation.camera.pan(Vec2::ZERO);
    game.presentation.selection.units = vec![sentinel];
    let edge = game.presentation.camera.to_screen(center + vec2(0.9, 0.0));

    apply_events(&mut game, &mut input, &[right_down(edge)]);

    assert!(game.pending.iter().any(|command| matches!(
        command.command,
        Command::Load { transport: target, .. } if target == transport
    )));
}

#[test]
fn hostile_skyhook_visible_edge_accepts_an_attack_order() {
    let mut game = skyhook_interaction_game();
    let mut input = InputState::new();
    let sentinel = game
        .state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Sentinel)
        .expect("fixture Sentinel")
        .id;
    let skyhook = game
        .state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook && unit.player != game.presentation.human)
        .expect("fixture hostile Skyhook");
    let target = skyhook.id;
    let center = vec2(skyhook.pos.x.to_num::<f32>(), skyhook.pos.y.to_num::<f32>());
    game.presentation.camera.center = center;
    game.presentation.camera.pan(Vec2::ZERO);
    game.presentation.selection.units = vec![sentinel];
    let edge = game.presentation.camera.to_screen(center + vec2(0.9, 0.0));

    apply_events(&mut game, &mut input, &[right_down(edge)]);

    assert!(game.pending.iter().any(|command| matches!(
        command.command,
        Command::Attack {
            target: oxide_sim::AttackTarget::Unit(unit),
            ..
        } if unit == target
    )));
}

#[test]
fn shift_click_selects_and_toggles_same_owner_buildings() {
    let mut game = multi_producer_game();
    let mut input = InputState::new();
    let mut own: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.presentation.human)
        .map(|building| building.id)
        .collect();
    own.sort_unstable();
    assert_eq!(own.len(), 2);
    let center = |game: &Game, id| {
        let building = game.state.building(id).unwrap();
        let size = building.kind.size();
        game.presentation.camera.to_screen(vec2(
            building.anchor.x as f32 + size.0 as f32 * 0.5,
            building.anchor.y as f32 + size.1 as f32 * 0.5,
        ))
    };

    let first = center(&game, own[0]);
    apply_events(&mut game, &mut input, &click(first.x, first.y));
    assert_eq!(game.presentation.selection.buildings, vec![own[0]]);

    let second = center(&game, own[1]);
    apply_events(
        &mut game,
        &mut input,
        &[
            key_down(Key::Shift),
            left_down(second),
            left_up(second),
            key_up(Key::Shift),
        ],
    );
    assert_eq!(game.presentation.selection.buildings, own);

    apply_events(
        &mut game,
        &mut input,
        &[
            key_down(Key::Shift),
            left_down(first),
            left_up(first),
            key_up(Key::Shift),
        ],
    );
    assert_eq!(game.presentation.selection.buildings, vec![own[1]]);
}

#[test]
fn box_select_falls_back_to_same_owner_buildings_and_shift_adds() {
    let mut game = empty_multi_producer_game();
    let mut own: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.presentation.human)
        .map(|building| building.id)
        .collect();
    own.sort_unstable();
    assert_eq!(own.len(), 2);
    let center = |game: &Game, id| {
        let building = game.state.building(id).unwrap();
        let center = building.center();
        game.presentation
            .camera
            .to_screen(vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>()))
    };

    let first = center(&game, own[0]);
    box_select(
        &mut game,
        first - vec2(8.0, 8.0),
        first + vec2(8.0, 8.0),
        false,
    );
    assert_eq!(game.presentation.selection.buildings, vec![own[0]]);
    assert!(game.presentation.selection.units.is_empty());

    let second = center(&game, own[1]);
    box_select(
        &mut game,
        second - vec2(8.0, 8.0),
        second + vec2(8.0, 8.0),
        true,
    );
    assert_eq!(game.presentation.selection.buildings, own);
    assert!(game.presentation.selection.units.is_empty());
}

#[test]
fn box_select_keeps_own_units_ahead_of_buildings() {
    let mut game = multi_producer_game();
    let own_units: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|unit| unit.player == game.presentation.human)
        .map(|unit| unit.id)
        .collect();
    assert!(!own_units.is_empty());

    let top_left = game.presentation.camera.to_screen(vec2(-1.0, -1.0));
    let bottom_right = game.presentation.camera.to_screen(vec2(30.0, 20.0));
    box_select(&mut game, top_left, bottom_right, false);

    assert_eq!(game.presentation.selection.units, own_units);
    assert!(
        game.presentation.selection.buildings.is_empty(),
        "units retain marquee priority"
    );
}

#[test]
fn box_select_keeps_own_buildings_ahead_of_foreign_units() {
    let mut game = contested_producer_game();
    let fabricator = game
        .state
        .buildings()
        .iter()
        .find(|building| {
            building.player == game.presentation.human
                && building.kind == oxide_sim::BuildingKind::Fabricator
        })
        .expect("own Fabricator")
        .id;
    let enemy = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player != game.presentation.human)
        .expect("foreign unit");
    assert!(
        game.my_vision().visible(enemy.tile()),
        "the foreign inspection candidate is visible"
    );

    let top_left = game.presentation.camera.to_screen(vec2(8.5, 2.5));
    let bottom_right = game.presentation.camera.to_screen(vec2(12.0, 5.0));
    box_select(&mut game, top_left, bottom_right, false);

    assert_eq!(game.presentation.selection.buildings, vec![fabricator]);
    assert!(
        game.presentation.selection.units.is_empty(),
        "a visible raider cannot hijack an own-building marquee"
    );
}

#[test]
fn selected_producers_receive_the_same_context_rally_in_id_order() {
    let mut game = multi_producer_game();
    let mut producers: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.presentation.human)
        .map(|building| building.id)
        .collect();
    producers.sort_unstable();
    game.presentation.selection.buildings = producers.clone();
    let rally = TilePos::new(14, 9);
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(rally.x as f32 + 0.5, rally.y as f32 + 0.5));

    context_order(&mut game, screen, false);

    let staged: Vec<_> = game
        .pending
        .iter()
        .filter_map(|command| match &command.command {
            Command::SetRally {
                building,
                rally: Some(tile),
            } => Some((*building, *tile)),
            _ => None,
        })
        .collect();
    assert_eq!(
        staged,
        producers
            .into_iter()
            .map(|building| (building, rally))
            .collect::<Vec<_>>()
    );
}

#[test]
fn training_skips_a_selected_nonproducer_before_the_factory() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.insert(
        0,
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: oxide_sim::BuildingKind::Turret,
            x: 9,
            y: 3,
        },
    );
    let mut game =
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("production fixture builds");
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == oxide_sim::BuildingKind::Turret)
        .unwrap()
        .id;
    let foundry = game.home_foundry().unwrap().id;
    game.presentation.selection.buildings = vec![turret, foundry];

    super::orders::train(&mut game, 0);

    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::Train { building, .. },
            ..
        }] if *building == foundry
    ));
}

#[test]
fn training_uses_the_first_selected_factory_that_supports_the_slot() {
    let mut game = multi_producer_game();
    let foundry = game.home_foundry().unwrap().id;
    let fabricator = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == oxide_sim::BuildingKind::Fabricator)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![foundry, fabricator];

    // Slot 4 sits past the Foundry's four-card roster, so only the
    // Fabricator can serve it — a slot both producers serve would
    // legitimately land on the first selected producer instead.
    super::orders::train(&mut game, 4);

    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::Train { building, .. },
            ..
        }] if *building == fabricator
    ));
}

#[test]
fn a_selected_defense_right_clicks_a_visible_enemy_into_focus() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Turret,
        x: 9,
        y: 3,
    });
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 12,
        y: 4,
    });
    let mut game =
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("focus fixture builds");
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == oxide_sim::BuildingKind::Turret)
        .unwrap()
        .id;
    let enemy = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player != game.presentation.human && unit.tile() == TilePos::new(12, 4))
        .unwrap();
    assert!(game.my_vision().visible(enemy.tile()));
    let enemy_id = enemy.id;
    let screen = game.presentation.camera.to_screen(vec2(
        enemy.pos.x.to_num::<f32>(),
        enemy.pos.y.to_num::<f32>(),
    ));
    game.presentation.selection.buildings = vec![turret];

    context_order(&mut game, screen, false);

    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::FocusFire { buildings, target },
            ..
        }] if buildings == &vec![turret] && *target == oxide_sim::Target::Unit(enemy_id).into()
    ));
    assert!(
        game.pending
            .iter()
            .all(|command| !matches!(command.command, Command::SetRally { .. })),
        "a defense click must never become a nonsensical rally"
    );
}

fn build_click(
    game: &mut Game,
    input: &mut InputState,
    kind: oxide_sim::BuildingKind,
    anchor: TilePos,
) {
    input.placing = Some(kind);
    let world = vec2(anchor.x as f32 + 0.5, anchor.y as f32 + 0.5);
    game.presentation.camera.center = world;
    game.presentation.camera.pan(Vec2::ZERO);
    let point = game.presentation.camera.to_screen(world);
    apply_events(game, input, &click(point.x, point.y));
}

fn extractor_input_game() -> Game {
    let scenario = oxide_sim::Scenario::from_json(
        "{
        \"name\": \"frame input\",
        \"players\": [
            {\"name\": \"F\", \"faction\": \"ferrous\", \"scrap\": 500, \"bot\": false},
            {\"name\": \"C\", \"faction\": \"cupric\", \"scrap\": 500, \"bot\": true,
             \"bot_config\": {}}
        ],
        \"map\": [
            \"################\",
            \"#..............#\",
            \"#.1............#\",
            \"#..............#\",
            \"#......E.......#\",
            \"#..............#\",
            \"#............2.#\",
            \"#..............#\",
            \"################\"
        ],
        \"units\": [
            {\"player\": 0, \"kind\": \"harvester\", \"x\": 5, \"y\": 4}
        ]
    }",
    )
    .expect("inline Extractor scenario parses");
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("Extractor input fixture builds")
}

#[test]
fn every_tile_of_a_known_extractor_frame_places_the_same_site() {
    let frame = TilePos::new(7, 4);
    for dy in 0..2 {
        for dx in 0..2 {
            let mut game = extractor_input_game();
            let mut input = InputState::new();
            let worker = game
                .state
                .units()
                .iter()
                .find(|unit| unit.player == game.presentation.human)
                .expect("fixture worker")
                .id;
            game.presentation.selection.units = vec![worker];
            input.placing = Some(oxide_sim::BuildingKind::Extractor);
            let clicked = frame.offset(dx, dy);
            let world = vec2(clicked.x as f32 + 0.5, clicked.y as f32 + 0.5);
            let point = game.presentation.camera.to_screen(world);

            apply_events(&mut game, &mut input, &[left_down(point)]);

            assert!(matches!(
                game.pending.as_slice(),
                [PlayerCommand {
                    command: Command::Build {
                        kind: oxide_sim::BuildingKind::Extractor,
                        anchor,
                        ..
                    },
                    ..
                }] if *anchor == frame
            ));
            assert_eq!(
                input
                    .placing_stroke
                    .as_ref()
                    .expect("accepted click opens its placement stroke")
                    .anchors,
                vec![frame]
            );
            assert!(
                game.presentation.fx.iter().any(|effect| matches!(
                    effect.kind,
                    crate::game::EffectKind::Ping { at, kind: crate::game::PingKind::Rally, .. }
                        if (at - vec2(8.0, 5.0)).length_squared() < f32::EPSILON
                )),
                "the acknowledgment stays centered on the snapped frame"
            );
        }
    }
}

#[test]
fn bookmarks_remember_and_recall_camera_ground() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let saved = game.presentation.camera.center;
    let chord = |game: &mut Game, input: &mut InputState, ctrl: bool, key: Key| {
        let mut ev = Vec::new();
        if ctrl {
            ev.push(key_down(Key::Ctrl));
        }
        ev.push(key_down(key));
        ev.push(key_up(key));
        if ctrl {
            ev.push(key_up(Key::Ctrl));
        }
        apply_events(game, input, &ev);
    };
    chord(&mut game, &mut input, true, Key::F5);
    game.presentation.camera.center = saved + vec2(6.0, 4.0);
    chord(&mut game, &mut input, false, Key::F5);
    assert!(
        (game.presentation.camera.center - saved).length() < 1e-4,
        "recall returns to the remembered ground"
    );
    chord(&mut game, &mut input, false, Key::F6);
    assert!(
        (game.presentation.camera.center - saved).length() < 1e-4,
        "an empty slot recalls nothing"
    );
}

#[test]
fn the_cycle_key_walks_idle_harvesters_in_id_order() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let idle = idle_harvesters(&game.view());
    assert!(idle.len() >= 2, "premise: skirmish opens with idle workers");
    let press = |game: &mut Game, input: &mut InputState| {
        apply_events(game, input, &[key_down(Key::N), key_up(Key::N)]);
    };
    press(&mut game, &mut input);
    assert_eq!(game.presentation.selection.units, vec![idle[0]]);
    press(&mut game, &mut input);
    assert_eq!(
        game.presentation.selection.units,
        vec![idle[1]],
        "id order, forward"
    );
    for _ in 0..idle.len() - 1 {
        press(&mut game, &mut input);
    }
    assert_eq!(
        game.presentation.selection.units,
        vec![idle[0]],
        "and wraps"
    );
}

#[test]
fn a_misclick_keeps_placement_armed_and_a_shift_click_repeats() {
    let mut game = headless_game();
    let mut input = InputState::new();
    // Arm a turret with a harvester selected (the palette's path).
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![harvester];
    input.placing = Some(oxide_sim::BuildingKind::Turret);

    // Skirmish's own foundry footprint is illegal ground: the
    // misclick toasts and stays armed, staging nothing.
    let foundry = game.state.buildings()[0].anchor;
    let bad = game
        .presentation
        .camera
        .to_screen(vec2(foundry.x as f32 + 0.5, foundry.y as f32 + 0.5));
    apply_events(&mut game, &mut input, &[left_down(bad)]);
    assert!(input.placing.is_some(), "a misclick must not disarm");
    assert!(game.pending.is_empty(), "and must spend nothing");

    // Shift-click on open visible ground stages and stays armed.
    let open = game
        .presentation
        .camera
        .to_screen(vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5));
    apply_events(
        &mut game,
        &mut input,
        &[key_down(Key::Shift), left_down(open), left_up(open)],
    );
    assert_eq!(game.pending.len(), 1, "legal ground stages the site");
    assert!(input.placing.is_some(), "shift keeps the wall going up");

    // A plain click (press and release; the mode settles at the release,
    // where the placement drag ends) disarms after staging. Skirmish's
    // 150 scrap is spent after the shift stamp, and an unaffordable click
    // refuses and keeps the mode armed, so the disarm half runs in a
    // fresh, still-funded session.
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation.selection.units = vec![
        game.state
            .units()
            .iter()
            .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
            .unwrap()
            .id,
    ];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(
        &mut game,
        &mut input,
        &[
            left_down(vec2(open.x + 96.0, open.y)),
            left_up(vec2(open.x + 96.0, open.y)),
        ],
    );
    assert_eq!(game.pending.len(), 1, "the plain click stages its site");
    assert!(input.placing.is_none(), "a plain click finishes the job");
}

#[test]
fn a_click_on_a_unit_selects_it_headlessly() {
    // The whole event path (resolver, hit-testing, selection) runs with
    // no window.
    let mut game = headless_game();
    let mut input = InputState::new();
    let unit = game.state.units()[0].id;
    let pos = game.state.units()[0].pos;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(game.presentation.selection.units, vec![unit]);
}

#[test]
fn a_right_click_on_ground_stages_an_advance() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let pos = game.state.units()[0].pos;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    let mid = game.presentation.camera.to_screen(vec2(
        pos.x.to_num::<f32>() + 4.0,
        pos.y.to_num::<f32>() + 2.0,
    ));
    apply_events(&mut game, &mut input, &[right_down(mid)]);
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { .. })),
        "zero-chase advance staged: {:?}",
        game.pending
    );
}

/// The newest order acknowledgment, if any.
fn last_ping(game: &Game) -> Option<(Vec2, crate::game::PingKind)> {
    game.presentation
        .fx
        .iter()
        .rev()
        .find_map(|fx| match fx.kind {
            crate::game::EffectKind::Ping { at, kind, .. } => Some((at, kind)),
            _ => None,
        })
}

/// Asserts the newest acknowledgment is `kind`, centred on `tile`.
fn assert_pinged_at_centre(game: &Game, tile: TilePos, kind: crate::game::PingKind) {
    let (at, pinged) = last_ping(game).expect("the order was acknowledged");
    assert!(pinged == kind, "the acknowledgment speaks the order's verb");
    assert!(
        (at - vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5)).length_squared() < f32::EPSILON,
        "the ring sits on the ordered tile's centre, not the cursor: {at:?} vs {tile:?}"
    );
}

/// A world point well inside `tile` but nowhere near its centre.
fn off_centre(tile: TilePos) -> Vec2 {
    vec2(tile.x as f32 + 0.15, tile.y as f32 + 0.85)
}

#[test]
fn an_off_centre_ground_click_orders_its_tile_and_pings_its_centre() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (fighter, at) = own_fighter(&game);
    game.presentation.selection.units = vec![fighter];
    let tile = TilePos::new(numeric::to_i32(at.x) + 4, numeric::to_i32(at.y) + 2);
    let screen = game.presentation.camera.to_screen(off_centre(tile));

    apply_events(&mut game, &mut input, &[right_down(screen)]);

    assert!(
        matches!(
            game.pending.as_slice(),
            [PlayerCommand { command: Command::Advance { goal, .. }, .. }] if *goal == tile
        ),
        "the order names the clicked tile: {:?}",
        game.pending
    );
    assert_pinged_at_centre(&game, tile, crate::game::PingKind::Move);
}

#[test]
fn armed_ground_verbs_ping_at_the_tile_centre() {
    for attack in [false, true] {
        let mut game = headless_game();
        let mut input = InputState::new();
        let (fighter, at) = own_fighter(&game);
        game.presentation.selection.units = vec![fighter];
        if attack {
            input.click_verb = Some(ClickVerb::Hunt);
        } else {
            input.click_verb = Some(ClickVerb::Run);
        }
        let tile = TilePos::new(numeric::to_i32(at.x) + 3, numeric::to_i32(at.y) - 2);
        let screen = game.presentation.camera.to_screen(off_centre(tile));

        apply_events(&mut game, &mut input, &[left_down(screen)]);

        let goal = game.pending.iter().find_map(|c| match c.command {
            Command::Run { goal, .. } if !attack => Some(goal),
            Command::Hunt { goal, .. } if attack => Some(goal),
            _ => None,
        });
        assert_eq!(goal, Some(tile), "attack={attack}: {:?}", game.pending);
        let kind = if attack {
            crate::game::PingKind::Attack
        } else {
            crate::game::PingKind::Move
        };
        assert_pinged_at_centre(&game, tile, kind);
    }
}

#[test]
fn a_minimap_order_pings_at_the_tile_centre() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (fighter, _) = own_fighter(&game);
    game.presentation.selection.units = vec![fighter];
    let minimap = publish_minimap(&game);
    let screen = vec2(minimap.x + 97.0, minimap.y + 61.0);
    let world = crate::render::minimap_world_at(&game.view(), screen).expect("inside the minimap");
    let tile = numeric::tile_at(world);
    assert!(
        (world - vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5)).length() > 0.05,
        "premise: the minimap point is off the tile's centre"
    );

    apply_events(&mut game, &mut input, &[right_down(screen)]);

    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { goal, .. } if goal == tile)),
        "{:?}",
        game.pending
    );
    assert_pinged_at_centre(&game, tile, crate::game::PingKind::Move);
}

#[test]
fn patrol_waypoints_ping_at_the_tile_centre() {
    let (mut game, mut input, _) = armed_patrol();
    let tile = TilePos::new(10, 6);
    let screen = game.presentation.camera.to_screen(off_centre(tile));

    apply_events(&mut game, &mut input, &[left_down(screen)]);

    assert_eq!(input.patrol_route, Some(vec![tile]));
    assert_pinged_at_centre(&game, tile, crate::game::PingKind::Rally);
}

#[test]
fn context_and_armed_rallies_ping_at_the_tile_centre() {
    let rally = TilePos::new(14, 9);
    let mut game = multi_producer_game();
    let producers: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.presentation.human)
        .map(|building| building.id)
        .collect();
    game.presentation.selection.buildings = producers.clone();
    let screen = game.presentation.camera.to_screen(off_centre(rally));
    context_order(&mut game, screen, false);
    assert!(
        !game.pending.is_empty()
            && game.pending.iter().all(
                |c| matches!(c.command, Command::SetRally { rally: Some(tile), .. } if tile == rally)
            ),
        "{:?}",
        game.pending
    );
    assert_pinged_at_centre(&game, rally, crate::game::PingKind::Rally);

    let mut game = multi_producer_game();
    let mut input = InputState::new();
    input.rallying = producers;
    let screen = game.presentation.camera.to_screen(off_centre(rally));
    apply_events(&mut game, &mut input, &[left_down(screen)]);
    assert!(
        !game.pending.is_empty()
            && game.pending.iter().all(
                |c| matches!(c.command, Command::SetRally { rally: Some(tile), .. } if tile == rally)
            ),
        "{:?}",
        game.pending
    );
    assert_pinged_at_centre(&game, rally, crate::game::PingKind::Rally);
}

/// A scenario with a harvester beside a scrap node on the map's west
/// edge, so a click in the camera's edge slack lands just past it.
fn edge_scrap_game() -> Game {
    let scenario = oxide_sim::Scenario::from_json(
        "{
        \"name\": \"edge scrap\",
        \"players\": [
            {\"name\": \"F\", \"faction\": \"ferrous\", \"scrap\": 500, \"bot\": false},
            {\"name\": \"C\", \"faction\": \"cupric\", \"scrap\": 500, \"bot\": true,
             \"bot_config\": {}}
        ],
        \"map\": [
            \"................\",
            \"..1.............\",
            \"................\",
            \"................\",
            \"s...............\",
            \"................\",
            \"............2...\",
            \"................\",
            \"................\"
        ],
        \"units\": [
            {\"player\": 0, \"kind\": \"harvester\", \"x\": 3, \"y\": 4}
        ]
    }",
    )
    .expect("inline edge scenario parses");
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("edge input fixture builds")
}

#[test]
fn an_edge_slack_click_orders_the_edge_tile_and_never_binds_edge_scrap() {
    let node = TilePos::new(0, 4);
    for (world, harvest) in [(vec2(-1.3, 4.6), false), (off_centre(node), true)] {
        let mut game = edge_scrap_game();
        let mut input = InputState::new();
        assert!(
            game.my_vision().remembered_scrap(node) > 0,
            "premise: the edge node is known"
        );
        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.presentation.human)
            .expect("fixture harvester")
            .id;
        game.presentation.selection.units = vec![harvester];
        let screen = game.presentation.camera.to_screen(world);
        assert!(
            screen.x > 0.0 && screen.y > 0.0 && !click_on_hud(&game, screen),
            "premise: the point is on open screen: {screen:?}"
        );

        apply_events(&mut game, &mut input, &[right_down(screen)]);

        if harvest {
            assert!(
                matches!(
                    game.pending.as_slice(),
                    [PlayerCommand { command: Command::Harvest { node: at, .. }, .. }] if *at == node
                ),
                "{:?}",
                game.pending
            );
            assert_pinged_at_centre(&game, node, crate::game::PingKind::Harvest);
        } else {
            assert!(
                matches!(
                    game.pending.as_slice(),
                    [PlayerCommand { command: Command::Advance { goal, .. }, .. }] if *goal == node
                ),
                "a slack click walks to the edge tile instead of harvesting it: {:?}",
                game.pending
            );
            assert_pinged_at_centre(&game, node, crate::game::PingKind::Move);
        }
    }
}

#[test]
fn a_context_order_cancels_placement_and_every_deferred_build_ghost() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[0].scrap = 300;
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let mut input = InputState::new();
    let builder = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player == game.presentation.human && unit.kind == UnitKind::Harvester)
        .expect("a starting Harvester")
        .id;
    let start = game.state.unit(builder).unwrap().tile();
    let kind = oxide_sim::BuildingKind::Turret;
    let claims = [
        PlayerCommand {
            player: game.presentation.human,
            command: Command::Build {
                units: vec![builder],
                kind,
                anchor: start.offset(5, 0),
                queue: false,
                defer: true,
            },
        },
        PlayerCommand {
            player: game.presentation.human,
            command: Command::Build {
                units: vec![builder],
                kind,
                anchor: start.offset(6, 0),
                queue: true,
                defer: true,
            },
        },
    ];
    let setup = game.state.tick(&claims);
    assert!(
        !setup
            .events
            .iter()
            .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. })),
        "premise: both deferred claims are accepted: {:?}",
        setup.events
    );
    game.presentation.selection.units = vec![builder];
    input.placing = Some(kind);

    let goal = start.offset(0, 4);
    let point = game
        .presentation
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    apply_events(&mut game, &mut input, &[right_down(point)]);

    assert!(
        input.placing.is_none(),
        "the cursor ghost exits as soon as a new contextual order is given"
    );
    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::Advance { queue: false, .. },
            ..
        }]
    ));

    let commands = std::mem::take(&mut game.pending);
    let report = game.state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. }))
    );
    let builder = game.state.unit(builder).unwrap();
    assert!(matches!(builder.order, oxide_sim::Order::Run { .. }));
    assert!(
        builder.queue.is_empty(),
        "replacement clears queued claims too"
    );
    assert!(
        std::iter::once(&builder.order)
            .chain(builder.queue.iter())
            .all(|order| !matches!(order, oxide_sim::Order::Found { .. })),
        "no deferred footprint remains for the renderer to ghost"
    );
}

#[test]
fn the_rally_card_arms_a_touchable_world_target() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let foundry = game
        .state
        .buildings()
        .iter()
        .find(|building| building.player == game.presentation.human)
        .expect("human Foundry")
        .id;
    game.presentation.selection.buildings = vec![foundry];

    let card = macroquad::math::Rect::new(300.0, 700.0, 60.0, 60.0);
    let mut layout = bare_layout(680.0, 500.0);
    layout.cards[0] = (card, crate::panel::CardAction::ArmRally);
    layout.card_count = 1;
    game.presentation.layout.set(layout);

    input.now = 1.0;
    apply_events(
        &mut game,
        &mut input,
        &[
            touch_down(1, vec2(card.x + 20.0, card.y + 20.0)),
            touch_up(1, vec2(card.x + 20.0, card.y + 20.0)),
        ],
    );
    assert_eq!(input.rallying, vec![foundry]);

    let rally = chassis::grid::TilePos::new(14, 9);
    let point = game
        .presentation
        .camera
        .to_screen(vec2(rally.x as f32 + 0.5, rally.y as f32 + 0.5));
    input.now = 2.0;
    apply_events(
        &mut game,
        &mut input,
        &[touch_down(2, point), touch_up(2, point)],
    );

    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::SetRally {
                building,
                rally: Some(staged),
            },
            ..
        }] if *building == foundry && *staged == rally
    ));
    assert!(
        input.rallying.is_empty(),
        "one target consumes the armed card"
    );
}

#[test]
fn a_ribbon_tap_cancels_the_mode_and_keeps_the_selection() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind.stats().can_fight())
        .expect("a starting combat unit")
        .id;
    game.presentation.selection.units = vec![fighter];
    input.click_verb = Some(ClickVerb::Hunt);
    let ribbon = macroquad::math::Rect::new(220.0, 620.0, 280.0, 44.0);
    let mut layout = bare_layout(f32::INFINITY, 0.0);
    layout.mode_ribbon = ribbon;
    game.presentation.layout.set(layout);
    tap(&mut game, &mut input, ribbon.center());
    assert_eq!(input.armed_mode(), None);
    assert_eq!(game.presentation.selection.units, vec![fighter]);
    assert!(game.pending.is_empty(), "cancel emits no gameplay command");

    input.click_verb = Some(ClickVerb::Run);
    apply_events(
        &mut game,
        &mut input,
        &click(ribbon.x + 20.0, ribbon.center().y),
    );
    assert_eq!(input.armed_mode(), None, "a click cancels too");
}

#[test]
fn a_lit_queue_turns_off_with_a_tap_so_the_next_ground_tap_clears() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (fighter, at) = own_fighter(&game);
    let chip = macroquad::math::Rect::new(160.0, 620.0, 96.0, 44.0);
    let mut layout = bare_layout(f32::INFINITY, 0.0);
    layout.queue_toggle = chip;
    game.presentation.layout.set(layout);
    game.presentation.selection.units = vec![fighter];
    input.queue_toggle = true;
    tap_world(&mut game, &mut input, at + vec2(4.0, 2.0));
    assert_eq!(
        game.presentation.selection.units,
        vec![fighter],
        "with QUEUE on a ground tap only adds"
    );

    input.now += 1.0;
    tap(&mut game, &mut input, chip.center());
    assert!(!input.queue_toggle, "one tap on the lit chip turns it off");
    tap_world(&mut game, &mut input, at + vec2(4.0, 2.0));
    assert!(game.presentation.selection.units.is_empty());

    // Resting on the chip never charges a battlefield order.
    game.presentation.selection.units = vec![fighter];
    input.now += 1.0;
    apply_events(&mut game, &mut input, &[touch_down(3, chip.center())]);
    input.now += 2.0;
    update_touch(&mut game, &mut input);
    assert!(
        game.pending.is_empty(),
        "a long-press on QUEUE orders nothing"
    );
}

#[test]
fn every_targeting_mode_has_persistent_human_copy() {
    let mut input = InputState::new();
    input.placing = Some(oxide_sim::BuildingKind::Bastion);
    assert_eq!(input.armed_mode().unwrap().label(), "Bastion");
    input.disarm_click_verbs();
    input.rallying = vec![oxide_sim::BuildingId(0)];
    assert_eq!(input.armed_mode().unwrap().label(), "Set rally");
    input.disarm_click_verbs();
    input.click_verb = Some(ClickVerb::Salvage);
    assert_eq!(input.armed_mode().unwrap().label(), "Salvage");
    input.disarm_click_verbs();
    input.click_verb = Some(ClickVerb::Weld);
    assert_eq!(input.armed_mode().unwrap().label(), "Weld");
    input.disarm_click_verbs();
    input.click_verb = Some(ClickVerb::Run);
    assert_eq!(input.armed_mode().unwrap().label(), "Run");
    input.disarm_click_verbs();
    input.click_verb = Some(ClickVerb::Hunt);
    assert_eq!(input.armed_mode().unwrap().label(), "Hunt");
    input.disarm_click_verbs();
    input.patrol_route = Some(vec![TilePos::new(1, 1), TilePos::new(2, 2)]);
    assert_eq!(input.armed_mode().unwrap().label(), "Patrol \u{b7} 2");
    assert!(input.cancel_armed_mode());
    assert_eq!(input.armed_mode(), None);
}

#[test]
fn double_click_timing_obeys_the_injected_clock() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let u = &game.state.units()[0];
    let (kind, pos) = (u.kind, u.pos);
    let same_kind_total = game
        .state
        .units()
        .iter()
        .filter(|o| o.kind == kind && o.player == game.presentation.human)
        .count();
    assert!(same_kind_total > 1, "premise: kin on screen to sweep up");
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    input.now = 10.0;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    // A slow second click is just a click...
    input.now = 11.0;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(
        game.presentation.selection.units.len(),
        1,
        "1.0s apart is two clicks"
    );
    // ...a fast one is a kind-sweep.
    input.now = 11.2;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert!(
        game.presentation.selection.units.len() > 1,
        "0.2s apart double-clicks into a kind sweep"
    );
}

#[test]
fn wheel_notches_and_trackpad_swipes_land_in_the_same_range() {
    // Windows notches (±120), X11 detents (±1), and a firm trackpad
    // swipe all read as whole steps; small fractional trackpad deltas
    // stay gentle.
    let generic = WheelUnits::Generic;
    assert_eq!(normalize_wheel(120.0, generic), 1.0);
    assert_eq!(normalize_wheel(-120.0, generic), -1.0);
    assert_eq!(normalize_wheel(1.0, generic), 1.0);
    assert_eq!(normalize_wheel(-1.0, generic), -1.0);
    assert_eq!(normalize_wheel(2.0, generic), 2.0);
    assert_eq!(normalize_wheel(10.0, generic), 1.0);
    assert!(normalize_wheel(0.4, generic) > 0.0 && normalize_wheel(0.4, generic) < 0.1);
}

#[test]
fn mac_trackpad_points_zoom_gently_and_wheel_lines_keep_their_notches() {
    let mac = WheelUnits::Mac;
    for points in [1.0, -1.0, 2.0, 3.0, 9.0] {
        assert!(
            normalize_wheel(points, mac).abs() < 1.0,
            "{points} trackpad points must stay under a full notch"
        );
    }
    assert!(normalize_wheel(1.0, mac) < normalize_wheel(2.0, mac));
    assert!(normalize_wheel(-2.0, mac) < 0.0);
    for lines in [10.0, -10.0, 20.0, 30.0, 39.9, 40.0, 120.0, 1200.0] {
        assert_eq!(
            normalize_wheel(lines, mac),
            normalize_wheel(lines, WheelUnits::Generic),
            "a macOS wheel reading of {lines} keeps its existing notch count"
        );
    }
}

#[test]
fn wheel_bursts_are_capped() {
    for units in [WheelUnits::Generic, WheelUnits::Mac] {
        assert_eq!(normalize_wheel(1200.0, units), 3.0);
        assert_eq!(normalize_wheel(-1200.0, units), -3.0);
        // The cap also catches fast trackpad flicks below the notch cutoff.
        assert_eq!(normalize_wheel(39.9, units), 3.0);
    }
}

#[test]
fn every_build_palette_entry_costs_scrap_to_raise() {
    // The palette is exactly what a harvester can place, so each entry
    // must carry construction stats with a real price. A `None` (a
    // Foundry-style scenario-only kind) or a zero cost would offer a
    // ghost the sim can never accept.
    for kind in crate::action::BUILD_CATEGORIES
        .iter()
        .flat_map(|(_, kinds)| kinds.iter())
    {
        let cost = kind
            .base_stats()
            .construction
            .unwrap_or_else(|| panic!("{} is in the palette but not constructable", kind.name()))
            .cost;
        assert!(cost > 0, "{} is free to build", kind.name());
    }
}

#[test]
fn the_build_palette_has_no_duplicate_structures() {
    // A repeated kind would burn a digit slot on a structure already
    // reachable by another digit.
    let kinds: Vec<_> = crate::action::BUILD_CATEGORIES
        .iter()
        .flat_map(|(_, kinds)| kinds.iter())
        .collect();
    for (i, a) in kinds.iter().enumerate() {
        for b in kinds.iter().skip(i + 1) {
            assert_ne!(a, b);
        }
    }
    assert!(
        crate::action::BUILD_CATEGORIES
            .iter()
            .all(|(_, kinds)| kinds.len() <= 4)
    );
}

#[test]
fn the_build_palette_offers_every_building_kind() {
    // A kind missing here could be built by a bot but never by a human.
    for kind in oxide_sim::BuildingKind::ALL {
        assert!(
            crate::action::BUILD_CATEGORIES
                .iter()
                .any(|(_, kinds)| kinds.contains(&kind)),
            "{kind:?} has no build palette slot"
        );
    }
}

#[test]
fn every_faction_roster_fits_the_production_hotkeys() {
    for producer in oxide_sim::BuildingKind::ALL {
        for faction in [oxide_sim::Faction::Ferrous, oxide_sim::Faction::Cupric] {
            let roster = producer
                .base_stats()
                .produces
                .iter()
                .filter(|kind| kind.faction().is_none_or(|f| f == faction))
                .count();
            assert!(
                roster <= usize::from(crate::action::TRAIN_SLOTS),
                "{producer:?} trains {roster} {faction:?} kinds"
            );
        }
    }
}

#[test]
fn key_map_binds_each_logical_key_at_most_once() {
    // Two rows sharing a logical Key would leave one keycode's binding
    // dead — whichever row `poll_events` reaches second is unreachable.
    for (i, a) in KEY_MAP.iter().enumerate() {
        for b in KEY_MAP.iter().skip(i + 1) {
            assert_ne!(a.0, b.0, "logical key bound twice: {:?}", a.0);
        }
    }
}

#[test]
fn each_physical_key_drives_at_most_one_logical_key() {
    // A repeated keycode silently shadows: `poll_events` emits the first
    // row's logical key and the second row never fires.
    for (i, a) in KEY_MAP.iter().enumerate() {
        for b in KEY_MAP.iter().skip(i + 1) {
            assert_ne!(a.1, b.1, "keycode bound twice: {:?}", a.1);
        }
    }
}

#[test]
fn a_right_click_anywhere_on_an_own_site_resumes_it() {
    // The resume verb addresses the SITE, not the cursor tile: clicking
    // the bottom-right tile of a 2x2 site must stage a Build at the
    // site's own anchor (the sim's resume arm matches anchor+kind).
    let mut game = headless_game();
    let mut input = InputState::new();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.presentation.human)
        .unwrap()
        .id;
    // Stand a Fabricator site on open visible ground near the base.
    let foundry = game.state.buildings()[0].anchor;
    let anchor = chassis::grid::TilePos::new(foundry.x + 3, foundry.y + 4);
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: game.presentation.human,
        command: oxide_sim::Command::Build {
            units: vec![harvester],
            kind: oxide_sim::stats::BuildingKind::Fabricator,
            anchor,
            queue: false,
            defer: false,
        },
    }]);
    assert!(
        game.state
            .buildings()
            .iter()
            .any(|b| b.anchor == anchor && !b.built()),
        "premise: the site stands"
    );
    // Select the harvester, then right-click the site's far corner.
    game.presentation.selection.units = vec![harvester];
    let corner = game
        .presentation
        .camera
        .to_screen(vec2(anchor.x as f32 + 1.5, anchor.y as f32 + 1.5));
    apply_events(&mut game, &mut input, &[right_down(corner)]);
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            oxide_sim::Command::Build { anchor: a, kind, .. }
                if *a == anchor && *kind == oxide_sim::stats::BuildingKind::Fabricator
        )),
        "the click resumed the site at its anchor: {:?}",
        game.pending
    );
}

#[test]
fn a_shift_click_on_the_wounded_wall_queues_the_weld_not_the_rat() {
    // Two claims at once: the queue flag rides the funnel into
    // Command::Repair, and an own-FOOTPRINT hit outranks the enemy
    // inside PICK_RADIUS of the same click.
    // A raw string can't hold this JSON (map rows open with `"#`,
    // which closes r#"..."# early), so the quotes are escaped.
    let scenario = oxide_sim::Scenario::from_json(
        "{
        \"name\": \"gnawed wall\",
        \"players\": [
            {\"name\": \"F\", \"faction\": \"ferrous\", \"scrap\": 100, \"bot\": false},
            {\"name\": \"C\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true}
        ],
        \"map\": [
            \"####################\",
            \"#..................#\",
            \"#..1...............#\",
            \"#..................#\",
            \"#..................#\",
            \"#..................#\",
            \"#..............2...#\",
            \"#..................#\",
            \"####################\"
        ],
        \"units\": [
            {\"player\": 0, \"kind\": \"harvester\", \"x\": 7, \"y\": 2},
            {\"player\": 1, \"kind\": \"scuttler\", \"x\": 5, \"y\": 3}
        ]
    }",
    )
    .expect("inline scenario parses");
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    let mut input = InputState::new();
    let foundry = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player == game.presentation.human)
        .unwrap()
        .id;
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human)
        .unwrap()
        .id;
    let rat = game
        .state
        .units()
        .iter()
        .find(|u| u.player != game.presentation.human)
        .unwrap()
        .id;
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: Command::Attack {
            units: vec![rat],
            target: oxide_sim::Target::Building(foundry).into(),
            queue: false,
        },
    }]);
    for _ in 0..120 {
        game.state.tick(&[]);
    }
    let wall = game.state.building(foundry).unwrap();
    assert!(wall.hp < wall.stats().max_hp, "premise: the rat left scars");
    // Click a footprint tile close enough to the rat that the enemy
    // pick would win if radius still outranked footprint.
    let rat_pos = {
        let u = game.state.unit(rat).unwrap();
        vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())
    };
    let center = vec2(
        wall.anchor.x as f32 + 1.0, // 2x2 footprint center
        wall.anchor.y as f32 + 1.0,
    );
    // The nearest wall point to the rat, nudged just inside.
    let clamped = vec2(
        rat_pos
            .x
            .clamp(wall.anchor.x as f32, wall.anchor.x as f32 + 2.0),
        rat_pos
            .y
            .clamp(wall.anchor.y as f32, wall.anchor.y as f32 + 2.0),
    );
    let world = clamped + (center - clamped).normalize() * 0.05;
    let tile = numeric::tile_at(world);
    assert!(
        wall.tiles().any(|t| t == tile),
        "premise: the click lands on the wall ({tile:?})"
    );
    assert!(
        world.distance(rat_pos) <= PICK_RADIUS,
        "premise: the rat is inside the pick radius"
    );
    game.presentation.selection.units = vec![harvester];
    let screen = game.presentation.camera.to_screen(world);
    apply_events(
        &mut game,
        &mut input,
        &[key_down(Key::Shift), right_down(screen)],
    );
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            Command::Repair { building, queue: true, .. } if *building == foundry
        )),
        "shift-right-click queued the weld: {:?}",
        game.pending
    );
    assert!(
        !game
            .pending
            .iter()
            .any(|c| matches!(&c.command, Command::Attack { .. })),
        "and the rat beside the wall did not steal the click"
    );
}

#[test]
fn the_armed_salvage_verb_strips_by_click_and_refuses_the_foundry() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Turret,
        x: 9,
        y: 5,
    });
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    let mut input = InputState::new();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.presentation.human)
        .unwrap()
        .id;
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::BuildingKind::Turret)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![harvester];
    // Arm with the hotkey, exactly as a player would.
    apply_events(&mut game, &mut input, &[key_down(Key::V), key_up(Key::V)]);
    assert!(input.armed(ClickVerb::Salvage), "V arms the wrecking crew");

    // A click on the Foundry refuses and stays armed.
    let foundry = game.state.buildings()[0].anchor;
    let on_foundry = game
        .presentation
        .camera
        .to_screen(vec2(foundry.x as f32 + 0.5, foundry.y as f32 + 0.5));
    apply_events(&mut game, &mut input, &[left_down(on_foundry)]);
    assert!(game.pending.is_empty(), "the victory token refuses");
    assert!(
        input.armed(ClickVerb::Salvage),
        "a misclick keeps the mode armed"
    );

    // A click on the turret stages the teardown and stands down.
    let on_turret = game.presentation.camera.to_screen(vec2(9.5, 5.5));
    apply_events(&mut game, &mut input, &[left_down(on_turret)]);
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            Command::Salvage { building, queue: false, .. } if *building == turret
        )),
        "the click sends the crew: {:?}",
        game.pending
    );
    assert!(
        !input.armed(ClickVerb::Salvage),
        "a plain click finishes the job"
    );
}

#[test]
fn the_armed_run_verb_issues_an_oblivious_move() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Sentinel)
        .expect("skirmish authors a sentinel")
        .id;
    game.presentation.selection.units = vec![fighter];
    // Arm with the classic hotkey, exactly as a player would.
    apply_events(&mut game, &mut input, &[key_down(Key::G), key_up(Key::G)]);
    assert!(input.armed(ClickVerb::Run), "G arms Run");

    // The click sends a Run order — the OBLIVIOUS walk, not the
    // explicit fighting march armed with F — and stands down.
    let home = game.state.unit(fighter).unwrap().tile();
    let goal = TilePos::new(home.x + 3, home.y);
    let p = game
        .presentation
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    apply_events(&mut game, &mut input, &[left_down(p), left_up(p)]);
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            Command::Run { goal: g, queue: false, .. } if *g == goal
        )),
        "the armed click issues Command::Run: {:?}",
        game.pending
    );
    assert!(
        !input.armed(ClickVerb::Run),
        "a plain click finishes the recall"
    );
    assert!(
        !game
            .pending
            .iter()
            .any(|c| matches!(&c.command, Command::Hunt { .. })),
        "nothing about the run engages"
    );
}

#[test]
fn arming_run_stands_the_other_verbs_down() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![harvester];
    // Placement armed, then G: exactly one verb may hold the cursor —
    // armed_click resolves placement before run, so both live at once
    // would stamp a building under a "run" toast.
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(&mut game, &mut input, &[key_down(Key::G), key_up(Key::G)]);
    assert!(input.armed(ClickVerb::Run), "G arms Run");
    assert!(input.placing.is_none(), "and placement stood down");
    let home = game.state.unit(harvester).unwrap().tile();
    let p = game
        .presentation
        .camera
        .to_screen(vec2(home.x as f32 + 2.5, home.y as f32 + 0.5));
    apply_events(&mut game, &mut input, &[left_down(p), left_up(p)]);
    assert!(
        game.pending
            .iter()
            .all(|c| !matches!(&c.command, Command::Build { .. })),
        "the click ran; it did not stamp the stale building: {:?}",
        game.pending
    );
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(&c.command, Command::Run { .. })),
        "the click issued the run"
    );
    // And the mirror direction: arming salvage stands run down.
    apply_events(
        &mut game,
        &mut input,
        &[
            key_down(Key::G),
            key_up(Key::G),
            key_down(Key::V),
            key_up(Key::V),
        ],
    );
    assert!(input.armed(ClickVerb::Salvage), "V arms salvage");
    assert!(!input.armed(ClickVerb::Run), "and the run stood down");
}

#[test]
fn f_arms_explicit_hunt_and_the_click_consumes_it() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind.stats().can_fight())
        .expect("a starting combat unit")
        .id;
    game.presentation.selection.units = vec![fighter];
    apply_events(&mut game, &mut input, &[key_down(Key::F), key_up(Key::F)]);
    assert!(input.armed(ClickVerb::Hunt), "F arms the fighting march");
    assert!(
        !input.armed(ClickVerb::Run),
        "hunt and run are mutually exclusive"
    );

    let goal = game.state.unit(fighter).unwrap().tile().offset(4, 1);
    let p = game
        .presentation
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    apply_events(&mut game, &mut input, &click(p.x, p.y));

    assert!(game.pending.iter().any(|command| matches!(
        command.command,
        Command::Hunt {
            goal: staged,
            queue: false,
            ..
        } if staged == goal
    )));
    assert!(
        !input.armed(ClickVerb::Hunt),
        "a plain click consumes the armed verb"
    );
}

#[test]
fn the_hunt_card_is_touchable_and_arms_the_same_world_tap() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind.stats().can_fight())
        .expect("a starting combat unit")
        .id;
    game.presentation.selection.units = vec![fighter];

    let card = macroquad::math::Rect::new(300.0, 700.0, 60.0, 60.0);
    let mut layout = bare_layout(680.0, 500.0);
    layout.cards[0] = (card, crate::panel::CardAction::Dispatch(Action::Hunt));
    layout.card_count = 1;
    game.presentation.layout.set(layout);

    input.now = 2.0;
    apply_events(
        &mut game,
        &mut input,
        &[touch_down(1, vec2(card.x + 20.0, card.y + 20.0))],
    );
    input.now = 2.1;
    apply_events(
        &mut game,
        &mut input,
        &[touch_up(1, vec2(card.x + 20.0, card.y + 20.0))],
    );
    assert!(
        input.armed(ClickVerb::Hunt),
        "the fingertip arms the panel verb"
    );

    let goal = game.state.unit(fighter).unwrap().tile().offset(4, 1);
    let point = game
        .presentation
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    input.now = 3.0;
    apply_events(&mut game, &mut input, &[touch_down(2, point)]);
    input.now = 3.1;
    apply_events(&mut game, &mut input, &[touch_up(2, point)]);

    assert!(game.pending.iter().any(|command| matches!(
        command.command,
        Command::Hunt {
            goal: staged,
            queue: false,
            ..
        } if staged == goal
    )));
    assert!(
        !input.armed(ClickVerb::Hunt),
        "the world tap consumes the armed verb"
    );
}

#[test]
fn a_paused_stroke_bills_each_kind_at_its_own_price() {
    // Bank 360: one staged turret (100) plus an armed bastion (250) is
    // affordable at the actual sum (350), not priced as two bastions
    // (500).
    let mut game = drag_arena(360);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    let p = game.presentation.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[key_down(Key::Shift), left_down(p), left_up(p)],
    );
    assert_eq!(staged_builds(&game), 1, "the turret staged");
    // The clock never ran (paused shell): the turret is still pending
    // when the palette switches kinds.
    input.placing = Some(oxide_sim::BuildingKind::Bastion);
    let p2 = game.presentation.camera.to_screen(vec2(9.5, 2.5));
    apply_events(&mut game, &mut input, &[left_down(p2), left_up(p2)]);
    assert_eq!(
        staged_builds(&game),
        2,
        "100 + 250 fits in 360 — the funded bastion must not be refused"
    );
}

#[test]
fn a_paused_stroke_refuses_ground_an_earlier_stroke_spoke_for() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    // Stroke A stamps a turret; the clock never runs, so the site
    // exists only in pending — live state still shows open ground.
    let p = game.presentation.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[key_down(Key::Shift), left_down(p), left_up(p)],
    );
    assert_eq!(staged_builds(&game), 1, "stroke A staged its site");
    // Stroke B opens on the same tile: the ground is spoken for, and
    // acknowledging the stamp would hand the sim a doomed command.
    apply_events(&mut game, &mut input, &[left_down(p), left_up(p)]);
    assert_eq!(
        staged_builds(&game),
        1,
        "the overlapping opening refused instead of double-booking the footprint"
    );
    assert!(
        input.placing.is_some(),
        "and the refusal keeps the mode armed"
    );
}

#[test]
fn queued_orders_count_against_the_stroke_prediction() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    // Three queued walks staged while paused: the builder's program
    // will hold them the moment the clock runs, so a build stroke
    // must see three fewer free slots even though live state still
    // reads an idle unit.
    for x in [14, 15, 16] {
        game.issue(Command::Run {
            units: vec![builder],
            goal: TilePos::new(x, 2),
            queue: true,
        });
    }
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    let mut tiles = Vec::new();
    for y in [2, 4, 6, 8] {
        for x in 4..=13 {
            if (x, y) != (7, 4) {
                tiles.push((x, y));
            }
        }
    }
    apply_events(&mut game, &mut input, &[key_down(Key::Shift)]);
    drag_over(&mut game, &mut input, &tiles);
    assert_eq!(
        staged_builds(&game),
        oxide_sim::stats::ORDER_QUEUE_CAP - 2,
        "three staged walks occupy three slots of the builder's program"
    );
}

#[test]
fn paused_strokes_share_one_queue_prediction() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    // Two Shift strokes with NO tick between them (paused shell): the
    // second must inherit the first's staged depth instead of
    // re-reading the untouched live queue and blowing past the cap.
    let mut tiles_a = Vec::new();
    let mut tiles_b = Vec::new();
    for y in [2, 4] {
        for x in 4..=13 {
            tiles_a.push((x, y));
        }
    }
    for y in [6, 8] {
        for x in 4..=13 {
            if (x, y) != (7, 8) {
                tiles_b.push((x, y));
            }
        }
    }
    let shift = [key_down(Key::Shift)];
    apply_events(&mut game, &mut input, &shift);
    drag_over(&mut game, &mut input, &tiles_a);
    apply_events(&mut game, &mut input, &shift);
    drag_over(&mut game, &mut input, &tiles_b);
    assert!(
        staged_builds(&game) <= oxide_sim::stats::ORDER_QUEUE_CAP + 1,
        "two paused strokes staged {} builds — more than the builder's program can hold",
        staged_builds(&game)
    );
    assert_eq!(
        staged_builds(&game),
        oxide_sim::stats::ORDER_QUEUE_CAP + 1,
        "and the cap itself is still reachable"
    );
}

#[test]
fn a_drag_rechecks_programs_staged_while_the_button_is_held() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];

    // Leave exactly one queue slot for the opening Shift stamp.
    let mut fill = vec![PlayerCommand {
        player: game.presentation.human,
        command: Command::Run {
            units: vec![builder],
            goal: TilePos::new(14, 7),
            queue: false,
        },
    }];
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP - 1 {
        fill.push(PlayerCommand {
            player: game.presentation.human,
            command: Command::Run {
                units: vec![builder],
                goal: TilePos::new(15, 7),
                queue: true,
            },
        });
    }
    game.state.tick(&fill);

    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(&mut game, &mut input, &[key_down(Key::Shift)]);
    let first = game.presentation.camera.to_screen(vec2(4.5, 2.5));
    apply_events(&mut game, &mut input, &[left_down(first)]);
    assert_eq!(staged_builds(&game), 1, "the last free slot was used");

    game.issue(Command::Stop {
        units: vec![builder],
    });
    let second = game.presentation.camera.to_screen(vec2(6.5, 2.5));
    apply_events(&mut game, &mut input, &[mouse_move(second)]);
    assert_eq!(
        staged_builds(&game),
        2,
        "a pending Stop frees the program for the next drag stamp"
    );

    // The inverse interleaving must also hold: externally staged orders
    // can consume all headroom before the next pointer event.
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    let first = game.presentation.camera.to_screen(vec2(4.5, 2.5));
    apply_events(&mut game, &mut input, &[left_down(first)]);
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP {
        game.issue(Command::Run {
            units: vec![builder],
            goal: TilePos::new(15, 7),
            queue: true,
        });
    }
    let second = game.presentation.camera.to_screen(vec2(6.5, 2.5));
    apply_events(&mut game, &mut input, &[mouse_move(second)]);
    assert_eq!(
        staged_builds(&game),
        1,
        "a projected full queue refuses a drag stamp the sim would reject"
    );
}

/// A 2v1 team scenario: the human and a configured bot ally on one
/// team, a lone enemy on the other — the readability tests' stage.
fn team_game() -> Game {
    let scenario = oxide_sim::Scenario::from_json(
        "{
        \"name\": \"team stage\",
        \"players\": [
            {\"name\": \"me\", \"faction\": \"ferrous\", \"scrap\": 100, \"bot\": false, \"team\": 1},
            {\"name\": \"pal\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true, \"team\": 1,
             \"bot_config\": {}},
            {\"name\": \"foe\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true,
             \"bot_config\": {}}
        ],
        \"map\": [
            \"########################\",
            \"#......................#\",
            \"#..1...................#\",
            \"#......................#\",
            \"#..2...................#\",
            \"#......................#\",
            \"#..................3...#\",
            \"#......................#\",
            \"########################\"
        ],
        \"units\": [
            {\"player\": 0, \"kind\": \"harvester\", \"x\": 7, \"y\": 2},
            {\"player\": 1, \"kind\": \"harvester\", \"x\": 7, \"y\": 4},
            {\"player\": 2, \"kind\": \"scuttler\", \"x\": 9, \"y\": 3}
        ]
    }",
    )
    .expect("team stage parses");
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds")
}

#[test]
fn an_ally_selection_reads_its_orders_but_takes_none() {
    let mut game = team_game();
    let mut input = InputState::new();
    let ally = game.state.units()[1].id;
    let pos = game.state.units()[1].pos;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(
        game.presentation.selection.units,
        vec![ally],
        "allies are selectable"
    );

    // The panel is read-only: no command cards; a single ally shows
    // static capability and its order chips.
    let panel = crate::panel::build_for_palette(&game.view(), &classic(), false).expect("a panel");
    assert!(panel.cards.is_empty(), "no verbs on an ally panel");
    assert!(
        panel
            .info
            .status
            .iter()
            .any(|status| status.contains("Standard / Balanced AI")),
        "the ally's controller stays visible"
    );
    assert!(
        !panel
            .info
            .rows
            .iter()
            .any(|row| matches!(row.label.as_str(), "Ground" | "Air")),
        "an unarmed ally needs no capability band"
    );
    assert!(
        panel
            .info
            .rows
            .iter()
            .any(|row| row.label == "Speed" && row.value == "2.5 tiles/s")
    );
    assert!(!panel.queue.is_empty(), "the ally's orders show");
    assert!(
        panel
            .queue
            .iter()
            .all(|chip| chip.action == crate::panel::CardAction::None),
        "an ally's chips remove nothing"
    );
    assert_eq!(
        panel.faction,
        oxide_sim::Faction::Cupric,
        "its colors, not mine"
    );

    // Every command path refuses: right-click stages nothing…
    apply_events(
        &mut game,
        &mut input,
        &[right_down(vec2(screen.x + 60.0, screen.y))],
    );
    assert!(game.pending.is_empty(), "ally units take no orders");
    // …and group assignment drops the foreign pick.
    apply_events(
        &mut game,
        &mut input,
        &[
            key_down(Key::Ctrl),
            key_down(Key::Num1),
            key_up(Key::Num1),
            key_up(Key::Ctrl),
        ],
    );
    assert!(input.groups[0].is_empty(), "no ally in a control group");
}

#[test]
fn a_hostile_selection_inspects_and_leaks_nothing() {
    let mut game = team_game();
    let mut input = InputState::new();
    let foe = game.state.units()[2].id;
    let pos = game.state.units()[2].pos;
    assert!(
        game.my_vision().visible(game.state.units()[2].tile()),
        "test premise: the raider stands in sight"
    );
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(
        game.presentation.selection.units,
        vec![foe],
        "a visible foe inspects"
    );

    // Static kind-level capability facts are safe to inspect. Command cards
    // and order chips stay absent because order state reveals intent.
    let panel = crate::panel::build_for_palette(&game.view(), &classic(), false).expect("a panel");
    assert!(panel.cards.is_empty(), "no verbs on a hostile panel");
    assert!(panel.queue.is_empty(), "no order chips on a hostile panel");
    assert!(
        panel
            .info
            .status
            .iter()
            .any(|status| status.contains("Standard / Balanced AI")),
        "the enemy's controller stays visible"
    );
    let weapon = panel
        .info
        .rows
        .iter()
        .find(|row| row.label == "Ground")
        .unwrap();
    assert_eq!(
        weapon.icon,
        Some(crate::panel::info::StatIcon::Capability(
            crate::panel::CapabilityIcon::Weapon
        ))
    );
    assert!(weapon.value.contains("dmg"));
    assert!(
        panel
            .info
            .rows
            .iter()
            .any(|row| row.label == "Range" && row.value.contains("tiles"))
    );

    // And no breadcrumbs, whatever program the enemy runs.
    let unit = game.state.unit(foe).unwrap();
    assert!(
        crumbs(&game, unit).is_empty(),
        "a foreign program draws no waypoints"
    );
}

#[test]
fn a_selection_never_mixes_allegiances() {
    let mut game = team_game();
    let mut input = InputState::new();
    let (mine, ally) = (game.state.units()[0].id, game.state.units()[1].id);
    let my_pos = game.state.units()[0].pos;
    let ally_pos = game.state.units()[1].pos;
    let my_screen = game
        .presentation
        .camera
        .to_screen(vec2(my_pos.x.to_num::<f32>(), my_pos.y.to_num::<f32>()));
    let ally_screen = game
        .presentation
        .camera
        .to_screen(vec2(ally_pos.x.to_num::<f32>(), ally_pos.y.to_num::<f32>()));
    // Own selected, shift-click the ally: REPLACE, never merge.
    apply_events(&mut game, &mut input, &click(my_screen.x, my_screen.y));
    assert_eq!(game.presentation.selection.units, vec![mine]);
    apply_events(
        &mut game,
        &mut input,
        &[
            key_down(Key::Shift),
            left_down(ally_screen),
            left_up(ally_screen),
            key_up(Key::Shift),
        ],
    );
    assert_eq!(
        game.presentation.selection.units,
        vec![ally],
        "a different owner replaces the selection"
    );
    // A box over both takes the OWN units only.
    let a = game.presentation.camera.to_screen(vec2(6.0, 1.5));
    let b = game.presentation.camera.to_screen(vec2(9.0, 5.0));
    apply_events(
        &mut game,
        &mut input,
        &[left_down(a), mouse_move(b), left_up(b)],
    );
    assert_eq!(
        game.presentation.selection.units,
        vec![mine],
        "a mixed box keeps only what the player can command"
    );
}

#[test]
fn touch_taps_select_and_a_still_hold_orders() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let unit = game.state.units()[0].id;
    let pos = game.state.units()[0].pos;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    // A short still touch is a tap: select.
    input.now = 5.0;
    apply_events(&mut game, &mut input, &[touch_down(1, screen)]);
    input.now = 5.1;
    apply_events(&mut game, &mut input, &[touch_up(1, screen)]);
    assert_eq!(
        game.presentation.selection.units,
        vec![unit],
        "a tap selects"
    );

    // A finger held still past the window fires the context order for
    // the live selection — a long-press is touch's right-click.
    let ground = game.presentation.camera.to_screen(vec2(
        pos.x.to_num::<f32>() + 4.0,
        pos.y.to_num::<f32>() + 2.0,
    ));
    input.now = 6.0;
    apply_events(&mut game, &mut input, &[touch_down(2, ground)]);
    input.now = 6.2;
    update_touch(&mut game, &mut input);
    assert!(game.pending.is_empty(), "0.2s is not a long-press yet");
    input.now = 6.5;
    update_touch(&mut game, &mut input);
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { .. })),
        "the held finger issued the ground order: {:?}",
        game.pending
    );
    let staged = game.pending.len();
    input.now = 7.0;
    update_touch(&mut game, &mut input);
    assert_eq!(game.pending.len(), staged, "a long-press fires once");
}

/// A game with one own harvester selected and `kind` armed for placement.
fn armed_placement(kind: oxide_sim::BuildingKind) -> (Game, InputState, oxide_sim::UnitId) {
    let mut game = headless_game();
    let mut input = InputState::new();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![harvester];
    input.placing = Some(kind);
    input.build_menu = true;
    (game, input, harvester)
}

fn staged_anchors(game: &Game) -> Vec<TilePos> {
    game.pending
        .iter()
        .filter_map(|c| match c.command {
            Command::Build { anchor, .. } => Some(anchor),
            _ => None,
        })
        .collect()
}

#[test]
fn a_touch_placement_drops_a_ghost_then_builds_where_it_is_drawn() {
    let kind = oxide_sim::BuildingKind::Turret;
    let (mut game, mut input, harvester) = armed_placement(kind);
    let foundry = game.state.buildings()[0].anchor;
    let open = vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5);
    tap_world(&mut game, &mut input, open);
    assert!(
        staged_anchors(&game).is_empty(),
        "the first tap only drops a ghost"
    );
    let ghost = input.ghost_anchor().expect("a ghost is down");
    let (w, h) = kind.size();
    let center = vec2(
        ghost.x as f32 + w as f32 * 0.5,
        ghost.y as f32 + h as f32 * 0.5,
    );
    assert!(
        center.distance(open) <= 0.75,
        "the ghost centers under the finger"
    );

    // A confirming tap anywhere on the ghost builds where it is drawn,
    // never where the fingertip happened to land.
    let corner = vec2(
        ghost.x as f32 + w as f32 - 0.1,
        ghost.y as f32 + h as f32 - 0.1,
    );
    tap_world(&mut game, &mut input, corner);
    assert_eq!(staged_anchors(&game), vec![ghost]);
    assert!(input.placing.is_none(), "a plain confirm disarms");
    assert!(input.ghost_anchor().is_none());
    assert_eq!(game.presentation.selection.units, vec![harvester]);
}

#[test]
fn dragging_the_ghost_moves_it_by_whole_tiles_without_panning() {
    let kind = oxide_sim::BuildingKind::Turret;
    let (mut game, mut input, _) = armed_placement(kind);
    let foundry = game.state.buildings()[0].anchor;
    tap_world(
        &mut game,
        &mut input,
        vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5),
    );
    let ghost = input.ghost_anchor().expect("a ghost is down");
    let grab = game
        .presentation
        .camera
        .to_screen(vec2(ghost.x as f32 + 0.5, ghost.y as f32 + 0.5));
    let camera = game.presentation.camera.center;
    let zoom = game.presentation.camera.zoom;
    input.now += 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, grab)]);
    for step in 1..=6 {
        let p = grab + vec2(step as f32 * 0.5 * zoom, zoom);
        apply_events(&mut game, &mut input, &[touch_move(1, p)]);
    }
    input.now += 2.0;
    update_touch(&mut game, &mut input);
    let dropped = grab + vec2(3.0 * zoom, zoom);
    apply_events(&mut game, &mut input, &[touch_up(1, dropped)]);
    assert_eq!(input.ghost_anchor(), Some(ghost.offset(3, 1)));
    assert_eq!(
        game.presentation.camera.center, camera,
        "a ghost drag never pans"
    );
    assert!(game.pending.is_empty(), "dropping the ghost builds nothing");
}

#[test]
fn a_refused_confirm_keeps_the_ghost_and_the_mode() {
    let (mut game, mut input, _) = armed_placement(oxide_sim::BuildingKind::Turret);
    let foundry = game.state.buildings()[0].center();
    let on_foundry = vec2(foundry.x.to_num::<f32>(), foundry.y.to_num::<f32>());
    tap_world(&mut game, &mut input, on_foundry);
    let ghost = input.ghost_anchor().expect("a ghost is down");
    tap_world(&mut game, &mut input, on_foundry);
    assert!(staged_anchors(&game).is_empty());
    assert_eq!(input.ghost_anchor(), Some(ghost));
    assert!(
        game.presentation
            .toasts
            .iter()
            .any(|t| t.text.starts_with("Can't build there")),
        "the refusal says why"
    );
}

#[test]
fn queue_keeps_placement_armed_after_a_confirm() {
    let (mut game, mut input, _) = armed_placement(oxide_sim::BuildingKind::Turret);
    input.queue_toggle = true;
    let foundry = game.state.buildings()[0].anchor;
    let open = vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5);
    tap_world(&mut game, &mut input, open);
    tap_world(&mut game, &mut input, open);
    assert_eq!(staged_anchors(&game).len(), 1);
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Build { queue: true, .. }))
    );
    assert_eq!(
        input.placing,
        Some(oxide_sim::BuildingKind::Turret),
        "still armed"
    );
    assert!(
        input.ghost_anchor().is_none(),
        "the next tap drops a fresh ghost"
    );
}

#[test]
fn leaving_placement_leaves_no_ghost() {
    let (mut game, mut input, _) = armed_placement(oxide_sim::BuildingKind::Turret);
    let foundry = game.state.buildings()[0].anchor;
    let open = vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5);
    tap_world(&mut game, &mut input, open);
    dispatch_action(&mut game, &mut input, Action::Back);
    assert!(
        input.placing.is_none() && input.touch_ghost.is_none(),
        "Back"
    );
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    tap_world(&mut game, &mut input, open);
    assert!(input.cancel_armed_mode());
    assert!(input.touch_ghost.is_none(), "CANCEL");
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    tap_world(&mut game, &mut input, open);
    input.placing = Some(oxide_sim::BuildingKind::Bastion);
    input.disarm_click_verbs();
    assert!(input.touch_ghost.is_none(), "switching kinds");
}

#[test]
fn a_long_press_while_placing_charges_nothing_and_orders_nothing() {
    let (mut game, mut input, _) = armed_placement(oxide_sim::BuildingKind::Turret);
    let foundry = game.state.buildings()[0].anchor;
    let ground = game
        .presentation
        .camera
        .to_screen(vec2(foundry.x as f32 + 4.5, foundry.y as f32 + 2.5));
    input.now = 3.0;
    apply_events(&mut game, &mut input, &[touch_down(1, ground)]);
    input.now = 3.25;
    assert_eq!(long_press_progress(&input), None, "no ring while placing");
    input.now = 4.0;
    update_touch(&mut game, &mut input);
    assert!(game.pending.is_empty(), "no context order");
    apply_events(&mut game, &mut input, &[touch_up(1, ground)]);
    assert!(
        input.ghost_anchor().is_some(),
        "the long rest still dropped a ghost"
    );
}

#[test]
fn an_extractor_ghost_snaps_to_its_frame() {
    let frame = TilePos::new(7, 4);
    let mut game = extractor_input_game();
    let mut input = InputState::new();
    let worker = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player == game.presentation.human)
        .expect("fixture worker")
        .id;
    game.presentation.selection.units = vec![worker];
    input.placing = Some(oxide_sim::BuildingKind::Extractor);
    tap_world(&mut game, &mut input, vec2(8.5, 5.5));
    assert_eq!(input.ghost_anchor(), Some(frame));
    tap_world(&mut game, &mut input, vec2(7.5, 4.5));
    assert_eq!(staged_anchors(&game), vec![frame]);
}

#[test]
fn a_touch_device_never_previews_placement_at_a_stale_mouse_point() {
    let (mut game, mut input, _) = armed_placement(oxide_sim::BuildingKind::Turret);
    input.mouse = vec2(400.0, 300.0);
    assert!(
        placement_preview_anchor(&game.view(), &input).is_some(),
        "the mouse previews"
    );
    let foundry = game.state.buildings()[0].anchor;
    let open = game
        .presentation
        .camera
        .to_screen(vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5));
    input.now += 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, open)]);
    assert!(
        placement_preview_anchor(&game.view(), &input).is_none(),
        "no ghost yet"
    );
    apply_events(&mut game, &mut input, &[touch_up(1, open)]);
    assert_eq!(
        placement_preview_anchor(&game.view(), &input).map(|(_, anchor)| anchor),
        input.ghost_anchor()
    );
}

#[test]
fn a_fogged_hostile_never_steers_the_long_press() {
    let mut game = headless_game();
    let mut input = InputState::new();
    // Own Foundry selected: a long-press on ground stages its rally.
    let own = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player == game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![own];
    // The enemy Foundry's ground is unexplored — but an omniscient
    // entity probe would still see the building there and flip the
    // gesture from rally to select, leaking hidden occupancy.
    let foe = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player != game.presentation.human)
        .unwrap();
    let center = vec2(foe.anchor.x as f32 + 1.0, foe.anchor.y as f32 + 1.0);
    let foe_tile = foe.anchor;
    assert!(
        !game.my_vision().visible(foe_tile),
        "the probe point must sit under fog for this test to bite"
    );
    game.presentation.camera.center = center;
    let screen = game.presentation.camera.to_screen(center);
    input.now = 9.0;
    apply_events(&mut game, &mut input, &[touch_down(7, screen)]);
    input.now = 9.9;
    update_touch(&mut game, &mut input);
    assert_eq!(
        game.presentation.selection.buildings,
        vec![own],
        "the hidden building must not turn the gesture into a select"
    );
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::SetRally { .. })),
        "fogged ground long-press means rally, occupied or not: {:?}",
        game.pending
    );
}

#[test]
fn one_finger_drags_the_camera_and_two_box_select() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let before = game.presentation.camera.center;
    // One moved finger pans the world under the hand.
    apply_events(&mut game, &mut input, &[touch_down(1, vec2(400.0, 300.0))]);
    apply_events(&mut game, &mut input, &[touch_move(1, vec2(340.0, 300.0))]);
    assert!(
        game.presentation.camera.center.x > before.x,
        "dragging left shows ground to the east"
    );
    apply_events(&mut game, &mut input, &[touch_up(1, vec2(340.0, 300.0))]);
    assert!(
        game.presentation.selection.units.is_empty(),
        "a drag is never a tap-select"
    );

    // A pair lifted before it rests into a box selects nothing: it was
    // a pinch that never got going.
    let a = game.presentation.camera.to_screen(vec2(2.0, 2.0));
    let b = game.presentation.camera.to_screen(vec2(12.0, 10.0));
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, a)]);
    apply_events(&mut game, &mut input, &[touch_down(2, b)]);
    apply_events(&mut game, &mut input, &[touch_up(2, b)]);
    apply_events(&mut game, &mut input, &[touch_up(1, a)]);
    assert!(
        game.presentation.selection.units.is_empty(),
        "no box without the rest"
    );

    // Two steady fingers that rest into a box select everything in it.
    input.now = 3.0;
    apply_events(&mut game, &mut input, &[touch_down(1, a)]);
    apply_events(&mut game, &mut input, &[touch_down(2, b)]);
    input.now = 3.0 + (touch::BOX_REST_MS + 10.0) / 1000.0;
    update_touch(&mut game, &mut input);
    apply_events(&mut game, &mut input, &[touch_up(2, b)]);
    assert!(
        !game.presentation.selection.units.is_empty(),
        "the finger-box swept the base"
    );
}

#[test]
fn a_resting_world_finger_charges_the_long_press_ring() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let ground = vec2(400.0, 300.0);
    input.now = 5.0;
    apply_events(&mut game, &mut input, &[touch_down(1, ground)]);
    assert_eq!(
        long_press_progress(&input),
        None,
        "a fresh touch may be a tap"
    );
    input.now = 5.0 + (TOUCH_REST_MS - 10.0) / 1000.0;
    assert_eq!(long_press_progress(&input), None, "quick taps never flash");

    let charge = f64::from(input.touch_prefs.long_press_ms) - TOUCH_REST_MS;
    input.now = 5.0 + (TOUCH_REST_MS + charge * 0.5) / 1000.0;
    let (at, half) = long_press_progress(&input).expect("a resting finger charges");
    assert_eq!(at, ground);
    assert!(
        (half - 0.5).abs() < 0.01,
        "halfway through the hold: {half}"
    );

    input.now = 5.0 + f64::from(input.touch_prefs.long_press_ms) / 1000.0 + 0.01;
    update_touch(&mut game, &mut input);
    assert_eq!(long_press_progress(&input), None, "a fired press is spent");
    apply_events(&mut game, &mut input, &[touch_up(1, ground)]);

    // A finger that pans, a second finger, or chrome ground never charges.
    input.now = 10.0;
    apply_events(&mut game, &mut input, &[touch_down(2, ground)]);
    apply_events(
        &mut game,
        &mut input,
        &[touch_move(2, ground + vec2(80.0, 0.0))],
    );
    input.now = 10.3;
    assert_eq!(long_press_progress(&input), None, "a pan is not a hold");
    apply_events(
        &mut game,
        &mut input,
        &[touch_up(2, ground + vec2(80.0, 0.0))],
    );

    input.now = 20.0;
    apply_events(
        &mut game,
        &mut input,
        &[
            touch_down(3, ground),
            touch_down(4, ground + vec2(200.0, 0.0)),
        ],
    );
    input.now = 20.3;
    assert_eq!(long_press_progress(&input), None, "a pair is not a hold");
    apply_events(
        &mut game,
        &mut input,
        &[touch_up(3, ground), touch_up(4, ground + vec2(200.0, 0.0))],
    );

    game.presentation.layout.set(top_bar_layout());
    let menu = game.presentation.layout.get().menu_button.center();
    input.now = 30.0;
    apply_events(&mut game, &mut input, &[touch_down(5, menu)]);
    input.now = 30.3;
    assert_eq!(long_press_progress(&input), None, "chrome owns its ground");
}

/// Hunt and Run side by side in the command band, with a fighter
/// selected so either card arms its verb.
fn two_card_band() -> (Game, macroquad::math::Rect, macroquad::math::Rect) {
    let mut game = headless_game();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind.stats().can_fight())
        .expect("a starting combat unit")
        .id;
    game.presentation.selection.units = vec![fighter];
    let attack = macroquad::math::Rect::new(300.0, 700.0, 60.0, 60.0);
    let run = macroquad::math::Rect::new(362.0, 700.0, 60.0, 60.0);
    let mut layout = bare_layout(680.0, 500.0);
    layout.cards[0] = (attack, crate::panel::CardAction::Dispatch(Action::Hunt));
    layout.cards[1] = (run, crate::panel::CardAction::Dispatch(Action::Run));
    layout.card_count = 2;
    game.presentation.layout.set(layout);
    (game, attack, run)
}

#[test]
fn a_resting_finger_previews_a_card_and_lifting_in_place_activates_it() {
    let (mut game, attack, _) = two_card_band();
    let mut input = InputState::new();
    let at = attack.center();
    input.now = 2.0;
    apply_events(&mut game, &mut input, &[touch_down(1, at)]);
    assert_eq!(input.touch_preview(), None, "a fresh touch may be a tap");
    input.now = 2.0 + (TOUCH_REST_MS + 10.0) / 1000.0;
    assert_eq!(input.touch_preview(), Some(at), "a resting finger previews");

    // Reading past the long-press window neither orders nor spends the tap.
    input.now = 4.0;
    update_touch(&mut game, &mut input);
    assert!(game.pending.is_empty(), "a held card orders nothing");
    assert_eq!(
        input.touch_preview(),
        Some(at),
        "the preview outlasts the long-press"
    );
    apply_events(&mut game, &mut input, &[touch_up(1, at)]);
    assert!(
        input.armed(ClickVerb::Hunt),
        "lifting in place activates the card"
    );

    // World ground never previews.
    let ground = vec2(400.0, 300.0);
    input.now = 10.0;
    apply_events(&mut game, &mut input, &[touch_down(2, ground)]);
    input.now = 10.2;
    assert_eq!(input.touch_preview(), None, "the battlefield has no cards");
}

#[test]
fn a_finger_that_leaves_its_card_activates_nothing() {
    let (mut game, attack, run) = two_card_band();
    let mut input = InputState::new();

    // Sliding off past the slop cancels, and the preview ends with it.
    input.now = 2.0;
    apply_events(&mut game, &mut input, &[touch_down(1, attack.center())]);
    input.now = 2.5;
    let away = attack.center() - vec2(0.0, 120.0);
    apply_events(&mut game, &mut input, &[touch_move(1, away)]);
    assert_eq!(input.touch_preview(), None);
    apply_events(&mut game, &mut input, &[touch_up(1, away)]);

    // Landing on one card and lifting on its neighbor, inside the slop.
    let edge = vec2(attack.right() - 4.0, attack.center().y);
    let over = vec2(run.x + 4.0, run.center().y);
    assert!(edge.distance(over) < 2.0 * 12.0, "premise: still a tap");
    input.now = 5.0;
    apply_events(&mut game, &mut input, &[touch_down(2, edge)]);
    input.now = 5.05;
    apply_events(
        &mut game,
        &mut input,
        &[touch_move(2, over), touch_up(2, over)],
    );

    assert!(
        !input.armed(ClickVerb::Hunt) && !input.armed(ClickVerb::Run),
        "neither card arms"
    );
    assert!(game.pending.is_empty());
}

#[test]
fn a_disabled_card_explains_itself_to_a_tap_or_a_click() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let foundry = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player == game.presentation.human)
        .expect("human Foundry")
        .id;
    game.presentation.selection.buildings = vec![foundry];
    let panel =
        crate::panel::build_for_input(&game.view(), &classic(), &input).expect("a Foundry panel");
    let (index, why) = panel
        .cards
        .iter()
        .enumerate()
        .find_map(|(i, card)| (!card.enabled).then(|| card.why.clone().map(|why| (i, why))))
        .flatten()
        .expect("premise: an opening Foundry has a locked card with a reason");
    *game.presentation.panel_model.borrow_mut() = Some(panel);
    let rect = macroquad::math::Rect::new(300.0, 700.0, 60.0, 60.0);
    let mut layout = bare_layout(680.0, 500.0);
    layout.cards[index] = (rect, crate::panel::CardAction::Refused);
    layout.card_count = index + 1;
    game.presentation.layout.set(layout);
    let why = crate::typography::sentence_case(&why);
    let toasted = |game: &Game| game.presentation.toasts.iter().any(|t| t.text == why);

    tap(&mut game, &mut input, rect.center());
    assert!(toasted(&game), "the tap names the reason");
    assert!(game.pending.is_empty(), "a refusal stages nothing");

    game.presentation.toasts.clear();
    apply_events(
        &mut game,
        &mut input,
        &click(rect.center().x, rect.center().y),
    );
    assert!(toasted(&game), "the click names the reason");
    assert!(game.pending.is_empty());
    assert_eq!(input.drag_origin, None, "the click never reaches the world");
}

#[test]
fn a_card_that_changes_under_a_resting_finger_activates_nothing() {
    // A disabled card that enables while the finger rests on it.
    let (mut game, attack, _) = two_card_band();
    let mut input = InputState::new();
    let mut layout = game.presentation.layout.get();
    layout.cards[0] = (attack, crate::panel::CardAction::Refused);
    game.presentation.layout.set(layout);
    input.now = 2.0;
    apply_events(&mut game, &mut input, &[touch_down(1, attack.center())]);
    layout.cards[0] = (attack, crate::panel::CardAction::Dispatch(Action::Hunt));
    game.presentation.layout.set(layout);
    input.now = 4.0;
    apply_events(&mut game, &mut input, &[touch_up(1, attack.center())]);
    assert!(
        !input.armed(ClickVerb::Hunt),
        "the lift never arms what the press never saw"
    );

    // A production queue that shifts under a pressed chip: slot 0 keeps
    // its action but now holds a different unit.
    let foundry = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player == game.presentation.human)
        .expect("own Foundry")
        .id;
    game.presentation.selection.units.clear();
    game.presentation.selection.buildings = vec![foundry];
    let chip = |kind: UnitKind, index: u8| crate::panel::Card {
        icon: crate::panel::CardIcon::Unit(kind),
        title: format!("{kind:?}"),
        cost: None,
        hotkey: String::new(),
        action: crate::panel::CardAction::CancelQueue(foundry, index),
        enabled: true,
        why: None,
        desc: Vec::new(),
        progress: None,
    };
    let slot = macroquad::math::Rect::new(20.0, 600.0, 48.0, 48.0);
    let mut layout = bare_layout(680.0, 500.0);
    layout.queue_slots[0] = (slot, crate::panel::CardAction::CancelQueue(foundry, 0));
    layout.queue_count = 1;
    game.presentation.layout.set(layout);
    for shifts in [false, true] {
        let mut panel = crate::panel::build_for_input(&game.view(), &classic(), &input)
            .expect("a Foundry panel");
        panel.queue = vec![chip(UnitKind::Harvester, 0), chip(UnitKind::Sentinel, 1)];
        *game.presentation.panel_model.borrow_mut() = Some(panel);
        game.pending.clear();
        input.now += 1.0;
        apply_events(&mut game, &mut input, &[touch_down(2, slot.center())]);
        if shifts {
            let mut model = game.presentation.panel_model.borrow_mut();
            let queue = &mut model.as_mut().expect("the published panel").queue;
            queue.remove(0);
            queue[0].action = crate::panel::CardAction::CancelQueue(foundry, 0);
        }
        input.now += 0.1;
        apply_events(&mut game, &mut input, &[touch_up(2, slot.center())]);
        let cancelled = game
            .pending
            .iter()
            .any(|c| matches!(c.command, Command::CancelTrain { index: 0, .. }));
        assert_eq!(cancelled, !shifts, "shifted: {shifts}");
    }
}

#[test]
fn a_re_reported_landing_is_the_same_finger() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let start = vec2(400.0, 300.0);
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, start)]);
    apply_events(
        &mut game,
        &mut input,
        &[touch_move(1, start - vec2(60.0, 0.0))],
    );
    let panned = game.presentation.camera.center;
    // iOS repeats the landing of a finger already down; the pan must
    // carry on without a fresh slop circle.
    apply_events(
        &mut game,
        &mut input,
        &[touch_down(1, start - vec2(60.0, 0.0))],
    );
    apply_events(
        &mut game,
        &mut input,
        &[touch_move(1, start - vec2(65.0, 0.0))],
    );
    assert_ne!(game.presentation.camera.center, panned, "the pan continues");
    apply_events(
        &mut game,
        &mut input,
        &[touch_up(1, start - vec2(65.0, 0.0))],
    );

    // A re-reported still finger is still a tap.
    let unit = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human)
        .map(|u| (u.id, vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())))
        .expect("an own unit");
    let p = game.presentation.camera.to_screen(unit.1);
    input.now = 5.0;
    apply_events(&mut game, &mut input, &[touch_down(2, p), touch_down(2, p)]);
    apply_events(&mut game, &mut input, &[touch_up(2, p)]);
    assert_eq!(game.presentation.selection.units, vec![unit.0]);
}

#[test]
fn a_pair_finger_falsely_reported_lifted_keeps_panning() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let a = vec2(400.0, 300.0);
    let b = vec2(600.0, 360.0);
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, a)]);
    apply_events(&mut game, &mut input, &[touch_down(2, b), touch_down(1, a)]);
    // Finger 2 lifts, and iOS reports both lifted, survivor first.
    apply_events(&mut game, &mut input, &[touch_up(1, a), touch_up(2, b)]);
    assert!(input.touches.is_empty(), "premise: both reported lifted");
    let before = game.presentation.camera.center;
    apply_events(&mut game, &mut input, &[touch_move(1, a - vec2(4.0, 0.0))]);
    assert_eq!(
        game.presentation.camera.center, before,
        "no pan inside the slop"
    );
    apply_events(&mut game, &mut input, &[touch_move(1, a - vec2(80.0, 0.0))]);
    assert_ne!(game.presentation.camera.center, before, "the survivor pans");
    let units = game.presentation.selection.units.clone();
    let buildings = game.presentation.selection.buildings.clone();
    apply_events(&mut game, &mut input, &[touch_up(1, a - vec2(80.0, 0.0))]);
    assert_eq!(game.presentation.selection.units, units, "and never taps");
    assert_eq!(game.presentation.selection.buildings, buildings);
    // The real lift forgets it: that id never moves the camera again.
    let after = game.presentation.camera.center;
    apply_events(
        &mut game,
        &mut input,
        &[touch_move(1, a - vec2(200.0, 0.0))],
    );
    assert_eq!(game.presentation.camera.center, after);
}

#[test]
fn a_falsely_lifted_minimap_finger_keeps_steering_and_never_pans() {
    let on_map = vec2(1100.0, 650.0);
    let drag = [vec2(1200.0, 700.0), vec2(900.0, 400.0), vec2(500.0, 400.0)];
    // One minimap finger that is never interrupted...
    let mut steady = headless_game();
    let mut input = InputState::new();
    publish_minimap(&steady);
    apply_events(&mut steady, &mut input, &[touch_down(1, on_map)]);
    for p in drag {
        apply_events(&mut steady, &mut input, &[touch_move(1, p)]);
    }
    // ...and the same finger after iOS reported it lifted along with a
    // second finger, off the minimap where a world finger would pan.
    let mut game = headless_game();
    let mut input = InputState::new();
    publish_minimap(&game);
    let world = vec2(400.0, 300.0);
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, on_map)]);
    apply_events(&mut game, &mut input, &[touch_down(2, world)]);
    apply_events(
        &mut game,
        &mut input,
        &[touch_up(2, world), touch_up(1, on_map)],
    );
    for p in drag {
        apply_events(&mut game, &mut input, &[touch_move(1, p)]);
    }
    assert_eq!(
        game.presentation.camera.center,
        steady.presentation.camera.center
    );
}

#[test]
fn a_move_from_an_unknown_finger_does_nothing() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let before = game.presentation.camera.center;
    // The tutorial card swallowed this finger's landing.
    for x in [400.0, 480.0, 560.0] {
        apply_events(&mut game, &mut input, &[touch_move(7, vec2(x, 300.0))]);
    }
    apply_events(&mut game, &mut input, &[touch_up(7, vec2(560.0, 300.0))]);
    assert_eq!(game.presentation.camera.center, before);
    assert!(input.touches.is_empty());
}

#[test]
fn a_box_survivor_pans_only_past_the_slop() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let a = game.presentation.camera.to_screen(vec2(2.0, 2.0));
    let b = game.presentation.camera.to_screen(vec2(12.0, 10.0));
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, a)]);
    apply_events(&mut game, &mut input, &[touch_down(2, b)]);
    input.now = 1.0 + (touch::BOX_REST_MS + 10.0) / 1000.0;
    update_touch(&mut game, &mut input);
    apply_events(&mut game, &mut input, &[touch_up(2, b)]);
    assert!(
        !game.presentation.selection.units.is_empty(),
        "the box landed"
    );
    let before = game.presentation.camera.center;
    apply_events(&mut game, &mut input, &[touch_move(1, a - vec2(5.0, 0.0))]);
    assert_eq!(
        game.presentation.camera.center, before,
        "jitter is not a pan"
    );
    apply_events(&mut game, &mut input, &[touch_move(1, a - vec2(80.0, 0.0))]);
    assert_ne!(game.presentation.camera.center, before);
}

#[test]
fn a_gentle_spread_commits_to_zooming_before_the_box_can_claim_it() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let a = vec2(500.0, 400.0);
    let b = vec2(600.0, 400.0);
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, a), touch_down(2, b)]);
    input.now = 1.2;
    apply_events(&mut game, &mut input, &[touch_move(2, b + vec2(16.0, 0.0))]);
    assert_eq!(
        input.pair.map(|pair| pair.state),
        Some(touch::PairState::Pinch)
    );
    input.now = 3.0;
    update_touch(&mut game, &mut input);
    assert_eq!(
        input.pair.map(|pair| pair.state),
        Some(touch::PairState::Pinch),
        "resting after the pinch starts never claims a box"
    );
    apply_events(&mut game, &mut input, &[touch_up(2, b + vec2(16.0, 0.0))]);
    assert!(game.presentation.selection.units.is_empty());
}

#[test]
fn a_resting_pair_claims_its_box_as_it_appears() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let a = game.presentation.camera.to_screen(vec2(2.0, 2.0));
    let b = game.presentation.camera.to_screen(vec2(4.0, 4.0));
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, a), touch_down(2, b)]);
    assert_eq!(touch_box(&input), None, "a fresh pair may be a pinch");
    input.now = 1.0 + (TOUCH_REST_MS + 10.0) / 1000.0;
    update_touch(&mut game, &mut input);
    assert_eq!(
        touch_box(&input),
        None,
        "nothing shows before it can be used"
    );
    input.now = 1.0 + (touch::BOX_REST_MS + 10.0) / 1000.0;
    update_touch(&mut game, &mut input);
    assert_eq!(
        touch_box(&input),
        Some((a, b)),
        "the box appears as the rest claims it"
    );

    // Claimed, a corner drag resizes instead of zooming.
    let zoom = game.presentation.camera.zoom;
    let far = game.presentation.camera.to_screen(vec2(12.0, 10.0));
    apply_events(&mut game, &mut input, &[touch_move(2, far)]);
    game.presentation.camera.update(1.0); // land any glide: headless has no frames
    assert_eq!(game.presentation.camera.zoom, zoom, "no pinch once claimed");
    assert_eq!(touch_box(&input), Some((a, far)));
    apply_events(&mut game, &mut input, &[touch_up(2, far)]);
    assert!(
        !game.presentation.selection.units.is_empty(),
        "the dragged-out box swept the base"
    );
    assert_eq!(touch_box(&input), None);
}

#[test]
fn a_pair_that_moves_before_resting_still_pinches() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let a = vec2(400.0, 300.0);
    input.now = 1.0;
    apply_events(
        &mut game,
        &mut input,
        &[touch_down(1, a), touch_down(2, a + vec2(260.0, 0.0))],
    );
    let zoom = game.presentation.camera.zoom;
    // One finger still, the other squeezing in: the common thumb-anchored
    // pinch grip.
    apply_events(
        &mut game,
        &mut input,
        &[touch_move(2, a + vec2(120.0, 0.0))],
    );
    assert_eq!(
        input.pair.map(|pair| pair.state),
        Some(touch::PairState::Pinch)
    );
    game.presentation.camera.update(1.0);
    assert_ne!(game.presentation.camera.zoom, zoom);
    input.now = 3.0;
    update_touch(&mut game, &mut input);
    assert_eq!(touch_box(&input), None, "a pinch never becomes a box");
}

#[test]
fn a_pair_formed_mid_pan_or_while_armed_never_boxes() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let a = game.presentation.camera.to_screen(vec2(2.0, 2.0));
    let b = game.presentation.camera.to_screen(vec2(12.0, 10.0));
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, a)]);
    apply_events(&mut game, &mut input, &[touch_move(1, a - vec2(40.0, 0.0))]);
    apply_events(&mut game, &mut input, &[touch_down(2, b)]);
    apply_events(&mut game, &mut input, &[touch_up(2, b)]);
    assert!(
        game.presentation.selection.units.is_empty(),
        "a pan never boxes"
    );
    apply_events(&mut game, &mut input, &[touch_up(1, a - vec2(40.0, 0.0))]);

    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .expect("a harvester")
        .id;
    game.presentation.selection.units = vec![harvester];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    input.build_menu = true;
    input.now = 5.0;
    apply_events(&mut game, &mut input, &[touch_down(3, a), touch_down(4, b)]);
    input.now = 6.0;
    update_touch(&mut game, &mut input);
    assert_eq!(touch_box(&input), None);
    apply_events(&mut game, &mut input, &[touch_up(4, b)]);
    assert_eq!(game.presentation.selection.units, vec![harvester]);
    assert_eq!(
        input.placing,
        Some(oxide_sim::BuildingKind::Turret),
        "still armed"
    );
}

#[test]
fn the_queue_toggle_makes_touch_queue_and_add() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let own: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .map(|u| (u.id, vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())))
        .take(2)
        .collect();
    let [(first, first_at), (second, second_at)] = own[..] else {
        panic!("two own units");
    };
    input.queue_toggle = true;
    tap_world(&mut game, &mut input, first_at);
    tap_world(&mut game, &mut input, second_at);
    let mut picked = game.presentation.selection.units.clone();
    picked.sort();
    let mut both = vec![first, second];
    both.sort();
    assert_eq!(picked, both, "a queued tap adds to the selection");

    // A quick second tap on the same unit toggles it rather than
    // sweeping every unit of its kind.
    input.now += 1.0;
    let p = game.presentation.camera.to_screen(first_at);
    apply_events(&mut game, &mut input, &[touch_down(1, p), touch_up(1, p)]);
    input.now += 0.1;
    apply_events(&mut game, &mut input, &[touch_down(1, p), touch_up(1, p)]);
    assert!(
        game.presentation.selection.units.len() <= 2,
        "no kind sweep"
    );

    game.presentation.selection.units = vec![first];
    long_press_world(&mut game, &mut input, first_at + vec2(4.0, 2.0));
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { queue: true, .. })),
        "a queued long-press appends the order: {:?}",
        game.pending
    );
}

#[test]
fn the_queue_chip_flips_by_tap_and_click() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let chip = macroquad::math::Rect::new(500.0, 620.0, 96.0, 44.0);
    let mut layout = bare_layout(680.0, 500.0);
    layout.queue_toggle = chip;
    game.presentation.layout.set(layout);
    tap(&mut game, &mut input, chip.center());
    assert!(input.queue_held());
    assert!(
        game.presentation.toasts.is_empty(),
        "the lit chip speaks for itself"
    );
    apply_events(
        &mut game,
        &mut input,
        &click(chip.center().x, chip.center().y),
    );
    assert!(!input.queue_held());
    assert!(
        game.presentation.selection.units.is_empty(),
        "the chip is chrome"
    );
    input.queue_toggle = true;
    input.reset_transient();
    assert!(!input.queue_held(), "leaving the screen drops it");
}

fn armed_patrol() -> (Game, InputState, oxide_sim::UnitId) {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind.stats().can_fight())
        .expect("a starting combat unit")
        .id;
    game.presentation.selection.units = vec![fighter];
    dispatch_action(&mut game, &mut input, Action::Patrol);
    assert_eq!(
        input.patrol_route,
        Some(Vec::new()),
        "premise: patrol armed"
    );
    (game, input, fighter)
}

#[test]
fn taps_collect_a_patrol_route_and_the_card_starts_it() {
    let (mut game, mut input, fighter) = armed_patrol();
    let minimap = publish_minimap(&game);
    let before = game.presentation.camera.center;
    tap_world(&mut game, &mut input, vec2(9.5, 5.5));
    tap_world(&mut game, &mut input, vec2(12.5, 8.5));
    tap(
        &mut game,
        &mut input,
        vec2(minimap.x + 150.0, minimap.y + 120.0),
    );
    assert_eq!(
        game.presentation.camera.center, before,
        "the minimap tap is a waypoint"
    );
    assert_eq!(input.patrol_route.as_ref().map(Vec::len), Some(3));
    assert_eq!(
        game.presentation.selection.units,
        vec![fighter],
        "taps never reselect"
    );
    dispatch_action(&mut game, &mut input, Action::Patrol);
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            Command::Patrol { waypoints, .. } if waypoints.len() == 3
        )),
        "the second Patrol starts the circuit: {:?}",
        game.pending
    );
}

#[test]
fn a_left_click_adds_a_patrol_waypoint_and_a_full_route_says_so() {
    let (mut game, mut input, _) = armed_patrol();
    let p = game.presentation.camera.to_screen(vec2(9.5, 5.5));
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    assert_eq!(input.patrol_route.as_ref().map(Vec::len), Some(1));
    input.patrol_route = Some(vec![TilePos::new(9, 5); oxide_sim::stats::ORDER_QUEUE_CAP]);
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    assert_eq!(
        input.patrol_route.as_ref().map(Vec::len),
        Some(oxide_sim::stats::ORDER_QUEUE_CAP)
    );
    assert!(
        game.presentation
            .toasts
            .iter()
            .any(|t| t.text.starts_with("Patrol is full"))
    );
}

#[test]
fn a_long_press_honors_the_armed_mode() {
    // Collecting a route, a long rest never orders: its lift is just a
    // slow tap, adding the waypoint under it.
    let (mut game, mut input, fighter) = armed_patrol();
    long_press_world(&mut game, &mut input, vec2(12.5, 8.5));
    assert!(game.pending.is_empty());
    assert_eq!(input.patrol_route, Some(vec![TilePos::new(12, 8)]));

    // Any other armed verb stands down, as for a right-click, and the
    // long-press issues its own order.
    input.patrol_route = None;
    input.click_verb = Some(ClickVerb::Hunt);
    game.presentation.selection.units = vec![fighter];
    long_press_world(&mut game, &mut input, vec2(12.5, 8.5));
    assert!(!input.armed(ClickVerb::Hunt), "the armed verb stood down");
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { .. })),
        "the long-press ordered: {:?}",
        game.pending
    );
}

#[test]
fn patrol_is_exclusive_with_the_other_armed_verbs() {
    let (mut game, mut input, _) = armed_patrol();
    dispatch_action(&mut game, &mut input, Action::Hunt);
    assert!(input.armed(ClickVerb::Hunt));
    assert_eq!(input.patrol_route, None, "arming hunt drops the route");
    dispatch_action(&mut game, &mut input, Action::Patrol);
    assert!(
        !input.armed(ClickVerb::Hunt),
        "arming patrol stands hunt down"
    );
    assert_eq!(input.patrol_route, Some(Vec::new()));
}

#[test]
fn patrol_copy_speaks_touch_on_touch_only_builds() {
    assert_eq!(
        patrol_arm_toast("R", false),
        "Patrol: click waypoints, R to start"
    );
    crate::platform::assert_touch_copy(&patrol_arm_toast("R", true));
    crate::platform::assert_touch_copy(&patrol_full_toast("R", true));
}

fn own_fighter(game: &Game) -> (oxide_sim::UnitId, Vec2) {
    game.state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind.stats().can_fight())
        .map(|u| (u.id, vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())))
        .expect("a starting combat unit")
}

#[test]
fn a_ground_tap_deselects_and_a_long_press_orders() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (fighter, at) = own_fighter(&game);
    let last_ping_queued = |game: &Game| {
        game.presentation
            .fx
            .iter()
            .rev()
            .find_map(|fx| match fx.kind {
                crate::game::EffectKind::Ping { queued, .. } => Some(queued),
                _ => None,
            })
    };
    game.presentation.selection.units = vec![fighter];
    tap_world(&mut game, &mut input, at + vec2(4.0, 2.0));
    assert!(
        game.presentation.selection.units.is_empty(),
        "a tap deselects"
    );
    assert!(game.pending.is_empty(), "and never orders");

    game.presentation.selection.units = vec![fighter];
    long_press_world(&mut game, &mut input, at + vec2(4.0, 2.0));
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { queue: false, .. })),
        "the long-press ordered: {:?}",
        game.pending
    );
    assert_eq!(last_ping_queued(&game), Some(false));

    game.pending.clear();
    input.queue_toggle = true;
    long_press_world(&mut game, &mut input, at + vec2(4.0, -2.0));
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { queue: true, .. })),
        "QUEUE queues the long-press's order"
    );
    assert_eq!(last_ping_queued(&game), Some(true), "and its ping says so");
}

#[test]
fn a_fingertip_that_just_misses_a_unit_still_selects_it() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (fighter, _) = own_fighter(&game);
    let other = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.id != fighter)
        .map(|u| (u.id, vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())))
        .expect("a second own unit");
    game.presentation.selection.units = vec![fighter];
    // Beyond a cursor's reach, inside a fingertip's.
    let reach = super::unit_pick_radius(game.state.unit(other.0).expect("the second unit").kind)
        .max(10.0 / game.presentation.camera.zoom);
    tap_world(&mut game, &mut input, other.1 + vec2(reach + 0.05, 0.0));
    assert!(game.pending.is_empty(), "a near miss never orders");
    assert_eq!(game.presentation.selection.units, vec![other.0]);
}

#[test]
fn a_double_tap_on_a_unit_sweeps_its_kind() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let harvesters: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .map(|u| (u.id, vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())))
        .collect();
    assert!(harvesters.len() > 1, "premise: several harvesters");
    let p = game.presentation.camera.to_screen(harvesters[0].1);
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, p), touch_up(1, p)]);
    input.now = 1.1;
    apply_events(&mut game, &mut input, &[touch_down(1, p), touch_up(1, p)]);
    assert_eq!(
        game.presentation.selection.units.len(),
        harvesters.len(),
        "a double tap on a unit sweeps its kind"
    );
}

#[test]
fn a_tap_on_a_scrap_pile_selects_it_for_its_panel() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let tile = TilePos::new(7, 2);
    let scrap = game.state.map().scrap_at(tile);
    assert!(scrap > 0, "premise: the home pile holds scrap");
    let (fighter, _) = own_fighter(&game);
    game.presentation.selection.units = vec![fighter];
    tap_world(&mut game, &mut input, vec2(7.5, 2.5));
    assert!(game.presentation.selection.units.is_empty());
    assert_eq!(game.presentation.selection.pile, Some(tile));
    let panel =
        crate::panel::build_for_input(&game.view(), &classic(), &input).expect("a pile panel");
    assert_eq!(panel.title, "Scrap pile");
    assert!(panel.cards.is_empty(), "a pile takes no orders");
    let row = |panel: &crate::panel::Panel, label: &str| {
        panel
            .info
            .rows
            .iter()
            .find(|row| row.label == label)
            .map(|row| row.value.clone())
    };
    assert_eq!(row(&panel, "Scrap left"), Some(scrap.to_string()));
    assert_eq!(row(&panel, "Harvesters"), Some("0".into()));

    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .expect("a harvester")
        .id;
    game.issue(Command::Harvest {
        units: vec![harvester],
        node: tile,
        queue: false,
    });
    game.present_ticks(1);
    let panel =
        crate::panel::build_for_input(&game.view(), &classic(), &input).expect("a pile panel");
    assert_eq!(row(&panel, "Harvesters"), Some("1".into()));

    tap_world(&mut game, &mut input, vec2(12.5, 12.5));
    assert_eq!(
        game.presentation.selection.pile, None,
        "bare ground clears it"
    );
}

#[test]
fn fingertip_slop_never_takes_a_tap_from_the_pile_under_it() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let pile = vec2(7.5, 2.5);
    let (nearest, distance) = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .map(|u| {
            let at = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (u, at.distance(pile))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("an own unit");
    assert!(
        distance > super::unit_pick_radius(nearest.kind),
        "premise: the tap misses the unit's body"
    );
    // Zoomed out until a fingertip's reach covers the unit.
    game.presentation.camera.zoom = 22.0 * input.ui / (distance + 0.1);
    tap_world(&mut game, &mut input, pile);
    assert!(game.presentation.selection.units.is_empty());
    assert_eq!(game.presentation.selection.pile, Some(TilePos::new(7, 2)));
}

#[test]
fn selecting_anything_else_drops_the_pile() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let tile = TilePos::new(7, 2);
    game.presentation.selection.pile = Some(tile);
    let (fighter, at) = own_fighter(&game);
    tap_world(&mut game, &mut input, at);
    assert_eq!(game.presentation.selection.units, vec![fighter]);
    assert_eq!(game.presentation.selection.pile, None);

    game.presentation.selection.units.clear();
    game.presentation.selection.pile = Some(tile);
    dispatch_action(&mut game, &mut input, Action::CycleIdleWorker);
    apply_events(&mut game, &mut input, &[]);
    assert!(!game.presentation.selection.units.is_empty());
    assert_eq!(
        game.presentation.selection.pile, None,
        "any selection writer, not just taps"
    );
}

#[test]
fn a_pile_the_viewer_cannot_know_is_never_selected_and_is_dropped_once_empty() {
    let mut game = headless_game();
    let input = InputState::new();
    let map = game.state.map();
    let tiles: Vec<TilePos> = (0..map.height())
        .flat_map(|y| (0..map.width()).map(move |x| TilePos::new(x, y)))
        .collect();
    let unseen = *tiles
        .iter()
        .find(|&&t| map.scrap_at(t) > 0 && !game.my_vision().explored(t))
        .expect("scrap the human has never seen");
    let empty = *tiles
        .iter()
        .find(|&&t| game.my_vision().visible(t) && map.scrap_at(t) == 0 && map.wreck_at(t) == 0)
        .expect("visible bare ground");
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(unseen.x as f32 + 0.5, unseen.y as f32 + 0.5));
    select::click_select(&mut game, screen, false, input.ui, Pointer::Touch);
    assert_eq!(game.presentation.selection.pile, None, "fog hides it");

    game.presentation.selection.pile = Some(empty);
    game.present_ticks(1);
    assert_eq!(
        game.presentation.selection.pile, None,
        "a tile with no known salvage drops out on the next tick"
    );
}

#[test]
fn touch_windows_keep_their_ordering_invariant() {
    // A hand-edited config cannot make a lazy double-tap read as a
    // long-press: the press window clamps strictly above the tap one.
    let prefs = crate::config::TouchPrefs {
        double_tap_ms: 600,
        long_press_ms: 300,
    }
    .clamped();
    assert!(prefs.long_press_ms > prefs.double_tap_ms);
}

#[test]
fn an_allied_site_under_fog_refuses_selection() {
    // Ally SITES are blind until built, so one beyond own sight is
    // fogged on screen — and must not be blind-clickable, or the
    // panel leaks its live kind and hp through the fog. The ally's
    // BUILT foundry stays selectable: it sees its own ground, and
    // team sight is shared.
    use oxide_sim::scenario::{BotConfig, PlayerSpec, UnitSpec};
    let seat = |name: &str, faction, team| PlayerSpec {
        name: name.into(),
        faction,
        team: Some(team),
        scrap: 300,
        bot: false,
        bot_config: None,
    };
    let mut scenario = oxide_sim::Scenario {
        mode: ScenarioMode::Match,
        name: "ally-site-arena".into(),
        map: vec![
            "################################".into(),
            "#1.............................#".into(),
            "#..............................#".into(),
            "#..............................#".into(),
            "#..............................#".into(),
            "#3.........................2...#".into(),
            "#..............................#".into(),
            "################################".into(),
        ],
        players: vec![
            seat("West", oxide_sim::Faction::Ferrous, 0),
            seat("East Ally", oxide_sim::Faction::Cupric, 0),
            seat("Foe", oxide_sim::Faction::Ferrous, 1),
        ],
        units: vec![UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 25,
            y: 4,
        }],
        buildings: Vec::new(),
        meta: None,
    };
    for p in scenario.players.iter_mut().skip(1) {
        p.bot = true;
        p.bot_config = Some(BotConfig::default());
    }
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("ally arena builds");
    let mut input = InputState::new();
    // The ally's harvester claims a site well outside seat 0's sight.
    let ally_worker = game
        .state
        .units()
        .iter()
        .find(|u| u.player == oxide_sim::PlayerId(1))
        .unwrap()
        .id;
    // Placement wants the footprint visible to the PLACER, so the
    // worker walks into sight of the ground first, claims, and then
    // goes home: a site is blind, and once every friendly eye leaves,
    // its ground goes dark.
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: Command::Run {
            units: vec![ally_worker],
            goal: TilePos::new(16, 2),
            queue: false,
        },
    }]);
    for _ in 0..400 {
        game.state.tick(&[]);
        if game.state.unit(ally_worker).unwrap().tile() == TilePos::new(16, 2) {
            break;
        }
    }
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: Command::Build {
            units: vec![ally_worker],
            kind: oxide_sim::BuildingKind::Turret,
            anchor: TilePos::new(15, 1),
            queue: false,
            defer: false,
        },
    }]);
    assert!(
        game.state
            .buildings()
            .iter()
            .any(|b| b.kind == oxide_sim::BuildingKind::Turret),
        "test premise: the claim landed instantly"
    );
    for _ in 0..100 {
        if game.state.buildings().iter().any(|b| {
            b.kind == oxide_sim::BuildingKind::Turret && b.construction_progress().unwrap_or(0) > 0
        }) {
            break;
        }
        game.state.tick(&[]);
    }
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: Command::Run {
            units: vec![ally_worker],
            goal: TilePos::new(25, 4),
            queue: false,
        },
    }]);
    for _ in 0..400 {
        game.state.tick(&[]);
        if game.state.unit(ally_worker).unwrap().tile() == TilePos::new(25, 4) {
            break;
        }
    }
    let site_center = {
        let site = game
            .state
            .buildings()
            .iter()
            .find(|b| b.kind == oxide_sim::BuildingKind::Turret)
            .expect("the ally claimed the site");
        assert!(!site.built(), "test premise: unfinished");
        assert!(
            !site.tiles().any(|t| game.my_vision().visible(t)),
            "test premise: the site sits under fog"
        );
        vec2(site.anchor.x as f32 + 0.5, site.anchor.y as f32 + 0.5)
    };
    game.presentation.camera.center = site_center;
    let screen = game.presentation.camera.to_screen(site_center);
    input.now = 5.0; // clicks land at the viewport center; keep them
    apply_events(&mut game, &mut input, &click(screen.x, screen.y)); // out of double-click range
    assert!(
        game.presentation.selection.buildings.is_empty(),
        "a fogged ally site must refuse the blind click"
    );
    // The built ally foundry selects through shared team sight.
    let (ally_foundry, center) = {
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == oxide_sim::PlayerId(1) && b.built())
            .unwrap();
        (
            foundry.id,
            vec2(foundry.anchor.x as f32 + 1.0, foundry.anchor.y as f32 + 1.0),
        )
    };
    game.presentation.camera.center = center;
    let screen = game.presentation.camera.to_screen(center);
    input.now = 10.0;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(
        game.presentation.selection.buildings,
        vec![ally_foundry],
        "the built ally building stays inspectable"
    );
}

#[test]
fn a_foreign_box_never_reaches_through_fog() {
    // One visible enemy scout at the fog's edge must not drag its
    // owner's HIDDEN units into an inspectable selection.
    let scenario = oxide_sim::Scenario::from_json(
        "{
        \"name\": \"fog box\",
        \"players\": [
            {\"name\": \"me\", \"faction\": \"ferrous\", \"scrap\": 100, \"bot\": false},
            {\"name\": \"foe\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true}
        ],
        \"map\": [
            \"##############################\",
            \"#............................#\",
            \"#..1.........................#\",
            \"#............................#\",
            \"#..........................2.#\",
            \"#............................#\",
            \"##############################\"
        ],
        \"units\": [
            {\"player\": 0, \"kind\": \"harvester\", \"x\": 6, \"y\": 2},
            {\"player\": 1, \"kind\": \"scuttler\", \"x\": 10, \"y\": 2},
            {\"player\": 1, \"kind\": \"scuttler\", \"x\": 20, \"y\": 2}
        ]
    }",
    )
    .expect("parses");
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    let mut input = InputState::new();
    let near = game.state.units()[1].id;
    let far = game.state.units()[2].id;
    assert!(
        game.my_vision()
            .visible(game.state.unit(near).unwrap().tile()),
        "premise: the scout stands in sight"
    );
    assert!(
        !game
            .my_vision()
            .visible(game.state.unit(far).unwrap().tile()),
        "premise: its army hides in fog"
    );
    // A box spanning both, with no own units inside.
    let a = game.presentation.camera.to_screen(vec2(9.0, 1.2));
    let b = game.presentation.camera.to_screen(vec2(21.5, 3.5));
    apply_events(
        &mut game,
        &mut input,
        &[left_down(a), mouse_move(b), left_up(b)],
    );
    assert_eq!(
        game.presentation.selection.units,
        vec![near],
        "only the visible scout inspects"
    );
}

#[test]
fn a_selected_hostile_drops_when_fog_recovers_it() {
    // The panel reads live hp off the selection: an inspection must
    // never become a tracking beacon into ground the player no longer
    // sees. Once nothing of the player's stands near the foe, its
    // ground goes dark and the selection lets go.
    let scenario = oxide_sim::Scenario::from_json(
        "{
        \"name\": \"beacon\",
        \"players\": [
            {\"name\": \"me\", \"faction\": \"ferrous\", \"scrap\": 100, \"bot\": false},
            {\"name\": \"foe\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true}
        ],
        \"map\": [
            \"##############################\",
            \"#............................#\",
            \"#..1.........................#\",
            \"#............................#\",
            \"#........................s.2.#\",
            \"#............................#\",
            \"##############################\"
        ],
        \"units\": [
            {\"player\": 0, \"kind\": \"harvester\", \"x\": 12, \"y\": 2},
            {\"player\": 1, \"kind\": \"harvester\", \"x\": 14, \"y\": 2}
        ]
    }",
    )
    .expect("parses");
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    let mut input = InputState::new();
    let foe = game.state.units()[1].id;
    let pos = game.state.units()[1].pos;
    assert!(
        game.my_vision()
            .visible(game.state.unit(foe).unwrap().tile()),
        "premise: the foe worker stands in my harvester's sight"
    );
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(game.presentation.selection.units, vec![foe]);
    // Send my only nearby eyes home; the foe's bot recalls its
    // harvester east to mine — both walks end my sight of it, and the
    // selection must end with the sight (the machine itself lives on).
    let mine = game.state.units()[0].id;
    game.issue(Command::Run {
        units: vec![mine],
        goal: TilePos::new(3, 4),
        queue: false,
    });
    for _ in 0..600 {
        game.do_tick();
        if game.presentation.selection.units.is_empty() {
            break;
        }
    }
    assert!(
        game.state.unit(foe).is_some(),
        "test premise: the machine is alive, only unseen"
    );
    assert!(
        game.presentation.selection.units.is_empty(),
        "the inspection let go with the sight"
    );
}

/// Publishes a layout whose minimap owns the window's bottom-right
/// corner — the chrome-ownership tests need real rects, exactly as the
/// renderer would publish them.
fn publish_minimap(game: &Game) -> macroquad::math::Rect {
    let minimap = macroquad::math::Rect::new(1060.0, 590.0, 200.0, 190.0);
    let mut layout = bare_layout(f32::INFINITY, 0.0);
    layout.minimap = minimap;
    game.presentation.layout.set(layout);
    minimap
}

#[test]
fn hardware_touch_phases_speak_the_funnel_vocabulary() {
    // The polling adapter translates macroquad's touch phases into the
    // exact events the harness injects — one vocabulary, so a real
    // fingertip and an injected one walk identical code.
    use macroquad::prelude::TouchPhase;
    assert!(matches!(
        touch_event(TouchPhase::Started, 3, 1.0, 2.0),
        Some(RawEvent::TouchDown { id: 3, .. })
    ));
    assert!(matches!(
        touch_event(TouchPhase::Moved, 3, 1.0, 2.0),
        Some(RawEvent::TouchMove { id: 3, .. })
    ));
    assert!(matches!(
        touch_event(TouchPhase::Ended, 3, 1.0, 2.0),
        Some(RawEvent::TouchUp { id: 3, .. })
    ));
    assert!(
        matches!(
            touch_event(TouchPhase::Cancelled, 3, 1.0, 2.0),
            Some(RawEvent::TouchUp { id: 3, .. })
        ),
        "a cancelled finger lifts — gesture state must not wait for it"
    );
    assert!(
        touch_event(TouchPhase::Stationary, 3, 1.0, 2.0).is_none(),
        "a resting finger emits nothing; the long-press timer rides the frame loop"
    );
}

#[test]
fn hardware_touches_arrive_once_in_order_and_in_logical_pixels() {
    use macroquad::miniquad::{EventHandler, TouchPhase};
    let mut stream = PointerStream::new(2.0, false);
    stream.touch_event(TouchPhase::Started, 7, 200.0, 100.0);
    stream.touch_event(TouchPhase::Ended, 7, 202.0, 100.0);
    assert_eq!(
        stream.events,
        vec![
            RawEvent::TouchDown {
                id: 7,
                x: 100.0,
                y: 50.0
            },
            RawEvent::TouchUp {
                id: 7,
                x: 101.0,
                y: 50.0
            },
        ],
        "a tap inside one frame keeps both edges, divided out of backing pixels"
    );
}

#[test]
fn a_minimap_drag_steers_the_camera_like_the_mouse() {
    // The same path by finger and by mouse, from the minimap's far
    // corner, back across it, and off its edge (clamped).
    let path = |minimap: macroquad::math::Rect| {
        [
            vec2(minimap.x + 180.0, minimap.y + 170.0),
            vec2(minimap.x + 30.0, minimap.y + 30.0),
            vec2(400.0, 300.0),
        ]
    };
    let mut by_touch = Vec::new();
    let mut game = headless_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let [land, cross, off] = path(minimap);
    apply_events(&mut game, &mut input, &[touch_down(1, land)]);
    by_touch.push(game.presentation.camera.center);
    for p in [cross, off] {
        apply_events(&mut game, &mut input, &[touch_move(1, p)]);
        by_touch.push(game.presentation.camera.center);
    }
    apply_events(&mut game, &mut input, &[touch_up(1, off)]);
    assert_eq!(
        game.presentation.camera.center, by_touch[2],
        "lifting changes nothing"
    );

    let mut by_mouse = Vec::new();
    let mut game = headless_game();
    let mut input = InputState::new();
    publish_minimap(&game);
    apply_events(&mut game, &mut input, &[left_down(land)]);
    by_mouse.push(game.presentation.camera.center);
    for p in [cross, off] {
        apply_events(&mut game, &mut input, &[mouse_move(p)]);
        by_mouse.push(game.presentation.camera.center);
    }
    assert_eq!(by_touch, by_mouse);
    assert_ne!(by_touch[0], by_touch[1], "the drag really steered");
}

#[test]
fn a_minimap_tap_with_rally_armed_sets_the_rally_without_steering() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let foundry = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player == game.presentation.human)
        .expect("own Foundry")
        .id;
    game.presentation.selection.buildings = vec![foundry];
    input.rallying = vec![foundry];
    let before = game.presentation.camera.center;
    let p = vec2(minimap.x + 150.0, minimap.y + 150.0);
    input.now = 1.0;
    apply_events(&mut game, &mut input, &[touch_down(1, p)]);
    assert_eq!(game.presentation.camera.center, before);
    apply_events(&mut game, &mut input, &[touch_up(1, p)]);
    assert_eq!(game.presentation.camera.center, before);
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::SetRally { rally: Some(_), .. })),
        "the minimap point became the rally: {:?}",
        game.pending
    );
}

#[test]
fn chrome_born_touches_never_drive_world_gestures() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let center_before = game.presentation.camera.center;

    // A swipe that LANDS on the panel must not pan the world behind
    // it, however far it travels.
    let mut layout = game.presentation.layout.get();
    layout.panel_regions[0] = macroquad::math::Rect::new(0.0, 680.0, 500.0, 120.0);
    game.presentation.layout.set(layout);
    input.now = 2.0;
    apply_events(&mut game, &mut input, &[touch_down(1, vec2(300.0, 720.0))]);
    apply_events(&mut game, &mut input, &[touch_move(1, vec2(400.0, 300.0))]);
    assert_eq!(
        game.presentation.camera.center, center_before,
        "a chrome-born swipe keeps its hands off the camera"
    );
    apply_events(&mut game, &mut input, &[touch_up(1, vec2(400.0, 300.0))]);

    // The same swipe born on open ground pans.
    apply_events(&mut game, &mut input, &[touch_down(2, vec2(400.0, 300.0))]);
    apply_events(&mut game, &mut input, &[touch_move(2, vec2(300.0, 260.0))]);
    assert_ne!(
        game.presentation.camera.center, center_before,
        "a world-born swipe still drags the world"
    );
    apply_events(&mut game, &mut input, &[touch_up(2, vec2(300.0, 260.0))]);

    // A two-finger box with one chrome-born corner selects nothing —
    // even when the pair spans the whole own base.
    let own = game.state.units()[0].pos;
    let base = game
        .presentation
        .camera
        .to_screen(vec2(own.x.to_num::<f32>(), own.y.to_num::<f32>()));
    game.presentation.selection.units.clear();
    apply_events(
        &mut game,
        &mut input,
        &[touch_down(3, vec2(minimap.x + 30.0, minimap.y + 30.0))],
    );
    apply_events(
        &mut game,
        &mut input,
        &[touch_down(4, vec2(base.x - 80.0, base.y - 80.0))],
    );
    apply_events(
        &mut game,
        &mut input,
        &[touch_up(4, vec2(base.x - 80.0, base.y - 80.0))],
    );
    assert!(
        game.presentation.selection.units.is_empty(),
        "a chrome-born corner must not box the base: {:?}",
        game.presentation.selection.units
    );
}

mod order_chips;
mod top_bar;

#[test]
fn the_alert_badge_jumps_the_camera_by_click_or_tap() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let badge = macroquad::math::Rect::new(420.0, 3.0, 120.0, 34.0);
    let mut layout = top_bar_layout();
    layout.alert_badge = badge;
    game.presentation.layout.set(layout);
    let map = game.state.map();
    let alert = vec2(map.width() as f32 * 0.5, map.height() as f32 * 0.5);
    game.presentation.last_alert = Some(alert);
    for touch in [false, true] {
        game.presentation.camera.center = vec2(5.0, 5.0);
        if touch {
            // A fingertip just under the badge still lands on its pad.
            tap(
                &mut game,
                &mut input,
                vec2(badge.center().x, badge.y + badge.h + 3.0),
            );
        } else {
            apply_events(
                &mut game,
                &mut input,
                &click(badge.center().x, badge.center().y),
            );
        }
        assert!(
            game.presentation.camera.center.distance(alert) < 0.01,
            "touch {touch}: {:?}",
            game.presentation.camera.center
        );
    }
    assert!(game.pending.is_empty());
}

#[test]
fn a_tap_on_the_idle_badge_cycles_workers() {
    let mut game = headless_game();
    let mut input = InputState::new();
    // Publish chrome with a live idle badge in the top bar: a fingertip
    // tap must cycle workers as a click does, not be swallowed as bare
    // chrome.
    let badge = macroquad::math::Rect::new(200.0, 4.0, 60.0, 24.0);
    let mut layout = bare_layout(f32::INFINITY, 0.0);
    layout.idle_badge = badge;
    game.presentation.layout.set(layout);
    let before = game.presentation.camera.center;
    input.now = 3.0;
    apply_events(
        &mut game,
        &mut input,
        &[touch_down(1, vec2(badge.x + 10.0, badge.y + 10.0))],
    );
    input.now = 3.1;
    apply_events(
        &mut game,
        &mut input,
        &[touch_up(1, vec2(badge.x + 10.0, badge.y + 10.0))],
    );
    // Cycling an idle worker selects it and jumps the camera to it —
    // either effect proves the badge answered the fingertip.
    assert!(
        !game.presentation.selection.units.is_empty() || game.presentation.camera.center != before,
        "the badge answers a tap like it answers a click"
    );
}

/// Live-play chrome with the menu button and a status target where the
/// top bar draws them at 1280 px wide.
fn top_bar_layout() -> crate::layout::LayoutModel {
    let mut layout = bare_layout(f32::INFINITY, 0.0);
    layout.menu_button = crate::layout::menu_button_rect(1280.0, 1.0, false);
    layout.pause_status = macroquad::math::Rect::new(1180.0, 3.0, 46.0, 34.0);
    layout
}

fn tap(game: &mut Game, input: &mut InputState, p: Vec2) {
    input.now += 1.0;
    apply_events(game, input, &[touch_down(1, p)]);
    input.now += 0.05;
    apply_events(game, input, &[touch_up(1, p)]);
}

#[test]
fn a_click_on_the_menu_button_requests_the_pause_menu() {
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation.layout.set(top_bar_layout());
    let menu = game.presentation.layout.get().menu_button;
    apply_events(
        &mut game,
        &mut input,
        &click(menu.center().x, menu.center().y),
    );
    assert!(input.take_menu_request());
    assert!(
        !input.take_menu_request(),
        "the request is one-shot: taking it clears it"
    );
    input.menu_requested = true;
    input.reset_transient();
    assert!(
        !input.take_menu_request(),
        "a screen change drops a request nobody took"
    );
}

#[test]
fn a_tap_in_the_menu_buttons_touch_pad_requests_the_pause_menu() {
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation.layout.set(top_bar_layout());
    let menu = game.presentation.layout.get().menu_button;
    // Left of the drawn square but inside its 44 px fingertip target.
    let p = vec2(menu.x - 3.0, menu.center().y);
    assert!(!menu.contains(p));
    tap(&mut game, &mut input, p);
    assert!(input.take_menu_request());
}

#[test]
fn a_bar_without_a_menu_button_ignores_its_corner() {
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation
        .layout
        .set(bare_layout(f32::INFINITY, 0.0));
    let corner = crate::layout::menu_button_rect(1280.0, 1.0, false).center();
    apply_events(&mut game, &mut input, &click(corner.x, corner.y));
    tap(&mut game, &mut input, corner);
    assert!(
        !input.take_menu_request(),
        "a spectator's bar publishes no button, so its corner stays inert"
    );
}

#[test]
fn an_armed_mode_never_eats_the_menu_button() {
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation.layout.set(top_bar_layout());
    input.placing = Some(oxide_sim::BuildingKind::Fabricator);
    let menu = game.presentation.layout.get().menu_button.center();
    apply_events(&mut game, &mut input, &click(menu.x, menu.y));
    assert!(input.take_menu_request());
    assert!(game.pending.is_empty(), "no build is staged under the bar");
    tap(&mut game, &mut input, menu);
    assert!(input.take_menu_request());
    assert!(game.pending.is_empty());
}

#[test]
fn a_drag_born_on_the_menu_button_neither_pans_nor_requests() {
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation.layout.set(top_bar_layout());
    let menu = game.presentation.layout.get().menu_button.center();
    let before = game.presentation.camera.center;
    input.now = 3.0;
    apply_events(&mut game, &mut input, &[touch_down(1, menu)]);
    let end = vec2(900.0, 400.0);
    apply_events(&mut game, &mut input, &[touch_move(1, end)]);
    input.now = 3.2;
    apply_events(&mut game, &mut input, &[touch_up(1, end)]);
    assert!(!input.take_menu_request());
    assert_eq!(game.presentation.camera.center, before);
}

#[test]
fn the_status_toggles_pause_by_click_and_by_tap() {
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation.layout.set(top_bar_layout());
    let status = game.presentation.layout.get().pause_status.center();
    apply_events(&mut game, &mut input, &click(status.x, status.y));
    assert!(game.clock.paused);
    apply_events(&mut game, &mut input, &click(status.x, status.y));
    assert!(!game.clock.paused);
    tap(&mut game, &mut input, status);
    assert!(game.clock.paused, "a fingertip pauses like the key");
    tap(&mut game, &mut input, status);
    assert!(!game.clock.paused);
    assert!(!input.take_menu_request());
}

#[test]
fn where_padded_targets_overlap_the_menu_wins() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let mut layout = top_bar_layout();
    // A short clock whose widened fingertip target reaches the menu's.
    layout.pause_status = macroquad::math::Rect::new(1206.0, 3.0, 20.0, 34.0);
    game.presentation.layout.set(layout);
    let p = vec2(1236.0, 20.0);
    assert!(crate::layout::touch_pad(layout.pause_status, 1.0).contains(p));
    tap(&mut game, &mut input, p);
    assert!(input.take_menu_request());
    assert!(!game.clock.paused);
}

#[test]
fn a_minimap_right_click_never_commands_a_foreign_selection() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let foe = game
        .state
        .units()
        .iter()
        .find(|u| u.player != game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![foe];
    let right = |x: f32, y: f32| right_down(vec2(x, y));
    apply_events(
        &mut game,
        &mut input,
        &[right(minimap.x + 30.0, minimap.y + 30.0)],
    );
    assert!(
        game.pending.is_empty(),
        "an inspected foreign army takes no minimap orders: {:?}",
        game.pending
    );
    // The gate is about allegiance, not the minimap: an own selection
    // still orders through it.
    let mine = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![mine];
    apply_events(
        &mut game,
        &mut input,
        &[right(minimap.x + 30.0, minimap.y + 30.0)],
    );
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { .. })),
        "own machines still take the minimap order"
    );
}

#[test]
fn a_minimap_right_click_sets_every_selected_producer_rally() {
    let mut game = empty_multi_producer_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let at = vec2(minimap.x + 30.0, minimap.y + 30.0);
    let world = crate::render::minimap_world_at(&game.view(), at).expect("point is inside minimap");
    let rally = numeric::tile_at(world);
    let mut producers: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| {
            building.player == game.presentation.human && !building.stats().produces.is_empty()
        })
        .map(|building| building.id)
        .collect();
    producers.sort_unstable();
    game.presentation.selection.buildings = producers.clone();

    apply_events(&mut game, &mut input, &[right_down(at)]);

    let staged: Vec<_> = game
        .pending
        .iter()
        .filter_map(|command| match command.command {
            Command::SetRally {
                building,
                rally: Some(tile),
            } => Some((building, tile)),
            _ => None,
        })
        .collect();
    assert_eq!(
        staged,
        producers
            .into_iter()
            .map(|building| (building, rally))
            .collect::<Vec<_>>()
    );
}

#[test]
fn touch_respects_chrome_ownership() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let (mx, my) = (minimap.x + 40.0, minimap.y + 40.0);
    // Zoom in so the camera has travel (the whole small map fits the
    // default view and clamping would eat any jump).
    game.presentation.camera.zoom_at(vec2(640.0, 400.0), 4.0);
    game.presentation.camera.update(1.0); // land the glide: headless has no frames
    game.presentation.camera.center = vec2(4.0, 4.0);
    game.presentation.camera.pan(macroquad::prelude::Vec2::ZERO);
    // A tap on the minimap jumps the camera — it must not select the
    // world ground hiding under the chrome pixel.
    let before = game.presentation.camera.center;
    game.presentation.selection.units = vec![game.state.units()[0].id];
    let selected = game.presentation.selection.units.clone();
    input.now = 3.0;
    apply_events(&mut game, &mut input, &[touch_down(9, vec2(mx, my))]);
    input.now = 3.1;
    apply_events(&mut game, &mut input, &[touch_up(9, vec2(mx, my))]);
    assert_ne!(
        game.presentation.camera.center, before,
        "the tap steered the camera"
    );
    assert_eq!(
        game.presentation.selection.units, selected,
        "and stole no selection"
    );

    // A long-press there orders nothing: chrome owns its ground for
    // the held finger too.
    input.now = 4.0;
    apply_events(&mut game, &mut input, &[touch_down(10, vec2(mx, my))]);
    input.now = 4.6;
    update_touch(&mut game, &mut input);
    assert!(
        game.pending.is_empty(),
        "a held finger on the minimap commands nothing: {:?}",
        game.pending
    );
}

#[test]
fn a_slow_pinch_zooms_and_never_commits_a_box() {
    let mut game = headless_game();
    let mut input = InputState::new();
    game.presentation.selection.units = vec![game.state.units()[0].id];
    let keep = game.presentation.selection.units.clone();
    let zoom_before = game.presentation.camera.zoom;
    apply_events(&mut game, &mut input, &[touch_down(1, vec2(600.0, 400.0))]);
    apply_events(&mut game, &mut input, &[touch_down(2, vec2(640.0, 400.0))]);
    // Sub-pixel-per-event spread: forty gentle half-pixel steps sum to
    // a real pinch even though no single event crosses a threshold.
    for i in 0..40 {
        let x = 640.0 + (i as f32) * 0.9;
        apply_events(&mut game, &mut input, &[touch_move(2, vec2(x, 400.0))]);
    }
    assert_eq!(
        input.pair.map(|pair| pair.state),
        Some(touch::PairState::Pinch),
        "the cumulative spread reads as a pinch"
    );
    game.presentation.camera.update(1.0); // land the glide: headless has no frames
    assert!(
        game.presentation.camera.zoom > zoom_before,
        "and it zoomed in"
    );
    apply_events(&mut game, &mut input, &[touch_up(2, vec2(676.0, 400.0))]);
    apply_events(&mut game, &mut input, &[touch_up(1, vec2(600.0, 400.0))]);
    assert_eq!(
        game.presentation.selection.units, keep,
        "a pinch's release never box-selects"
    );

    // And the NEXT pair starts undecided: a fresh steady pair still
    // commits its box (pinch state must not outlive its fingers).
    let a = game.presentation.camera.to_screen(vec2(2.0, 2.0));
    let b = game.presentation.camera.to_screen(vec2(12.0, 10.0));
    apply_events(&mut game, &mut input, &[touch_down(3, a)]);
    apply_events(&mut game, &mut input, &[touch_down(4, b)]);
    apply_events(&mut game, &mut input, &[touch_up(4, b)]);
    assert!(
        !game.presentation.selection.units.is_empty(),
        "the fresh pair's box landed"
    );
}

#[test]
fn a_placement_drag_stamps_a_row_of_queued_builds() {
    // A funded arena: one harvester, 1000 scrap — room for a wall.
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Drag Range",
            "players": [
                {"name": "Mason", "faction": "ferrous", "scrap": 1000, "bot": false},
                {"name": "Idle", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "map": [
                "################",
                "#1.............#",
                "#..............#",
                "#..............#",
                "#............2.#",
                "#..............#",
                "################"
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 5, "y": 3}
            ]
        })
        .to_string(),
    )
    .expect("drag arena parses");
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);

    // Screen points at the centers of three adjacent open tiles.
    let at = |x: i32, y: i32| {
        game.presentation
            .camera
            .to_screen(vec2(x as f32 + 0.5, y as f32 + 0.5))
    };
    let (a, b, c) = (at(7, 3), at(8, 3), at(9, 3));
    let mut events = vec![left_down(a)];
    events.push(mouse_move(b));
    events.push(mouse_move(c));
    events.push(left_up(c));
    apply_events(&mut game, &mut input, &events);

    let builds: Vec<_> = game
        .pending
        .iter()
        .filter_map(|pc| match &pc.command {
            Command::Build { anchor, queue, .. } => Some((*anchor, *queue)),
            _ => None,
        })
        .collect();
    assert_eq!(builds.len(), 3, "one stroke, three stamps: {builds:?}");
    assert!(!builds[0].1, "the first stamp replaces (no Shift held)");
    assert!(
        builds[1].1 && builds[2].1,
        "drag stamps queue behind the program"
    );
    let anchors: std::collections::BTreeSet<_> = builds.iter().map(|(a, _)| (a.x, a.y)).collect();
    assert_eq!(anchors.len(), 3, "no overlapping footprints");
    assert!(
        input.placing.is_none() && input.placing_stroke.is_none(),
        "release without Shift disarms the mode and closes the stroke"
    );

    // The sim accepts the whole row.
    let commands = std::mem::take(&mut game.pending);
    game.state.tick(&commands);
    assert_eq!(
        game.state
            .buildings()
            .iter()
            .filter(|b| b.kind == oxide_sim::BuildingKind::Turret)
            .count(),
        3,
        "all three sites claimed ground"
    );
}

#[test]
fn the_roster_strip_cuts_a_mixed_selection_both_ways() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let mine: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .map(|u| u.id)
        .collect();
    game.presentation.selection.units = mine.clone();
    let panel = crate::panel::build_for_palette(&game.view(), &classic(), false).expect("panel");
    let strip: Vec<_> = panel
        .roster
        .iter()
        .filter_map(|c| match c.action {
            crate::panel::CardAction::FilterKind(k) => Some((k, c.title.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(strip.len(), 2, "two kinds, two counted cards: {strip:?}");
    assert!(
        panel
            .cards
            .iter()
            .all(|card| !matches!(card.action, crate::panel::CardAction::FilterKind(_))),
        "roster filters must not occupy the command-verb collection"
    );
    assert!(
        strip
            .iter()
            .any(|(k, t)| *k == UnitKind::Harvester && t.contains("x3")),
        "the strip counts its kind: {strip:?}"
    );

    // Ctrl-click drops the kind...
    apply_events(&mut game, &mut input, &[key_down(Key::Ctrl)]);
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::FilterKind(UnitKind::Harvester),
    );
    assert!(
        !game.presentation.selection.units.is_empty()
            && game.presentation.selection.units.iter().all(|id| game
                .state
                .unit(*id)
                .unwrap()
                .kind
                != UnitKind::Harvester),
        "Ctrl cuts the named kind out"
    );
    apply_events(&mut game, &mut input, &[key_up(Key::Ctrl)]);

    // Shift, and touch's lit QUEUE, drop it too.
    let without_harvesters = game.presentation.selection.units.clone();
    for queue in [false, true] {
        game.presentation.selection.units = mine.clone();
        if queue {
            input.queue_toggle = true;
        } else {
            apply_events(&mut game, &mut input, &[key_down(Key::Shift)]);
        }
        activate_card(
            &mut game,
            &mut input,
            crate::panel::CardAction::FilterKind(UnitKind::Harvester),
        );
        assert_eq!(
            game.presentation.selection.units, without_harvesters,
            "queue {queue}: the kind drops out"
        );
        input.queue_toggle = false;
        apply_events(&mut game, &mut input, &[key_up(Key::Shift)]);
    }

    // ...and the plain click keeps only the named kind.
    game.presentation.selection.units = mine;
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::FilterKind(UnitKind::Sentinel),
    );
    assert!(
        !game.presentation.selection.units.is_empty()
            && game.presentation.selection.units.iter().all(|id| game
                .state
                .unit(*id)
                .unwrap()
                .kind
                == UnitKind::Sentinel),
        "a plain click narrows to the kind"
    );
}

#[test]
fn construction_order_cards_stage_their_targeted_cancel_commands() {
    let mut game = headless_game();
    let mut input = InputState::new();

    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::CancelSite(oxide_sim::BuildingId(91)),
    );
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::CancelFound(
            oxide_sim::BuildingKind::Bastion,
            TilePos::new(11, 7),
        ),
    );

    assert_eq!(game.pending.len(), 2);
    assert!(matches!(
        game.pending[0].command,
        Command::Cancel {
            building: oxide_sim::BuildingId(91)
        }
    ));
    assert!(matches!(
        game.pending[1].command,
        Command::CancelFound {
            kind: oxide_sim::BuildingKind::Bastion,
            anchor,
        } if anchor == TilePos::new(11, 7)
    ));
}

#[test]
fn drag_feedback_starts_before_box_selection_does() {
    let origin = vec2(100.0, 100.0);
    assert_eq!(
        drag_feedback(origin, origin, 1.0),
        DragFeedback::Still,
        "an idle press draws nothing"
    );
    assert_eq!(
        drag_feedback(origin, origin + vec2(1.0, 0.0), 1.0),
        DragFeedback::Outline,
        "the first movement draws the box"
    );
    assert_eq!(
        drag_feedback(origin, origin + vec2(6.0, 0.0), 1.0),
        DragFeedback::Outline,
        "the click boundary remains visual feedback only"
    );
    assert_eq!(
        drag_feedback(origin, origin + vec2(6.1, 0.0), 1.0),
        DragFeedback::Selection,
        "unit preview begins exactly when release would box-select"
    );
    assert_eq!(
        drag_feedback(origin, origin + vec2(12.0, 0.0), 2.0),
        DragFeedback::Outline,
        "the semantic boundary still follows UI scale"
    );
    assert_eq!(
        drag_feedback(origin, origin + vec2(12.1, 0.0), 2.0),
        DragFeedback::Selection,
        "scaled movement beyond the boundary previews the selection"
    );
}

/// A mouse already in flight when the button lands: the press must
/// anchor the box where it landed, and the box must be drawable on the
/// very frame of the press. A frame-polled adapter would stamp the
/// button with the frame's last cursor position, moving the anchor
/// forward by a frame of travel and leaving a zero-sized rect until the
/// next frame.
#[test]
fn a_press_mid_flight_anchors_the_box_where_it_landed() {
    use macroquad::miniquad::EventHandler;
    // Retina: the platform speaks backing-store pixels, the shell logical ones.
    let mut stream = PointerStream::new(2.0, false);
    stream.mouse_motion_event(600.0, 400.0);
    stream.mouse_motion_event(760.0, 400.0);
    stream.mouse_button_down_event(macroquad::miniquad::MouseButton::Left, 768.0, 400.0);
    stream.mouse_motion_event(900.0, 400.0);
    stream.mouse_motion_event(1000.0, 400.0);
    assert_eq!(
        stream.events,
        vec![
            mouse_move(vec2(300.0, 200.0)),
            mouse_move(vec2(380.0, 200.0)),
            left_down(vec2(384.0, 200.0)),
            mouse_move(vec2(450.0, 200.0)),
            mouse_move(vec2(500.0, 200.0)),
        ],
        "every event keeps its own position, in arrival order"
    );

    let mut game = headless_game();
    let mut input = InputState::new();
    input.ui = 1.0;
    apply_events(&mut game, &mut input, &stream.events);
    assert_eq!(
        input.drag_origin,
        Some(vec2(384.0, 200.0)),
        "the box anchors at the press, not at the end of the frame"
    );
    assert!(
        input.mouse.distance(input.drag_origin.expect("dragging")) > drag_threshold(input.ui),
        "and the rect is already drawable on the press frame itself"
    );
}

#[test]
fn the_hardware_stream_preserves_backspace_repeat_only_when_requested() {
    use macroquad::miniquad::EventHandler;
    let mods = macroquad::miniquad::KeyMods::default();

    let mut disabled = PointerStream::new(1.0, false);
    disabled.key_down_event(macroquad::miniquad::KeyCode::Backspace, mods, true);
    assert!(
        disabled.events.is_empty(),
        "ordinary screens keep every binding edge-only"
    );

    let mut stream = PointerStream::new(1.0, true);
    stream.key_down_event(macroquad::miniquad::KeyCode::Backspace, mods, false);
    stream.key_down_event(macroquad::miniquad::KeyCode::A, mods, true);
    stream.key_down_event(macroquad::miniquad::KeyCode::Backspace, mods, true);
    stream.key_down_event(macroquad::miniquad::KeyCode::Backspace, mods, true);
    assert_eq!(
        stream.events,
        vec![key_down(Key::Backspace), key_down(Key::Backspace),],
        "the initial edge still comes from polling; only repeat edges use the subscriber"
    );
}

#[test]
fn a_paste_chord_types_the_clipboard_only_into_a_text_field() {
    use macroquad::miniquad::{EventHandler, KeyCode, KeyMods};
    let cmd = KeyMods {
        logo: true,
        ..KeyMods::default()
    };
    let ctrl = KeyMods {
        ctrl: true,
        ..KeyMods::default()
    };
    let clipboard = || Some("10.0.0.2:4200\n\u{e9}".to_owned());

    let mut gameplay = PointerStream::new(1.0, false);
    gameplay.clipboard = clipboard;
    gameplay.key_down_event(KeyCode::V, ctrl, false);
    assert!(gameplay.events.is_empty());

    let mut stream = PointerStream::new(1.0, true);
    stream.clipboard = clipboard;
    stream.key_down_event(KeyCode::V, cmd, false);
    stream.char_event('v', cmd, false);
    let pasted: String = stream
        .events
        .iter()
        .map(|event| match event {
            RawEvent::Text { ch } => *ch,
            other => panic!("{other:?} is not text"),
        })
        .collect();
    assert_eq!(pasted, "10.0.0.2:4200", "printable ASCII, and no stray v");

    stream.events.clear();
    stream.key_down_event(KeyCode::V, KeyMods::default(), false);
    stream.key_down_event(KeyCode::V, ctrl, true);
    let alt_gr = KeyMods {
        ctrl: true,
        alt: true,
        ..KeyMods::default()
    };
    stream.char_event('@', alt_gr, false);
    stream.char_event('v', ctrl, false);
    assert_eq!(stream.events, vec![RawEvent::Text { ch: '@' }]);
}

/// The selection consequence of the same frame: a unit sitting between
/// the press point and where the pointer ended the frame belongs in the
/// box.
#[test]
fn the_stretch_between_press_and_frame_end_still_selects() {
    use macroquad::miniquad::EventHandler;
    let mut game = headless_game();
    let mut input = InputState::new();
    input.ui = 1.0;
    let mine: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .collect();
    let (lo, hi) = (mine[0].pos, mine[mine.len() - 1].pos);
    let a = game
        .presentation
        .camera
        .to_screen(vec2(lo.x.to_num::<f32>() - 1.0, lo.y.to_num::<f32>() - 1.0));
    let b = game
        .presentation
        .camera
        .to_screen(vec2(hi.x.to_num::<f32>() + 1.0, hi.y.to_num::<f32>() + 1.0));
    let want: Vec<_> = mine.iter().map(|u| u.id).collect();

    // One frame: the pointer flies past `a`, the button lands there,
    // and the pointer carries on to `b` before the frame ends.
    let mut stream = PointerStream::new(1.0, false);
    stream.mouse_motion_event(a.x - 40.0, a.y - 40.0);
    stream.mouse_button_down_event(macroquad::miniquad::MouseButton::Left, a.x, a.y);
    stream.mouse_motion_event(b.x, b.y);
    stream.mouse_button_up_event(macroquad::miniquad::MouseButton::Left, b.x, b.y);
    apply_events(&mut game, &mut input, &stream.events);
    let mut got = game.presentation.selection.units.clone();
    got.sort_unstable();
    assert_eq!(got, want, "the whole sweep selects, press point included");

    // The shape a frame-polled adapter would produce for that same frame:
    // one MouseMove at the frame's end, and a press and release stamped
    // there too. Origin == release, so the sweep reads as a bare click.
    let mut input = InputState::new();
    input.ui = 1.0;
    game.presentation.selection.units.clear();
    apply_events(
        &mut game,
        &mut input,
        &[mouse_move(b), left_down(b), left_up(b)],
    );
    assert_ne!(
        game.presentation.selection.units, want,
        "premise: coalescing the frame into its last position loses the drag"
    );
}

/// The drag arena, parameterized by bank: same shape as the funded
/// test's fixture, one harvester, open ground.
fn drag_arena(scrap: u32) -> Game {
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Drag Bank",
            "players": [
                {"name": "Mason", "faction": "ferrous", "scrap": scrap, "bot": false},
                {"name": "Idle", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "map": [
                "######################",
                "#1...................#",
                "#....................#",
                "#....................#",
                "#....................#",
                "#....................#",
                "#....................#",
                "#....................#",
                "#..................2.#",
                "#....................#",
                "######################"
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 7, "y": 4}
            ]
        })
        .to_string(),
    )
    .expect("drag bank parses");
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds")
}

fn staged_builds(game: &Game) -> usize {
    game.pending
        .iter()
        .filter(|pc| matches!(pc.command, Command::Build { .. }))
        .count()
}

/// Drags the left button across `tiles`, running `after_event` once each
/// pointer event has been applied.
fn drag_with(
    game: &mut Game,
    input: &mut InputState,
    tiles: &[(i32, i32)],
    mut after_event: impl FnMut(&mut Game),
) {
    let at = |game: &Game, (x, y): (i32, i32)| {
        game.presentation
            .camera
            .to_screen(vec2(x as f32 + 0.5, y as f32 + 0.5))
    };
    let mut deliver = |game: &mut Game, event: RawEvent| {
        apply_events(game, input, &[event]);
        after_event(game);
    };
    let first = at(game, tiles[0]);
    deliver(game, left_down(first));
    for &tile in &tiles[1..] {
        let p = at(game, tile);
        deliver(game, mouse_move(p));
    }
    let last = at(game, tiles[tiles.len() - 1]);
    deliver(game, left_up(last));
}

fn drag_over(game: &mut Game, input: &mut InputState, tiles: &[(i32, i32)]) {
    drag_with(game, input, tiles, |_| {});
}

/// `drag_over` with the frame loop's heartbeat: pending drains into
/// the sim between pointer events, the way real drags actually run.
fn drag_over_ticking(game: &mut Game, input: &mut InputState, tiles: &[(i32, i32)]) -> usize {
    let mut rejections = 0;
    drag_with(game, input, tiles, |game| {
        let commands = std::mem::take(&mut game.pending);
        let report = game.state.tick(&commands);
        rejections += report
            .events
            .iter()
            .filter(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
            .count();
    });
    rejections
}

#[test]
fn a_ticking_drag_spends_the_whole_bank() {
    // Ten turrets, exactly funded, and the tick charging earlier stamps
    // mid-drag must not make the gate bill them twice and cut the wall
    // short.
    let mut game = drag_arena(1000);
    let mut input = InputState::new();
    game.presentation.selection.units = vec![game.state.units()[0].id];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    // Two short rows bracketing the builder: every anchor stays inside
    // someone's sight even as the builder walks to its first site. A wall
    // drawn off into fog is a different test.
    let tiles: Vec<_> = (4..=8)
        .map(|x| (x, 2))
        .chain((4..=8).map(|x| (x, 6)))
        .collect();
    let rejections = drag_over_ticking(&mut game, &mut input, &tiles);
    assert_eq!(rejections, 0, "the gate stages nothing the sim refuses");
    assert_eq!(
        game.state
            .buildings()
            .iter()
            .filter(|b| b.kind == oxide_sim::BuildingKind::Turret)
            .count(),
        10,
        "a funded wall goes up whole"
    );
    assert_eq!(
        game.state.player(game.presentation.human).scrap,
        0,
        "the bank spends to exactly zero"
    );
}

#[test]
fn a_broke_opening_click_toasts_instead_of_pinging() {
    let mut game = drag_arena(50);
    let mut input = InputState::new();
    game.presentation.selection.units = vec![game.state.units()[0].id];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    drag_over(&mut game, &mut input, &[(9, 4)]);
    assert_eq!(staged_builds(&game), 0, "a broke seat stages nothing");
    assert!(
        input.placing.is_some(),
        "a refusal keeps the mode armed, like any misclick"
    );
}

#[test]
fn a_placement_drag_stops_at_the_bank() {
    let mut game = drag_arena(250);
    let mut input = InputState::new();
    game.presentation.selection.units = vec![game.state.units()[0].id];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    drag_over(&mut game, &mut input, &[(9, 4), (10, 4), (11, 4)]);
    assert_eq!(
        staged_builds(&game),
        2,
        "250 scrap affords two 100-scrap turrets; the third stamp is \
         refused at the gate, not by the sim"
    );
    let commands = std::mem::take(&mut game.pending);
    let report = game.state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. })),
        "the shell staged nothing the sim had to refuse"
    );

    // A single-turret bank stages exactly one: the first click's own
    // cost is reserved through the stroke seed.
    let mut game = drag_arena(150);
    let mut input = InputState::new();
    game.presentation.selection.units = vec![game.state.units()[0].id];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    drag_over(&mut game, &mut input, &[(9, 4), (10, 4), (11, 4)]);
    assert_eq!(staged_builds(&game), 1, "150 scrap affords one turret");
}

#[test]
fn a_shift_stroke_spends_only_the_builders_headroom() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    // Pre-load the program through the sim: one active move plus 30
    // queued — headroom 2.
    let mut fill = vec![PlayerCommand {
        player: game.presentation.human,
        command: Command::Run {
            units: vec![builder],
            goal: TilePos::new(3, 7),
            queue: false,
        },
    }];
    for _ in 0..30 {
        fill.push(PlayerCommand {
            player: game.presentation.human,
            command: Command::Run {
                units: vec![builder],
                goal: TilePos::new(4, 7),
                queue: true,
            },
        });
    }
    game.state.tick(&fill);
    assert_eq!(game.state.unit(builder).unwrap().queue.len(), 30);

    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(&mut game, &mut input, &[key_down(Key::Shift)]);
    drag_over(
        &mut game,
        &mut input,
        &[
            (4, 2),
            (6, 2),
            (8, 2),
            (10, 2),
            (12, 2),
            (4, 6),
            (6, 6),
            (8, 6),
            (10, 6),
            (12, 6),
        ],
    );
    assert_eq!(
        staged_builds(&game),
        2,
        "an active order and thirty queued leave headroom for exactly two"
    );
    let commands = std::mem::take(&mut game.pending);
    let report = game.state.tick(&commands);
    assert!(
        !report.events.iter().any(|e| matches!(
            e,
            oxide_sim::Event::CommandRejected {
                reason: oxide_sim::command::RejectReason::QueueFull,
                ..
            }
        )),
        "the stroke never outruns the queue"
    );
    assert_eq!(
        game.state.unit(builder).unwrap().queue.len(),
        oxide_sim::stats::ORDER_QUEUE_CAP,
        "the queue lands exactly full"
    );
}

#[test]
fn a_fresh_stroke_owns_the_cap_plus_the_active_slot() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    // Rows 2, 4, 6, 8 with free rows between: every site keeps a
    // doorstep, all inside the harvester's vision and clear of both
    // foundry footprints.
    let mut tiles = Vec::new();
    for y in [2, 4, 6, 8] {
        for x in 4..=13 {
            if (x, y) != (7, 4) {
                tiles.push((x, y));
            }
        }
    }
    drag_over(&mut game, &mut input, &tiles);
    assert_eq!(
        staged_builds(&game),
        oxide_sim::stats::ORDER_QUEUE_CAP + 1,
        "one active order plus a full queue is legal"
    );
}

#[test]
fn a_full_queue_refuses_the_opening_shift_stamp() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.presentation.selection.units = vec![builder];
    // Fill the program to the brim in the SIM: one active order plus a
    // full queue — zero headroom for the stamp the click would append.
    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::Run {
            units: vec![builder],
            goal: TilePos::new(15, 2),
            queue: false,
        },
    }]);
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP {
        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::Run {
                units: vec![builder],
                goal: TilePos::new(15, 2),
                queue: true,
            },
        }]);
    }
    assert_eq!(
        game.state.unit(builder).unwrap().queue.len(),
        oxide_sim::stats::ORDER_QUEUE_CAP,
        "the fixture actually filled the queue"
    );
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    let p = game.presentation.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[key_down(Key::Shift), left_down(p), left_up(p)],
    );
    assert!(
        game.pending.is_empty(),
        "zero headroom refuses the opening stamp instead of pinging a doomed build"
    );
    assert!(
        game.presentation
            .sounds_pending
            .iter()
            .any(|(k, _)| matches!(k, crate::game::SoundKind::Denied)),
        "the refusal is audible"
    );
    assert!(input.placing.is_some(), "and the mode stays armed");
}

#[test]
fn a_fogged_leg_draws_at_its_click_in_program_order() {
    let mut game = headless_game();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Sentinel)
        .expect("skirmish authors a sentinel")
        .id;
    game.presentation.selection.units = vec![fighter];
    // First leg into unexplored ground, then a leg back onto explored
    // home turf.
    let fogged = {
        let map = game.state.map();
        let mut found = None;
        'scan: for y in (0..map.height()).rev() {
            for x in (0..map.width()).rev() {
                let t = TilePos::new(x, y);
                if game.state.passable(t) && !game.my_vision().explored(t) {
                    found = Some(t);
                    break 'scan;
                }
            }
        }
        found.expect("skirmish keeps unexplored ground at boot")
    };
    let home = game.state.unit(fighter).unwrap().tile();
    game.state.tick(&[
        PlayerCommand {
            player: game.presentation.human,
            command: Command::Hunt {
                units: vec![fighter],
                goal: fogged,
                queue: false,
            },
        },
        PlayerCommand {
            player: game.presentation.human,
            command: Command::Hunt {
                units: vec![fighter],
                goal: home,
                queue: true,
            },
        },
    ]);
    let unit = game.state.unit(fighter).unwrap();
    assert_eq!(unit.queue.len(), 1, "two-leg program");
    assert!(
        !game.my_vision().explored(fogged),
        "premise: the first leg is still fogged"
    );
    let at = |tile: TilePos| {
        game.presentation
            .camera
            .to_screen(vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5))
    };
    let points: Vec<_> = crumbs(&game, unit)
        .into_iter()
        .map(|(index, point, _)| (index, point))
        .collect();
    assert_eq!(
        points,
        vec![(0, at(fogged)), (1, at(home))],
        "the fogged leg draws at the tile the player clicked, and chip 2 \
         stays waypoint 2"
    );
}

/// A long corridor whose eastern half lies beyond the western seat's sight,
/// with a lone rock at (36, 4), and `units` of `kind` for the western seat.
fn fog_corridor_game(kind: &str, units: impl IntoIterator<Item = (i32, i32)>) -> Game {
    let units: Vec<_> = units
        .into_iter()
        .map(|(x, y)| serde_json::json!({"player": 0, "kind": kind, "x": x, "y": y}))
        .collect();
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Corridor",
            "players": [
                {"name": "Walker", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "Idle", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "map": [
                "################################################",
                "#1...........................................2.#",
                "#..............................................#",
                "#..............................................#",
                "#...................................#..........#",
                "#..............................................#",
                "#..............................................#",
                "#..............................................#",
                "################################################"
            ],
            "units": units
        })
        .to_string(),
    )
    .expect("corridor parses");
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds")
}

#[test]
fn a_group_sent_into_fog_draws_its_click_for_every_member() {
    let mut game = fog_corridor_game("sentinel", (4..8).map(|x| (x, 5)));
    let human = game.presentation.human;
    let group: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == human)
        .map(|u| u.id)
        .collect();
    game.presentation.selection.units = group.clone();
    let clicked = TilePos::new(30, 4);
    assert!(
        !game.my_vision().explored(clicked),
        "premise: the click lands in fog"
    );
    game.state.tick(&[PlayerCommand {
        player: human,
        command: Command::Run {
            units: group.clone(),
            goal: clicked,
            queue: false,
        },
    }]);
    let goals = |game: &Game| -> Vec<oxide_sim::Goal> {
        group
            .iter()
            .map(|id| match game.state.unit(*id).unwrap().order {
                oxide_sim::Order::Run { goal } => goal,
                other => panic!("unit {id} left its walk: {other:?}"),
            })
            .collect()
    };
    let draws_the_click = |game: &Game| {
        let at = game
            .presentation
            .camera
            .to_screen(vec2(clicked.x as f32 + 0.5, clicked.y as f32 + 0.5));
        for id in &group {
            let unit = game.state.unit(*id).unwrap();
            let points: Vec<_> = crumbs(game, unit)
                .into_iter()
                .map(|(index, point, _)| (index, point))
                .collect();
            assert_eq!(points, vec![(0, at)], "unit {id} marks the click");
        }
    };

    assert!(goals(&game).iter().all(oxide_sim::Goal::is_pending));
    draws_the_click(&game);

    let mut exposed = false;
    for _ in 0..600 {
        game.state.tick(&[]);
        if goals(&game).iter().all(|goal| !goal.is_pending()) {
            exposed = true;
            break;
        }
    }
    assert!(exposed, "the walk explores its click on the way");
    let mut targets: Vec<_> = goals(&game).iter().map(oxide_sim::Goal::target).collect();
    targets.sort_unstable_by_key(|tile| (tile.y, tile.x));
    targets.dedup();
    assert_eq!(targets.len(), group.len(), "each member took its own slot");
    draws_the_click(&game);
}

#[test]
fn a_landing_that_took_over_a_walk_draws_at_its_click() {
    let mut game = fog_corridor_game("condor", [(4, 4)]);
    let human = game.presentation.human;
    let condor = game.state.units()[0].id;
    game.presentation.selection.units = vec![condor];
    // A rock takes no landing, so the pad lies beside the click.
    let clicked = TilePos::new(36, 4);
    assert!(!game.state.passable(clicked), "premise: the click is rock");
    game.state.tick(&[PlayerCommand {
        player: human,
        command: Command::Run {
            units: vec![condor],
            goal: clicked,
            queue: false,
        },
    }]);
    let mut pad = None;
    for _ in 0..1_500 {
        game.state.tick(&[]);
        if let oxide_sim::Order::Land { goal, from } = game.state.unit(condor).unwrap().order {
            assert_eq!(from, Some(clicked), "the landing keeps the walk's click");
            pad = Some(goal);
            break;
        }
    }
    assert_ne!(pad.expect("the walk hands over to a landing"), clicked);
    let unit = game.state.unit(condor).unwrap();
    let points: Vec<_> = crumbs(&game, unit)
        .into_iter()
        .map(|(index, point, _)| (index, point))
        .collect();
    let at = game
        .presentation
        .camera
        .to_screen(vec2(clicked.x as f32 + 0.5, clicked.y as f32 + 0.5));
    assert_eq!(points, vec![(0, at)], "the marker stays on the click");
}

#[test]
fn an_attack_on_an_even_footprint_draws_at_its_center() {
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Even footprint",
            "players": [
                {"name": "Raider", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "Target", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "map": [
                "########################",
                "#1.....................#",
                "#......................#",
                "#......................#",
                "#......................#",
                "#......................#",
                "#......................#",
                "#...................2..#",
                "#......................#",
                "########################"
            ],
            "units": [{"player": 0, "kind": "sentinel", "x": 14, "y": 3}],
            "buildings": [{"player": 1, "kind": "fabricator", "x": 17, "y": 2}]
        })
        .to_string(),
    )
    .expect("even footprint parses");
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    let human = game.presentation.human;
    let raider = game
        .state
        .units()
        .iter()
        .find(|u| u.player == human)
        .unwrap()
        .id;
    let fabricator = game
        .state
        .buildings_at(TilePos::new(17, 2))
        .next()
        .unwrap()
        .id;
    game.presentation.selection.units = vec![raider];
    game.state.tick(&[PlayerCommand {
        player: human,
        command: Command::Attack {
            units: vec![raider],
            target: oxide_sim::Target::Building(fabricator).into(),
            queue: false,
        },
    }]);
    let unit = game.state.unit(raider).unwrap();
    assert!(matches!(unit.order, oxide_sim::Order::Attack { .. }));
    let points: Vec<_> = crumbs(&game, unit)
        .into_iter()
        .map(|(index, point, _)| (index, point))
        .collect();
    let center = game.presentation.camera.to_screen(vec2(18.0, 3.0));
    assert_eq!(
        points,
        vec![(0, center)],
        "the marker sits where the footprint's four tiles meet"
    );
}

#[test]
fn work_on_an_even_footprint_draws_at_its_center() {
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Even footprint work",
            "players": [
                {"name": "Builder", "faction": "ferrous", "scrap": 1000, "bot": false},
                {"name": "Rival", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "map": [
                "########################",
                "#1.....................#",
                "#......................#",
                "#......................#",
                "#......................#",
                "#......................#",
                "#......................#",
                "#...................2..#",
                "#......................#",
                "########################"
            ],
            "units": [{"player": 0, "kind": "harvester", "x": 3, "y": 4}],
            "buildings": [
                {"player": 0, "kind": "fabricator", "x": 14, "y": 2},
                {"player": 1, "kind": "fabricator", "x": 19, "y": 5}
            ]
        })
        .to_string(),
    )
    .expect("even footprint parses");
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    let human = game.presentation.human;
    let worker = game
        .state
        .units()
        .iter()
        .find(|u| u.player == human)
        .unwrap()
        .id;
    let salvaged = game
        .state
        .buildings_at(TilePos::new(14, 2))
        .next()
        .unwrap()
        .id;
    let kind = oxide_sim::BuildingKind::Fabricator;
    let build = |anchor, queue, defer| PlayerCommand {
        player: human,
        command: Command::Build {
            units: vec![worker],
            kind,
            anchor,
            queue,
            defer,
        },
    };
    game.presentation.selection.units = vec![worker];
    // Staged, so the provisional claim is still a Found order and the
    // queued site exists only in the projection.
    game.pending.extend([
        build(TilePos::new(6, 5), false, true),
        build(TilePos::new(10, 5), true, false),
        PlayerCommand {
            player: human,
            command: Command::Salvage {
                units: vec![worker],
                building: salvaged,
                queue: true,
            },
        },
    ]);
    let view = game.view();
    let projection = view.projection();
    let orders = &projection.program(worker).unwrap().orders;
    assert!(
        matches!(
            orders[..],
            [
                oxide_sim::Order::Found { .. },
                oxide_sim::Order::Build { .. },
                oxide_sim::Order::Salvage { building },
            ] if building == salvaged
        ),
        "premise: {orders:?}"
    );
    let points: Vec<_> = crate::render::entities::breadcrumb_points(
        &view,
        &projection,
        game.state.unit(worker).unwrap(),
    )
    .into_iter()
    .map(|(index, point, _)| (index, point))
    .collect();
    let at = |x, y| game.presentation.camera.to_screen(vec2(x, y));
    assert_eq!(
        points,
        vec![(0, at(7.0, 6.0)), (1, at(11.0, 6.0)), (2, at(15.0, 3.0))],
        "each marker sits where the footprint's four tiles meet"
    );
}

#[test]
fn the_docks_subject_always_draws_its_trail() {
    // Twelve older harvesters ahead of thirteen newer sentinels: the
    // majority-kind subject sits past the decor cap in raw selection
    // order, and the cap must never drop it.
    let mut units = Vec::new();
    for i in 0..12 {
        units.push(serde_json::json!({"player": 0, "kind": "harvester", "x": 2 + i, "y": 2}));
    }
    for i in 0..13 {
        units.push(serde_json::json!({"player": 0, "kind": "sentinel", "x": 2 + i, "y": 4}));
    }
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Crowd",
            "players": [
                {"name": "Mass", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "Idle", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "map": [
                "######################",
                "#....................#",
                "#....................#",
                "#....................#",
                "#....................#",
                "#....................#",
                "#1.................2.#",
                "#....................#",
                "######################"
            ],
            "units": units
        })
        .to_string(),
    )
    .expect("crowd parses");
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("builds");
    game.presentation.selection.units = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .map(|u| u.id)
        .collect();
    let subject = crate::panel::subject_unit(&game.view()).expect("a subject");
    assert_eq!(
        game.presentation
            .selection
            .units
            .iter()
            .position(|id| *id == subject),
        Some(12),
        "premise: the subject sits exactly past the old cap's cut"
    );
    let decor = crate::render::entities::decor_units(&game.view());
    assert_eq!(decor.len(), 12, "the cap holds");
    assert_eq!(decor[0], subject, "the subject draws first, never dropped");
    assert!(
        decor
            .iter()
            .all(|id| game.presentation.selection.units.contains(id)),
        "decor only draws selected machines"
    );

    // A lone selection degrades to itself.
    let one = game.presentation.selection.units[0];
    game.presentation.selection.units = vec![one];
    assert_eq!(
        crate::render::entities::decor_units(&game.view()),
        vec![one]
    );
}

#[test]
fn the_tutorial_survives_its_own_literal_instructions() {
    // Every lesson, played as its card words it (keyboard alternatives
    // the text itself offers, world clicks for the rest), must stay
    // affordable with the shipped numbers, so a cost or bank change that
    // strands the player fails here.
    use crate::tutorial::{Tutorial, tutorial_scenario};

    let harvester_cost = UnitKind::Harvester.stats().cost;
    let turret_cost = oxide_sim::BuildingKind::Turret
        .base_stats()
        .construction
        .expect("palette structures are constructable")
        .cost;
    let sentinel_cost = UnitKind::Sentinel.stats().cost;
    let opening = tutorial_scenario().players[0].scrap;
    assert!(
        opening >= harvester_cost + turret_cost + sentinel_cost,
        "the tutorial bank ({opening}) no longer covers its literal lesson spends \
         ({harvester_cost}+{turret_cost}+{sentinel_cost}): raise tutorial_scenario's \
         scrap or cheapen a lesson"
    );

    let mut game =
        Game::with_viewport(tutorial_scenario(), vec2(1280.0, 800.0)).expect("tutorial builds");
    let mut input = InputState::new();
    let mut t = Tutorial::new();
    game.presentation.camera.center = vec2(8.0, 5.0);
    let right_click = |game: &mut Game, input: &mut InputState, world: Vec2| {
        let p = game.presentation.camera.to_screen(world);
        apply_events(game, input, &[right_down(p)]);
    };
    let bank = |game: &Game| game.state.player(game.presentation.human).scrap;

    // Select the Foundry and use the displayed production shortcut.
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 0);
    assert!(bank(&game) >= harvester_cost, "lesson 1 must be affordable");
    let home = game.home_foundry().unwrap().center();
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(home.x.to_num::<f32>(), home.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    controls_key(&mut game, &mut input, Key::Q);
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 1, "training graduates lesson 1");

    // Lesson 2 — select a harvester, right-click a scrap pile; the
    // card holds until the first load lands.
    let hauler = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .map(|u| (u.id, u.pos))
        .expect("a starting harvester");
    let p = game
        .presentation
        .camera
        .to_screen(vec2(hauler.1.x.to_num::<f32>(), hauler.1.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    assert_eq!(
        game.presentation.selection.units,
        vec![hauler.0],
        "the harvester is in hand"
    );
    right_click(&mut game, &mut input, vec2(7.5, 2.5)); // the home scrap pile
    game.do_tick();
    assert!(game.demo.harvested, "the order was accepted");
    assert!(t.advance(game.demo));
    assert_eq!(
        t.step, 1,
        "an accepted order alone must not graduate the lesson"
    );
    for _ in 0..1500 {
        if game.demo.deposited {
            break;
        }
        game.do_tick();
    }
    assert!(game.demo.deposited, "a load reaches the bank within budget");
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 2, "income graduates the mining lesson");

    // Lesson 3 — "Pick a DIFFERENT harvester": the hauler keeps its
    // program while an idle machine raises the palette's structure.
    assert!(bank(&game) >= turret_cost, "lesson 3 must be affordable");
    let idle = game
        .state
        .units()
        .iter()
        .find(|u| {
            u.player == game.presentation.human
                && u.kind == UnitKind::Harvester
                && matches!(u.order, oxide_sim::Order::Idle)
        })
        .map(|u| (u.id, u.pos))
        .expect("an idle harvester to build with");
    let p = game
        .presentation
        .camera
        .to_screen(vec2(idle.1.x.to_num::<f32>(), idle.1.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    assert_eq!(
        game.presentation.selection.units.len(),
        1,
        "one builder in hand"
    );
    let picked = game.presentation.selection.units[0];
    assert!(
        game.state
            .unit(picked)
            .is_some_and(|u| !matches!(u.order, oxide_sim::Order::Harvest { .. })),
        "the literal reading leaves the hauler hauling"
    );
    controls_key(&mut game, &mut input, Key::B);
    controls_key(&mut game, &mut input, Key::R);
    controls_key(&mut game, &mut input, Key::Q);
    let ground = game.presentation.camera.to_screen(vec2(10.5, 4.5));
    apply_events(&mut game, &mut input, &click(ground.x, ground.y));
    assert!(
        game.pending.iter().any(
            |c| matches!(&c.command, Command::Build { kind, .. } if *kind == oxide_sim::BuildingKind::Turret)
        ),
        "B, digit, ground click staged the build: {:?}",
        game.pending
    );
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 3, "the site graduates the building lesson");
    assert!(
        game.state
            .units()
            .iter()
            .any(|u| u.player == game.presentation.human
                && matches!(u.order, oxide_sim::Order::Harvest { .. })),
        "income survives the building lesson"
    );

    // Lesson 4: the fighter must be payable here.
    assert!(
        bank(&game) >= sentinel_cost,
        "the fighter lesson re-opened the dead end: bank {} vs {} needed",
        bank(&game),
        sentinel_cost
    );
    let home = game.home_foundry().unwrap().center();
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(home.x.to_num::<f32>(), home.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    controls_key(&mut game, &mut input, Key::E);
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 4, "the fighter graduates the arming lesson");

    // Lesson 5 — right-click ground with a fighter selected.
    let sentinel = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Sentinel)
        .map(|u| (u.id, u.pos))
        .expect("the starting sentinel stands");
    let p = game.presentation.camera.to_screen(vec2(
        sentinel.1.x.to_num::<f32>(),
        sentinel.1.y.to_num::<f32>(),
    ));
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    right_click(&mut game, &mut input, vec2(12.5, 9.5));
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 5, "advance graduates the march lesson");

    // Lesson 6 is the pause menu, a frame-loop act outside the command
    // stream; its flag flips in `app::screen_flow`.
    game.demo.paused_menu = true;
    assert!(!t.advance(game.demo), "school is out");
}

/// Publishes the live panel's card whose action `pick` accepts at one
/// fixed rect and taps it, the way a finger meets the drawn panel.
fn tap_panel_card(
    game: &mut Game,
    input: &mut InputState,
    pick: impl Fn(&crate::panel::Card) -> bool,
) {
    let panel = crate::panel::build_for_input(&game.view(), &classic(), input).expect("a panel");
    let card = panel
        .cards
        .iter()
        .find(|card| pick(card))
        .unwrap_or_else(|| {
            let titles: Vec<_> = panel.cards.iter().map(|card| &card.title).collect();
            panic!("no such card among {titles:?}")
        });
    let rect = macroquad::math::Rect::new(300.0, 700.0, 60.0, 60.0);
    let mut layout = bare_layout(680.0, 500.0);
    layout.cards[0] = (rect, card.action);
    layout.card_count = 1;
    game.presentation.layout.set(layout);
    tap(game, input, rect.center());
}

fn tap_world(game: &mut Game, input: &mut InputState, world: Vec2) {
    let p = game.presentation.camera.to_screen(world);
    tap(game, input, p);
}

fn long_press_world(game: &mut Game, input: &mut InputState, world: Vec2) {
    let p = game.presentation.camera.to_screen(world);
    input.now += 1.0;
    apply_events(game, input, &[touch_down(1, p)]);
    input.now += f64::from(input.touch_prefs.long_press_ms) / 1000.0 + 0.05;
    update_touch(game, input);
    apply_events(game, input, &[touch_up(1, p)]);
}

#[test]
fn the_tutorial_survives_its_own_touch_instructions() {
    // The touch twin of the literal playthrough: every lesson played
    // exactly as its touch card words it, with taps, long-presses, and
    // panel cards, and nothing a touch-only build lacks.
    use crate::tutorial::{STEPS, Tutorial, tutorial_scenario};

    let mut game =
        Game::with_viewport(tutorial_scenario(), vec2(1280.0, 800.0)).expect("tutorial builds");
    let mut input = InputState::new();
    let mut t = Tutorial::new();
    game.presentation.camera.center = vec2(8.0, 5.0);
    game.presentation.layout.set(bare_layout(680.0, 500.0));
    let own_unit = |game: &Game, kind: UnitKind, idle: bool| {
        game.state
            .units()
            .iter()
            .find(|u| {
                u.player == game.presentation.human
                    && u.kind == kind
                    && (!idle || matches!(u.order, oxide_sim::Order::Idle))
            })
            .map(|u| (u.id, vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())))
            .expect("an own machine of that kind")
    };
    let home = |game: &Game| {
        let c = game.home_foundry().unwrap().center();
        vec2(c.x.to_num::<f32>(), c.y.to_num::<f32>())
    };

    // "Tap your Foundry, then the Harvester card."
    assert!(t.advance(game.demo));
    assert!(STEPS[0].body(true)[0].contains("Harvester card"));
    let world = home(&game);
    tap_world(&mut game, &mut input, world);
    tap_panel_card(&mut game, &mut input, |card| card.title == "Harvester");
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 1, "training graduates lesson 1");

    // "Select a Harvester and long-press a scrap pile."
    let (hauler, at) = own_unit(&game, UnitKind::Harvester, false);
    tap_world(&mut game, &mut input, at);
    assert_eq!(game.presentation.selection.units, vec![hauler]);
    long_press_world(&mut game, &mut input, vec2(7.5, 2.5));
    game.do_tick();
    assert!(game.demo.harvested, "the long-press ordered the harvest");
    for _ in 0..1500 {
        if game.demo.deposited {
            break;
        }
        game.do_tick();
    }
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 2, "income graduates the mining lesson");

    // "Tap Build, then a building, then open ground."
    let (_, at) = own_unit(&game, UnitKind::Harvester, true);
    tap_world(&mut game, &mut input, at);
    tap_panel_card(&mut game, &mut input, |card| card.title == "Build");
    assert!(
        input.construction_open(),
        "the Build card opens construction"
    );
    tap_panel_card(&mut game, &mut input, |card| {
        card.action == crate::panel::CardAction::ArmBuild(oxide_sim::BuildingKind::Turret)
    });
    tap_world(&mut game, &mut input, vec2(10.5, 4.5));
    assert!(game.pending.is_empty(), "open ground only drops the ghost");
    tap_world(&mut game, &mut input, vec2(10.5, 4.5));
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            Command::Build { kind, .. } if *kind == oxide_sim::BuildingKind::Turret
        )),
        "Build, a building, open ground, and the ghost staged the site: {:?}",
        game.pending
    );
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 3, "the site graduates the building lesson");

    // "Train a Sentinel at the Foundry."
    let world = home(&game);
    tap_world(&mut game, &mut input, world);
    tap_panel_card(&mut game, &mut input, |card| card.title == "Sentinel");
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 4, "the fighter graduates the arming lesson");

    // "Long-press ground with a combat unit selected."
    let (_, at) = own_unit(&game, UnitKind::Sentinel, false);
    tap_world(&mut game, &mut input, at);
    long_press_world(&mut game, &mut input, vec2(12.5, 9.5));
    game.do_tick();
    assert!(t.advance(game.demo));
    assert_eq!(t.step, 5, "advance graduates the march lesson");

    // "Tap the menu button at the top right to open the pause menu."
    game.presentation.layout.set(top_bar_layout());
    let menu = game.presentation.layout.get().menu_button.center();
    tap(&mut game, &mut input, menu);
    assert!(input.take_menu_request(), "the menu button asks for pause");
    game.demo.paused_menu = true;
    assert!(!t.advance(game.demo), "school is out");
}

#[test]
fn a_click_on_remembered_ground_defers_and_unscouted_refuses() {
    use chassis::grid::TilePos;
    let mut game = headless_game();
    let mut input = InputState::new();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    let spot = TilePos::new(18, 4);
    // Scout the spot, then walk home so it stays explored but unseen.
    let walk = |game: &mut Game, goal: TilePos| {
        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::Run {
                units: vec![harvester],
                goal,
                queue: false,
            },
        }]);
    };
    walk(&mut game, TilePos::new(18, 5));
    for _ in 0..600 {
        if game.state.can_see(game.presentation.human, spot) {
            break;
        }
        game.state.tick(&[]);
    }
    assert!(
        game.state.can_see(game.presentation.human, spot),
        "scout reached the spot"
    );
    walk(&mut game, TilePos::new(7, 5));
    for _ in 0..600 {
        if !game.state.can_see(game.presentation.human, spot) {
            break;
        }
        game.state.tick(&[]);
    }
    assert!(!game.state.can_see(game.presentation.human, spot));
    assert!(game.state.vision(game.presentation.human).explored(spot));

    game.presentation.selection.units = vec![harvester];
    build_click(&mut game, &mut input, oxide_sim::BuildingKind::Turret, spot);
    assert_eq!(game.pending.len(), 1, "remembered ground stages the claim");
    match &game.pending[0].command {
        Command::Build { anchor, defer, .. } => {
            assert_eq!(*anchor, spot);
            assert!(*defer, "remembered ground emits the deferred mode");
        }
        other => panic!("expected a build, staged {other:?}"),
    }

    // Never-explored ground refuses outright: nothing staged, mode
    // stays armed for the next try.
    let dark = TilePos::new(30, 5);
    assert!(!game.state.vision(game.presentation.human).explored(dark));
    build_click(&mut game, &mut input, oxide_sim::BuildingKind::Turret, dark);
    assert_eq!(game.pending.len(), 1, "unscouted ground stages nothing");
    assert!(input.placing.is_some(), "the refusal keeps the mode armed");
}

#[test]
fn an_undrained_deferred_build_is_replaced_before_preflight() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let builder = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .expect("skirmish authors a harvester")
        .id;
    let kind = oxide_sim::BuildingKind::Fabricator;
    let first = TilePos::new(18, 4);
    let replacement = TilePos::new(19, 4);
    let cost = kind
        .base_stats()
        .construction
        .expect("fabricator is constructible")
        .cost;
    let scrap = game.state.player(game.presentation.human).scrap;
    assert!(
        cost <= scrap && scrap < cost.saturating_mul(2),
        "premise: the bank funds one fabricator, not both"
    );

    let walk = |game: &mut Game, goal: TilePos| {
        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::Run {
                units: vec![builder],
                goal,
                queue: false,
            },
        }]);
    };
    walk(&mut game, TilePos::new(19, 6));
    for _ in 0..600 {
        if [first, replacement].iter().all(|anchor| {
            let (w, h) = kind.size();
            (0..h).all(|dy| {
                (0..w).all(|dx| {
                    game.state
                        .can_see(game.presentation.human, anchor.offset(dx, dy))
                })
            })
        }) {
            break;
        }
        game.state.tick(&[]);
    }
    walk(&mut game, TilePos::new(7, 5));
    for _ in 0..600 {
        if [first, replacement].iter().all(|anchor| {
            let (w, h) = kind.size();
            (0..h).all(|dy| {
                (0..w).all(|dx| {
                    !game
                        .state
                        .can_see(game.presentation.human, anchor.offset(dx, dy))
                })
            })
        }) {
            break;
        }
        game.state.tick(&[]);
    }
    for anchor in [first, replacement] {
        let (w, h) = kind.size();
        for dy in 0..h {
            for dx in 0..w {
                let tile = anchor.offset(dx, dy);
                assert!(game.state.vision(game.presentation.human).explored(tile));
                assert!(!game.state.can_see(game.presentation.human, tile));
            }
        }
    }

    game.presentation.selection.units = vec![builder];
    build_click(&mut game, &mut input, kind, first);
    assert_eq!(game.pending.len(), 1, "the first deferred build staged");
    assert!(matches!(
        game.pending[0].command,
        Command::Build {
            queue: false,
            defer: true,
            ..
        }
    ));

    let queued = pending_build_projection(&game.view(), kind, replacement, true).funds;
    assert_eq!(queued.scrap, scrap - cost);
    assert_eq!(
        queued.refund, 0,
        "Shift preserves the pending claim and its reservation"
    );
    assert_eq!(
        placement_refusal(&game.view(), kind, replacement, true),
        Some(oxide_sim::PlaceRefusal::Building),
        "Shift preserves the overlapping pending footprint"
    );
    let replacing = pending_build_projection(&game.view(), kind, replacement, false).funds;
    assert_eq!(replacing.scrap, scrap - cost);
    assert_eq!(
        replacing.refund, cost,
        "a plain click replaces the pending claim before it can charge"
    );
    assert_eq!(
        placement_refusal(&game.view(), kind, replacement, false),
        None,
        "the abandoned pending footprint no longer blocks its replacement"
    );

    build_click(&mut game, &mut input, kind, replacement);
    assert_eq!(
        game.pending.len(),
        2,
        "one-build bank accepts the replacement instead of billing both claims"
    );

    let scrap = game.state.player(game.presentation.human).scrap;
    let commands = std::mem::take(&mut game.pending);
    let report = game.state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. })),
        "the sim accepts the same command sequence the shell preflight accepted"
    );
    assert_eq!(
        game.state.player(game.presentation.human).scrap,
        scrap - cost,
        "replacement refunds the first site and pays for the second"
    );
    assert!(matches!(
        game.state.unit(builder).expect("builder survives").order,
        oxide_sim::Order::Found {
            kind: ordered,
            anchor,
        } if ordered == kind && anchor == replacement
    ));

    let other_builder = game
        .state
        .units()
        .iter()
        .find(|unit| {
            unit.player == game.presentation.human
                && unit.kind == UnitKind::Harvester
                && unit.id != builder
        })
        .expect("skirmish authors another harvester")
        .id;
    game.pending.push(PlayerCommand {
        player: game.presentation.human,
        command: Command::Stop {
            units: vec![builder],
        },
    });
    game.presentation.selection.units = vec![other_builder];
    assert_eq!(
        placement_refusal(&game.view(), kind, first, false),
        None,
        "the pending Stop releases the live claim before the next command"
    );

    build_click(&mut game, &mut input, kind, first);
    assert_eq!(
        game.pending.len(),
        2,
        "a stale live claim does not make the shell refuse the valid replacement"
    );
    let commands = std::mem::take(&mut game.pending);
    let report = game.state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. }))
    );
    assert!(matches!(
        game.state
            .unit(other_builder)
            .expect("other builder survives")
            .order,
        oxide_sim::Order::Found {
            kind: ordered,
            anchor,
        } if ordered == kind && anchor == first
    ));
}

#[test]
fn pending_projection_refunds_unstarted_sites_on_replacement_or_stop() {
    let mut game = drag_arena(500);
    let builder = game.state.units()[0].id;
    let kind = oxide_sim::BuildingKind::Turret;
    let anchor = TilePos::new(4, 2);
    let cost = kind
        .base_stats()
        .construction
        .expect("turret is constructible")
        .cost;
    game.presentation.selection.units = vec![builder];
    game.pending.push(PlayerCommand {
        player: game.presentation.human,
        command: Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer: false,
        },
    });
    let committed = pending_build_projection(&game.view(), kind, anchor, false).funds;
    assert_eq!(
        committed.scrap,
        500 - cost,
        "an immediate site charges before its builder is reprogrammed"
    );
    assert_eq!(committed.refund, cost);
    assert_eq!(
        placement_refusal(&game.view(), kind, anchor, false),
        None,
        "replacement releases an unstarted site"
    );

    game.pending.clear();
    game.pending.push(PlayerCommand {
        player: game.presentation.human,
        command: Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer: true,
        },
    });
    assert_eq!(
        pending_build_projection(&game.view(), kind, anchor, true).funds,
        PendingBuildFunds {
            scrap: 500 - cost,
            refund: 0,
        }
    );
    assert_eq!(
        placement_refusal(&game.view(), kind, anchor, true),
        Some(oxide_sim::PlaceRefusal::Building)
    );
    game.pending.push(PlayerCommand {
        player: game.presentation.human,
        command: Command::Patrol {
            units: vec![builder],
            waypoints: Vec::new(),
        },
    });
    assert_eq!(
        pending_build_projection(&game.view(), kind, anchor, true).funds,
        PendingBuildFunds {
            scrap: 500 - cost,
            refund: 0,
        },
        "a rejected pending command cannot release the claim"
    );
    assert_eq!(
        placement_refusal(&game.view(), kind, anchor, true),
        Some(oxide_sim::PlaceRefusal::Building)
    );
    game.pending.pop();

    game.pending.push(PlayerCommand {
        player: game.presentation.human,
        command: Command::Stop {
            units: vec![builder],
        },
    });
    assert_eq!(
        pending_build_projection(&game.view(), kind, anchor, true).funds,
        PendingBuildFunds {
            scrap: 500,
            refund: 0,
        },
        "Stop clears the deferred promise before it can charge"
    );
    assert_eq!(
        placement_refusal(&game.view(), kind, anchor, true),
        None,
        "Stop releases the deferred footprint"
    );
}

#[test]
fn a_paid_site_does_not_reserve_its_surviving_deferred_claim_again() {
    let mut game = headless_game();
    let workers: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|unit| unit.player == game.presentation.human && unit.kind == UnitKind::Harvester)
        .map(|unit| unit.id)
        .take(2)
        .collect();
    assert_eq!(workers.len(), 2, "skirmish authors two human workers");
    let kind = oxide_sim::BuildingKind::Turret;
    let anchor = TilePos::new(10, 4);
    let cost = kind
        .base_stats()
        .construction
        .expect("turret is constructible")
        .cost;
    let scrap = game.state.player(game.presentation.human).scrap;
    game.pending.extend([
        PlayerCommand {
            player: game.presentation.human,
            command: Command::Build {
                units: vec![workers[0]],
                kind,
                anchor,
                queue: false,
                defer: true,
            },
        },
        PlayerCommand {
            player: game.presentation.human,
            command: Command::Build {
                units: vec![workers[1]],
                kind,
                anchor,
                queue: false,
                defer: false,
            },
        },
    ]);
    game.state
        .inspect_command_phase(&game.pending, |projected| {
            assert!(matches!(
                projected.unit(workers[0]).expect("worker survives").order,
                oxide_sim::Order::Found {
                    kind: ordered,
                    anchor: claimed,
                } if ordered == kind && claimed == anchor
            ));
            assert_eq!(projected.scrap(game.presentation.human), Some(scrap - cost));
        });

    game.presentation.selection.units = vec![workers[1]];
    let projection =
        pending_build_projection_for(&game.view(), kind, TilePos::new(13, 4), true, false);
    assert_eq!(
        projection.funds,
        PendingBuildFunds {
            scrap: scrap - cost,
            refund: 0,
        },
        "the projected bank is charged once and the free join reserves nothing"
    );
}

#[test]
fn a_deferred_shift_build_can_use_any_selected_worker_with_room() {
    let mut game = headless_game();
    let workers: Vec<_> = game
        .state
        .units()
        .iter()
        .filter(|unit| unit.player == game.presentation.human && unit.kind == UnitKind::Harvester)
        .map(|unit| unit.id)
        .take(2)
        .collect();
    assert_eq!(workers.len(), 2, "skirmish authors two human workers");
    let low = game.state.unit(workers[0]).expect("worker exists").tile();
    let far = (0..game.state.map().height())
        .flat_map(|y| (0..game.state.map().width()).map(move |x| TilePos::new(x, y)))
        .filter(|&tile| game.state.passable(tile))
        .max_by_key(|tile| (tile.x - low.x).abs() + (tile.y - low.y).abs())
        .expect("map has passable ground");
    let mut fill = vec![PlayerCommand {
        player: game.presentation.human,
        command: Command::Run {
            units: vec![workers[0]],
            goal: far,
            queue: false,
        },
    }];
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP {
        fill.push(PlayerCommand {
            player: game.presentation.human,
            command: Command::Run {
                units: vec![workers[0]],
                goal: far,
                queue: true,
            },
        });
    }
    game.state.tick(&fill);
    assert_eq!(
        game.state
            .unit(workers[0])
            .expect("worker survives")
            .queue
            .len(),
        oxide_sim::stats::ORDER_QUEUE_CAP,
        "the lowest-id founder starts full"
    );
    assert!(matches!(
        game.state.unit(workers[1]).expect("worker survives").order,
        oxide_sim::Order::Idle
    ));

    let kind = oxide_sim::BuildingKind::Turret;
    let low = game.state.unit(workers[0]).expect("worker survives").tile();
    let anchor = (0..game.state.map().height())
        .flat_map(|y| (0..game.state.map().width()).map(move |x| TilePos::new(x, y)))
        .filter(|&tile| game.state.can_place(game.presentation.human, kind, tile))
        .min_by_key(|tile| (tile.x - low.x).abs() + (tile.y - low.y).abs())
        .expect("visible reachable ground remains");
    game.presentation.selection.units = workers.clone();
    assert!(
        pending_build_projection_for(&game.view(), kind, anchor, true, true).queue_has_room,
        "deferred construction succeeds when any selected worker can take the claim"
    );
    assert!(
        !pending_build_projection_for(&game.view(), kind, anchor, true, false).queue_has_room,
        "immediate construction is still gated by the lowest-id founder"
    );

    let command = |defer| PlayerCommand {
        player: game.presentation.human,
        command: Command::Build {
            units: workers.clone(),
            kind,
            anchor,
            queue: true,
            defer,
        },
    };
    let mut deferred = game.state.clone();
    let report = deferred.tick(&[command(true)]);
    assert!(
        !report.events.iter().any(|event| matches!(
            event,
            oxide_sim::Event::CommandRejected {
                reason: oxide_sim::command::RejectReason::QueueFull,
                ..
            }
        )),
        "the sim accepts the deferred command through the free worker"
    );
    let mut immediate = game.state.clone();
    let report = immediate.tick(&[command(false)]);
    assert!(
        report.events.iter().any(|event| matches!(
            event,
            oxide_sim::Event::CommandRejected {
                reason: oxide_sim::command::RejectReason::QueueFull,
                ..
            }
        )),
        "the sim rejects the immediate command when its founder is full"
    );
}

#[test]
fn a_plain_placement_replaces_the_selected_claim_while_shift_preserves_it() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let builder = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .expect("skirmish authors a harvester")
        .id;
    let kind = oxide_sim::BuildingKind::Fabricator;
    let old_spot = TilePos::new(10, 4);
    let new_spot = TilePos::new(11, 4);
    for spot in [old_spot, new_spot] {
        assert!(
            game.state
                .place_intent_refusal(game.presentation.human, kind, spot)
                .is_none(),
            "premise: the visible site starts open"
        );
    }
    let report = game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::Build {
            units: vec![builder],
            kind,
            anchor: old_spot,
            queue: false,
            defer: true,
        },
    }]);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. }))
    );
    let site = game
        .state
        .buildings()
        .iter()
        .find(|b| b.anchor == old_spot)
        .unwrap();
    assert!(!site.built());
    assert_eq!(site.construction_progress(), Some(0));
    assert_eq!(
        game.state.unit(builder).unwrap().order,
        oxide_sim::Order::Build { site: site.id }
    );

    game.presentation.selection.units = vec![builder];
    input.build_menu = true;
    let affordable = |game: &Game, input: &InputState| {
        crate::panel::build_for_input(&game.view(), &classic(), input)
            .unwrap()
            .cards
            .iter()
            .find(|card| card.action == crate::panel::CardAction::ArmBuild(kind))
            .unwrap()
            .enabled
    };
    assert!(
        affordable(&game, &input),
        "replacement cards can use the unstarted site's refund"
    );
    apply_events(&mut game, &mut input, &[key_down(Key::Shift)]);
    assert!(
        !affordable(&game, &input),
        "queued construction cannot spend a retained site's refund"
    );
    apply_events(&mut game, &mut input, &[key_up(Key::Shift)]);
    input.placing = Some(kind);
    game.presentation.camera.center = vec2(new_spot.x as f32 + 0.5, new_spot.y as f32 + 0.5);
    game.presentation.camera.pan(Vec2::ZERO);
    let p = game
        .presentation
        .camera
        .to_screen(vec2(new_spot.x as f32 + 0.5, new_spot.y as f32 + 0.5));

    assert_eq!(
        placement_refusal(&game.view(), kind, new_spot, true),
        Some(oxide_sim::PlaceRefusal::Building),
        "Shift appends, so the live claim remains a blocker"
    );
    apply_events(
        &mut game,
        &mut input,
        &[
            key_down(Key::Shift),
            left_down(p),
            left_up(p),
            key_up(Key::Shift),
        ],
    );
    assert!(
        game.pending.is_empty(),
        "the conservative Shift preflight stages no duplicate claim"
    );
    assert!(input.placing.is_some(), "the refused click stays armed");

    assert_eq!(
        placement_refusal(&game.view(), kind, new_spot, false),
        None,
        "a plain click abandons the selected founder's old claim"
    );
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    assert_eq!(
        game.pending.len(),
        1,
        "claim, overlap, and scrap preflights all account for replacement"
    );
    assert!(matches!(
        game.pending[0].command,
        Command::Build {
            anchor,
            queue: false,
            ..
        } if anchor == new_spot
    ));
}

#[test]
fn the_upgrade_card_stages_only_the_building() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario
        .units
        .retain(|unit| unit.player != 0 || unit.kind != UnitKind::Harvester);
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Turret,
        x: 9,
        y: 3,
    });
    scenario.players[0].scrap = 1000;
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Fabricator,
        x: 12,
        y: 3,
    });
    let mut game =
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("upgrade fixture builds");
    let mut input = InputState::new();
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::BuildingKind::Turret)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![turret];
    activate_card(&mut game, &mut input, crate::panel::CardAction::Upgrade);
    assert_eq!(game.pending.len(), 1, "one upgrade command staged");
    assert!(matches!(
        game.pending[0].command,
        oxide_sim::Command::UpgradeBuilding { building } if building == turret
    ));
}

#[test]
fn an_automatic_upgrade_is_not_a_worker_target_or_a_scrappable_site() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[0].scrap = 500;
    scenario.buildings.extend([
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: oxide_sim::BuildingKind::Fabricator,
            x: 9,
            y: 3,
        },
        oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: oxide_sim::BuildingKind::Turret,
            x: 12,
            y: 3,
        },
    ]);
    let mut game =
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("upgrade fixture builds");
    let mut input = InputState::new();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player == game.presentation.human && unit.kind == UnitKind::Harvester)
        .expect("fixture has a human harvester")
        .id;
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == oxide_sim::BuildingKind::Turret)
        .expect("fixture has a turret")
        .id;
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: game.presentation.human,
        command: oxide_sim::Command::UpgradeBuilding { building: turret },
    }]);
    let center = game.state.building(turret).expect("upgrade lives").center();
    assert_eq!(
        (
            game.state.building(turret).unwrap().built(),
            game.state.building(turret).unwrap().tier,
        ),
        (false, 1),
        "premise: the turret is rebuilding"
    );

    game.presentation.selection.units = vec![harvester];
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &[right_down(screen)]);
    assert!(
        game.pending.is_empty(),
        "right-click must not draft a worker"
    );
    assert!(
        game.presentation
            .toasts
            .iter()
            .any(|toast| toast.text == "Upgrade runs automatically")
    );

    game.presentation.toasts.clear();
    game.presentation.selection.units.clear();
    game.presentation.selection.buildings = vec![turret];
    super::dispatch::dispatch_action(&mut game, &mut input, &classic(), Action::StopOrScrap);
    assert!(
        game.pending.is_empty(),
        "the scrap hotkey must not stage Cancel"
    );
    assert_eq!(game.presentation.selection.buildings, vec![turret]);
    assert!(
        game.presentation
            .toasts
            .iter()
            .any(|toast| toast.text == "Upgrades cannot be cancelled")
    );
}

#[test]
fn construction_menu_shows_every_building_and_shortcuts_arm_the_visible_card() {
    use crate::action::{BUILD_CATEGORIES, building_category};
    let mut game = headless_game();
    let mut input = InputState::new();
    let keys = [Key::Q, Key::E, Key::R, Key::T];
    dispatch_action(&mut game, &mut input, Action::ToggleBuildPalette);
    let panel = crate::panel::build_for_input(&game.view(), &classic(), &input).unwrap();
    assert_eq!(panel.cards.len(), 14);
    let (back, buildings) = panel.cards.split_last().expect("cards");
    assert_eq!(back.action, crate::panel::CardAction::ClosePalette);
    for card in buildings {
        let crate::panel::CardAction::ArmBuild(kind) = card.action else {
            panic!("build card");
        };
        input.close_construction();
        controls_key(&mut game, &mut input, Key::B);
        let category = building_category(kind) as usize;
        let index = BUILD_CATEGORIES[category]
            .1
            .iter()
            .position(|k| *k == kind)
            .unwrap();
        controls_key(&mut game, &mut input, keys[category]);
        assert_eq!(input.build_category, Some(category.fit::<u8>()));
        // Shift remains queue semantics, never a different building.
        apply_events(&mut game, &mut input, &[key_down(Key::Shift)]);
        controls_key(&mut game, &mut input, keys[index]);
        apply_events(&mut game, &mut input, &[key_up(Key::Shift)]);
        assert_eq!(
            input.placing,
            card.enabled.then_some(kind),
            "{}",
            card.title
        );
        assert!(game.pending.is_empty(), "arming never spends scrap");
    }
    input.close_construction();
    controls_key(&mut game, &mut input, Key::B);
    controls_key(&mut game, &mut input, Key::B);
    assert!(!input.construction_open());
}

#[test]
fn a_tap_on_open_ground_closes_the_palette_and_keeps_the_builder() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let (_, at) = own_fighter(&game);
    dispatch_action(&mut game, &mut input, Action::ToggleBuildPalette);
    let builders = game.presentation.selection.units.clone();
    assert!(input.build_menu && !builders.is_empty());
    tap_world(&mut game, &mut input, at + vec2(4.0, 2.0));
    assert!(!input.construction_open(), "the tap dismissed the palette");
    assert_eq!(
        game.presentation.selection.units, builders,
        "and kept the crew"
    );
    assert!(game.pending.is_empty());

    // The dismissing tap never counts toward a double tap.
    assert_eq!(input.last_tap, None);
    tap_world(&mut game, &mut input, at + vec2(4.0, 2.0));
    assert!(
        game.presentation.selection.units.is_empty(),
        "with the palette closed, the next ground tap deselects"
    );

    // A tap that lands on a unit still selects it.
    dispatch_action(&mut game, &mut input, Action::ToggleBuildPalette);
    let (fighter, _) = own_fighter(&game);
    let fighter_at = {
        let u = game.state.unit(fighter).expect("fighter");
        vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())
    };
    tap_world(&mut game, &mut input, fighter_at);
    assert_eq!(game.presentation.selection.units, vec![fighter]);
}

#[test]
fn the_back_card_closes_the_palette_and_keeps_the_builder() {
    let mut game = headless_game();
    let mut input = InputState::new();
    dispatch_action(&mut game, &mut input, Action::ToggleBuildPalette);
    assert!(input.construction_open());
    let builders = game.presentation.selection.units.clone();
    assert!(!builders.is_empty(), "opening the palette picked a builder");
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::ArmBuild(oxide_sim::BuildingKind::Turret),
    );
    assert!(input.placing.is_some(), "a building is armed");
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::ClosePalette,
    );
    assert!(!input.construction_open(), "one press leaves it outright");
    assert_eq!(game.presentation.selection.units, builders);
    assert!(game.pending.is_empty());
}

#[test]
fn mouse_and_touch_switch_construction_without_cancelling_or_placing_in_the_world() {
    use crate::panel::CardAction;
    use macroquad::math::Rect;
    for touch in [false, true] {
        let mut game = headless_game();
        let mut input = InputState::new();
        dispatch_action(&mut game, &mut input, Action::ToggleBuildPalette);
        activate_card(
            &mut game,
            &mut input,
            CardAction::ArmBuild(oxide_sim::BuildingKind::Turret),
        );
        let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
        let mut layout = game.presentation.layout.get();
        layout.panel_top = 680.0;
        layout.panel_right = 600.0;
        layout.panel_regions[0] = Rect::new(0.0, 680.0, 600.0, 120.0);
        layout.cards[0] = (
            Rect::new(300.0, 700.0, 100.0, 90.0),
            CardAction::ArmBuild(oxide_sim::BuildingKind::Reclaimer),
        );
        layout.cards[1] = (Rect::new(410.0, 700.0, 100.0, 90.0), CardAction::None);
        layout.card_count = 2;
        layout.minimap = zero;
        game.presentation.layout.set(layout);
        let hash = game.state.hash();
        for x in [340.0, 450.0] {
            let events = if touch {
                vec![touch_down(1, vec2(x, 740.0)), touch_up(1, vec2(x, 740.0))]
            } else {
                click(x, 740.0).to_vec()
            };
            apply_events(&mut game, &mut input, &events);
            assert_eq!(input.placing, Some(oxide_sim::BuildingKind::Reclaimer));
            assert!(input.construction_open());
            assert!(input.placing_stroke.is_none());
            assert!(game.pending.is_empty());
            assert_eq!(game.state.hash(), hash);
        }
    }
}

fn knowledge_attack_game() -> Game {
    use oxide_sim::scenario::{BuildingSpec, UnitSpec};
    let mut scenario = oxide_sim::Scenario::skirmish();
    let mut rows = vec![vec!['.'; 40]; 30];
    rows[1][1] = '1';
    rows[27][37] = '2';
    scenario.map = rows.into_iter().map(|r| r.into_iter().collect()).collect();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Avalanche,
            x: 2,
            y: 7,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 12,
            y: 14,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Gnat,
            x: 14,
            y: 8,
        },
    ];
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: oxide_sim::BuildingKind::Array,
            x: 5,
            y: 16,
        },
        BuildingSpec {
            player: 0,
            kind: oxide_sim::BuildingKind::Bastion,
            x: 5,
            y: 6,
        },
        // Radar reports only the footprint tile nearest the Array, (16, 16);
        // the rest of the remembered footprint has no contact above it.
        BuildingSpec {
            player: 1,
            kind: oxide_sim::BuildingKind::Fabricator,
            x: 16,
            y: 16,
        },
    ];
    for (seat, player) in scenario.players.iter_mut().enumerate() {
        player.bot = seat != 0;
        player.bot_config = None;
    }
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap()
}

#[test]
fn right_click_uses_building_memory_and_anonymous_contacts_and_stop_clears_focus() {
    let mut game = knowledge_attack_game();
    let gun = game.state.units()[0].id;
    let scout = game.state.units()[1].id;
    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::Run {
            units: vec![scout],
            goal: TilePos::new(2, 3),
            queue: false,
        },
    }]);
    for _ in 0..200 {
        game.state.tick(&[]);
    }
    assert!(!game.my_vision().visible(TilePos::new(17, 17)));
    game.presentation.selection.units = vec![gun];
    let screen = game.presentation.camera.to_screen(vec2(17.5, 17.5));
    context_order(&mut game, screen, false);
    assert!(matches!(
        game.pending.last().unwrap().command,
        Command::Attack {
            target: oxide_sim::AttackTarget::RememberedBuilding(_),
            ..
        }
    ));
    game.pending.clear();
    let track = game
        .my_vision()
        .tracks()
        .iter()
        .find(|t| t.visible_unit.is_none())
        .unwrap();
    let contact = track.id;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(track.tile.x as f32 + 0.5, track.tile.y as f32 + 0.5));
    context_order(&mut game, screen, false);
    assert!(
        matches!(game.pending.last().unwrap().command, Command::Attack {
        target: oxide_sim::AttackTarget::Contact(id), ..
    } if id == contact)
    );
    game.pending.clear();
    game.presentation.selection.units.clear();
    let defense = game
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::BuildingKind::Bastion)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![defense];
    context_order(&mut game, screen, false);
    assert!(
        matches!(game.pending.last().unwrap().command, Command::FocusFire {
        target: oxide_sim::AttackTarget::Contact(id), ..
    } if id == contact)
    );
    let commands = std::mem::take(&mut game.pending);
    game.state.tick(&commands);
    assert!(game.state.building(defense).unwrap().focus.is_some());
    super::dispatch::dispatch_action(
        &mut game,
        &mut InputState::new(),
        &classic(),
        Action::StopOrScrap,
    );
    assert!(matches!(
        game.pending.last().unwrap().command,
        Command::ClearFocus { .. }
    ));
    let commands = std::mem::take(&mut game.pending);
    game.state.tick(&commands);
    assert!(game.state.building(defense).unwrap().focus.is_none());
}

#[test]
fn radar_contact_above_a_building_ghost_wins_for_units_and_defenses() {
    let mut scenario = knowledge_attack_game().scenario.clone();
    scenario.units[0].kind = UnitKind::Talon;
    scenario.units[2].x = 16;
    scenario.units[2].y = 16;
    scenario.buildings[1].kind = oxide_sim::BuildingKind::FlakTurret;
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let gun = game.state.units()[0].id;
    let scout = game.state.units()[1].id;
    let enemy = game.state.units()[2].id;
    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::Run {
            units: vec![scout],
            goal: TilePos::new(2, 3),
            queue: false,
        },
    }]);
    for _ in 0..200 {
        game.state.tick(&[]);
    }
    let tile = game.state.unit(enemy).unwrap().tile();
    assert!(!game.my_vision().visible(tile));
    assert!(
        game.my_vision()
            .ghosts()
            .iter()
            .any(|ghost| ghost.anchor == tile)
    );
    let contact = game
        .my_vision()
        .tracks()
        .iter()
        .find(|track| track.tile == tile && track.visible_unit.is_none())
        .unwrap()
        .id;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5));
    game.presentation.selection.units = vec![gun];
    context_order(&mut game, screen, false);
    assert!(matches!(game.pending.last().unwrap().command,
        Command::Attack { target: oxide_sim::AttackTarget::Contact(id), .. } if id == contact));
    game.pending.clear();
    game.presentation.selection.units.clear();
    game.presentation.selection.buildings = vec![
        game.state
            .buildings()
            .iter()
            .find(|building| building.kind == oxide_sim::BuildingKind::FlakTurret)
            .unwrap()
            .id,
    ];
    context_order(&mut game, screen, false);
    assert!(matches!(game.pending.last().unwrap().command,
        Command::FocusFire { target: oxide_sim::AttackTarget::Contact(id), .. } if id == contact));
}

#[test]
fn a_hidden_mine_does_not_change_placement_selection_or_resume_input() {
    use oxide_sim::BuildingKind;
    let anchor = TilePos::new(12, 4);
    for mined in [false, true] {
        let scenario=oxide_sim::Scenario::from_json(&serde_json::json!({
            "name":"Mine placement","players":[{"name":"Builder","faction":"ferrous","scrap":800,"bot":false},{"name":"Mines","faction":"cupric","scrap":0,"bot":true}],
            "map":["########################","#1.....................#","#......................#","#......................#","#......................#","#......................#","#......................#","#...................2..#","#......................#","########################"],
            "units":[{"player":0,"kind":"harvester","x":4,"y":4},{"player":0,"kind":"harvester","x":10,"y":2}],
            "buildings":if mined {vec![serde_json::json!({"player":1,"kind":"scuttle_charge","x":12,"y":4})]}else{vec![]}
        }).to_string()).unwrap();
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        let mut input = InputState::new();
        let worker = game.state.units()[0].id;
        game.presentation.selection.units = vec![worker];
        assert_eq!(
            placement_refusal(&game.view(), BuildingKind::Barricade, anchor, false),
            None
        );
        build_click(&mut game, &mut input, BuildingKind::Barricade, anchor);
        assert_eq!(game.pending.len(), 1);
        assert!(matches!(
            game.pending[0].command,
            Command::Build { defer: false, .. }
        ));
        let report = game.state.tick(&std::mem::take(&mut game.pending));
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
        );
        let site = game
            .state
            .buildings_at(anchor)
            .find(|b| b.player == game.presentation.human)
            .unwrap()
            .id;
        assert_eq!(game.state.player(game.presentation.human).scrap, 760);
        assert_eq!(
            placement_refusal(&game.view(), BuildingKind::Barricade, anchor, false),
            None
        );
        input.placing = None;
        let point = game.presentation.camera.to_screen(vec2(12.5, 4.5));
        apply_events(&mut game, &mut input, &click(point.x, point.y));
        assert_eq!(game.presentation.selection.buildings, vec![site]);
        game.presentation.selection.buildings.clear();
        game.presentation.selection.units = vec![worker];
        apply_events(&mut game, &mut input, &[right_down(point)]);
        assert!(
            matches!(game.pending.last().unwrap().command,Command::Build{anchor:a,..} if a==anchor)
        );
    }
}

#[test]
fn selecting_an_unfinished_mine_does_not_reveal_its_condition_after_concealment() {
    use oxide_sim::BuildingKind;
    let scenario=oxide_sim::Scenario::from_json(&serde_json::json!({
        "name":"Mine visibility","players":[{"name":"Observer","faction":"ferrous","scrap":800,"bot":false},{"name":"Mines","faction":"cupric","scrap":800,"bot":true}],
        "map":["########################","#1.....................#","#......................#","#......................#","#......................#","#......................#","#......................#","#...................2..#","#......................#","########################"],
        "units":[{"player":0,"kind":"harvester","x":10,"y":2},{"player":1,"kind":"harvester","x":13,"y":4}],
        "buildings":[{"player":1,"kind":"fabricator","x":17,"y":2}]
    }).to_string()).unwrap();
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let mut input = InputState::new();
    game.pending.push(PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: Command::Build {
            units: vec![game.state.units()[1].id],
            kind: BuildingKind::ScuttleCharge,
            anchor: TilePos::new(12, 4),
            queue: false,
            defer: false,
        },
    });
    game.do_tick();
    let mine = game
        .state
        .buildings_at(TilePos::new(12, 4))
        .next()
        .unwrap()
        .id;
    game.presentation.camera.center = vec2(12.5, 4.5);
    game.presentation.camera.pan(Vec2::ZERO);
    let point = game.presentation.camera.to_screen(vec2(12.5, 4.5));
    apply_events(&mut game, &mut input, &click(point.x, point.y));
    assert_eq!(game.presentation.selection.buildings, vec![mine]);
    for _ in 0..200 {
        if game.state.building(mine).unwrap().built() {
            break;
        }
        game.do_tick();
    }
    assert!(game.state.building(mine).unwrap().built());
    assert!(game.presentation.selection.buildings.is_empty());
    assert!(
        game.my_vision()
            .ghosts()
            .iter()
            .any(|g| g.anchor == TilePos::new(12, 4))
    );
    apply_events(&mut game, &mut input, &click(point.x, point.y));
    assert!(game.presentation.selection.buildings.is_empty());
}

fn controls_key(game: &mut Game, input: &mut InputState, key: Key) {
    controls_key_with(game, input, &classic(), key);
}

fn controls_key_with(game: &mut Game, input: &mut InputState, bindings: &BindingMap, key: Key) {
    apply_events_with(game, input, bindings, &[key_down(key), key_up(key)]);
}

#[test]
fn group_recall_from_production_or_construction_never_purchases_anything() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let worker = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![worker];
    apply_events(&mut game, &mut input, &[key_down(Key::Ctrl)]);
    controls_key(&mut game, &mut input, Key::Num1);
    apply_events(&mut game, &mut input, &[key_up(Key::Ctrl)]);
    game.presentation.selection.units.clear();
    game.presentation.selection.buildings = vec![game.home_foundry().unwrap().id];
    controls_key(&mut game, &mut input, Key::Num1);
    assert_eq!(game.presentation.selection.units, vec![worker]);
    assert!(game.presentation.selection.buildings.is_empty());
    assert!(game.pending.is_empty());
    controls_key(&mut game, &mut input, Key::B);
    controls_key(&mut game, &mut input, Key::R);
    controls_key(&mut game, &mut input, Key::Q);
    assert!(input.placing.is_some());
    controls_key(&mut game, &mut input, Key::Num1);
    assert!(!input.construction_open());
    assert!(input.armed_mode().is_none());
    assert!(game.pending.is_empty());
    game.presentation.selection.units.clear();
    controls_key(&mut game, &mut input, Key::S);
    controls_key(&mut game, &mut input, Key::H);
    controls_key(&mut game, &mut input, Key::Q);
    assert!(game.pending.is_empty(), "no hidden home production aliases");
}

#[test]
fn remapped_construction_sequence_arms_every_enabled_card_without_shift_changing_it() {
    use crate::action::{Action, BUILD_CATEGORIES, BindingMap, Chord};
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[0].scrap = 10000;
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let mut input = InputState::new();
    let mut bindings = BindingMap::classic();
    for (category, (_, kinds)) in BUILD_CATEGORIES.into_iter().enumerate() {
        assert!(bindings.rebind(
            Action::BuildCategory(category.fit::<u8>()),
            Chord::ctrl([Key::J, Key::K, Key::L, Key::O][category])
        ));
        for kind in kinds {
            let action = Action::Build(*kind);
            let old = bindings.chord_for(action).unwrap();
            assert!(bindings.rebind(action, Chord::ctrl(old.key)));
            input.close_construction();
            controls_key_with(&mut game, &mut input, &bindings, Key::B);
            apply_events_with(
                &mut game,
                &mut input,
                &bindings,
                &[key_down(Key::Ctrl), key_down(Key::Shift)],
            );
            controls_key_with(
                &mut game,
                &mut input,
                &bindings,
                [Key::J, Key::K, Key::L, Key::O][category],
            );
            let panel = crate::panel::build_for_input(&game.view(), &bindings, &input).unwrap();
            let card = panel
                .cards
                .iter()
                .find(|c| c.action == crate::panel::CardAction::ArmBuild(*kind))
                .unwrap();
            assert!(card.enabled, "{}: {:?}", card.title, card.why);
            assert!(card.hotkey.contains("Ctrl+"));
            controls_key_with(&mut game, &mut input, &bindings, old.key);
            assert_eq!(input.placing, Some(*kind));
            apply_events_with(
                &mut game,
                &mut input,
                &bindings,
                &[key_up(Key::Ctrl), key_up(Key::Shift)],
            );
        }
    }
}

#[test]
fn upgrade_and_rally_shortcuts_share_the_cards_owner_and_affordability_gates() {
    use crate::action::{BindingMap, Chord};
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[0].scrap = 10000;
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Turret,
        x: 9,
        y: 3,
    });
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Fabricator,
        x: 12,
        y: 3,
    });
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let mut input = InputState::new();
    let mut bindings = BindingMap::classic();
    assert!(bindings.rebind(Action::Upgrade, Chord::bare(Key::I)));
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::BuildingKind::Turret)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![turret];
    controls_key_with(&mut game, &mut input, &bindings, Key::I);
    assert!(
        matches!(game.pending.last().unwrap().command, Command::UpgradeBuilding { building } if building == turret)
    );
    game.pending.clear();
    let enemy = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player != game.presentation.human)
        .unwrap()
        .id;
    game.presentation.selection.buildings = vec![enemy];
    controls_key_with(&mut game, &mut input, &bindings, Key::I);
    assert!(game.pending.is_empty());
    let foundry = game.home_foundry().unwrap().id;
    game.presentation.selection.buildings = vec![foundry];
    controls_key_with(&mut game, &mut input, &bindings, Key::Y);
    assert_eq!(input.rallying, vec![foundry]);
}

#[test]
fn remapped_clear_rally_is_disabled_until_a_selected_producer_has_a_rally() {
    use crate::action::Chord;
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    game.presentation.selection.buildings = game
        .state
        .buildings()
        .iter()
        .filter(|b| b.player == game.presentation.human)
        .map(|b| b.id)
        .collect();
    let producers = game.presentation.selection.buildings.clone();
    let mut input = InputState::new();
    let mut bindings = crate::action::BindingMap::classic();
    assert!(bindings.rebind(Action::ClearRally, Chord::bare(Key::I)));
    controls_key_with(&mut game, &mut input, &bindings, Key::I);
    assert!(game.pending.is_empty());

    game.state.tick(&[PlayerCommand {
        player: game.presentation.human,
        command: Command::SetRally {
            building: producers[0],
            rally: Some(TilePos::new(14, 9)),
        },
    }]);
    controls_key_with(&mut game, &mut input, &bindings, Key::I);
    let expected: Vec<_> = producers
        .iter()
        .map(|id| PlayerCommand {
            player: game.presentation.human,
            command: Command::SetRally {
                building: *id,
                rally: None,
            },
        })
        .collect();
    assert_eq!(*game.pending, expected);
    let commands = std::mem::take(&mut game.pending);
    game.state.tick(&commands);
    controls_key_with(&mut game, &mut input, &bindings, Key::I);
    assert!(game.pending.is_empty());
    assert!(
        producers
            .iter()
            .all(|id| game.state.building(*id).unwrap().rally.is_none())
    );
}

#[test]
fn grouped_production_clicks_and_shortcuts_stage_the_same_batch() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[0].scrap = 125;
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: oxide_sim::BuildingKind::Foundry,
        x: 9,
        y: 3,
    });
    let make_game = || {
        let mut game = Game::with_viewport(scenario.clone(), vec2(1280.0, 800.0)).unwrap();
        game.presentation.selection.buildings = game
            .state
            .buildings()
            .iter()
            .filter(|b| b.player == game.presentation.human)
            .map(|b| b.id)
            .collect();
        game
    };
    let mut mouse_game = make_game();
    let mut key_game = make_game();
    let mut mouse_input = InputState::new();
    let mut key_input = InputState::new();
    let card = crate::panel::build_for_input(&mouse_game.view(), &classic(), &mouse_input)
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.action == crate::panel::CardAction::Dispatch(Action::TrainSlot(0)))
        .unwrap();
    let mut layout = mouse_game.presentation.layout.get();
    layout.cards[0] = (mq::Rect::new(240.0, 720.0, 100.0, 48.0), card.action);
    layout.card_count = 1;
    mouse_game.presentation.layout.set(layout);
    for _ in 0..2 {
        apply_events(&mut mouse_game, &mut mouse_input, &click(260.0, 740.0));
        apply_events(
            &mut key_game,
            &mut key_input,
            &[key_down(Key::Q), key_up(Key::Q)],
        );
    }
    assert_eq!(
        mouse_game.pending.len(),
        2,
        "two factories share 100 of the 125 scrap"
    );
    assert_eq!(*mouse_game.pending, *key_game.pending);
    mouse_game.do_tick();
    key_game.do_tick();
    assert_eq!(mouse_game.state.hash(), key_game.state.hash());
}

#[test]
fn grouped_upgrade_mouse_touch_and_remapped_keys_share_pending_eligibility() {
    use crate::action::Chord;
    use crate::building_actions::tests::fixture;
    use crate::panel::CardAction;
    let mut outcomes = Vec::new();
    for mode in 0..4 {
        let mut game = fixture(oxide_sim::BuildingKind::Turret, &[0, 1, 2], 450);
        let ids = game.presentation.selection.buildings.clone();
        let mut input = InputState::new();
        let mut bindings = BindingMap::classic();
        if mode == 3 {
            assert!(bindings.rebind(Action::Upgrade, Chord::bare(Key::I)));
        }
        let panel = crate::panel::build_for_input(&game.view(), &bindings, &input).unwrap();
        let card = panel
            .cards
            .iter()
            .find(|c| c.action == CardAction::Upgrade)
            .unwrap();
        assert!(card.enabled);
        assert_eq!(card.cost, Some(450));
        assert_eq!(card.title, "Upgrade 2/3");
        let mut layout = game.presentation.layout.get();
        layout.cards[0] = (mq::Rect::new(240.0, 720.0, 120.0, 48.0), card.action);
        layout.card_count = 1;
        game.presentation.layout.set(layout);
        // Deliberately retain the old hit-test card between activations.
        for _ in 0..2 {
            match mode {
                0 => apply_events_with(&mut game, &mut input, &bindings, &click(260.0, 740.0)),
                1 => apply_events_with(
                    &mut game,
                    &mut input,
                    &bindings,
                    &[
                        touch_down(1, vec2(260.0, 740.0)),
                        touch_up(1, vec2(260.0, 740.0)),
                    ],
                ),
                _ => controls_key_with(
                    &mut game,
                    &mut input,
                    &bindings,
                    if mode == 2 { Key::U } else { Key::I },
                ),
            }
        }
        assert_eq!(game.pending.len(), 2, "mode {mode}");
        assert_eq!(
            *game.pending,
            ids[..2]
                .iter()
                .map(|&building| PlayerCommand {
                    player: game.presentation.human,
                    command: Command::UpgradeBuilding { building },
                })
                .collect::<Vec<_>>()
        );
        let report = game.do_tick();
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
        );
        outcomes.push(game.state.hash());
    }
    assert!(outcomes.iter().all(|hash| *hash == outcomes[0]));
}

#[test]
fn grouped_focus_and_stop_skip_an_upgrade_staged_before_the_target_click() {
    use crate::building_actions::tests::fixture;
    let mut game = fixture(oxide_sim::BuildingKind::Turret, &[0, 1, 2], 1000);
    let ids = game.presentation.selection.buildings.clone();
    let target = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player != game.presentation.human)
        .unwrap()
        .id;
    // Place the enemy Foundry inside the selected defenses' shared sight.
    let mut json = serde_json::to_value(&*game.state).unwrap();
    let enemy = json["buildings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|b| b["id"] == serde_json::json!(target))
        .unwrap();
    enemy["anchor"] = serde_json::to_value(TilePos::new(11, 2)).unwrap();
    *game.state = serde_json::from_value(json).unwrap();
    game.state.tick(&[]);
    game.state.validate_invariants().unwrap();
    let at = game.state.building(target).unwrap().center();
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(at.x.to_num(), at.y.to_num()));
    game.issue(Command::UpgradeBuilding { building: ids[0] });
    context_order(&mut game, screen, false);
    assert!(
        matches!(&game.pending.last().unwrap().command, Command::FocusFire { buildings, .. } if buildings == &ids[1..])
    );
    let mut input = InputState::new();
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::Dispatch(Action::StopOrScrap),
    );
    assert!(
        matches!(&game.pending.last().unwrap().command, Command::ClearFocus { buildings } if buildings == &ids[1..])
    );
    assert!(
        !game
            .do_tick()
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
    );
    assert!(
        ids.iter()
            .all(|id| game.state.building(*id).unwrap().focus.is_none())
    );
}

#[test]
fn return_cargo_card_and_shortcut_replace_work_for_both_workers() {
    for kind in [UnitKind::Harvester, UnitKind::Excavator] {
        for via_card in [false, true] {
            let mut game = headless_game();
            let worker = game
                .state
                .units()
                .iter()
                .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
                .unwrap()
                .id;
            let mut data = serde_json::to_value(&*game.state).unwrap();
            let row = data["units"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|u| u["id"] == serde_json::json!(worker))
                .unwrap();
            row["kind"] = serde_json::json!(kind);
            row["worker"]["carrying"] = serde_json::json!(4);
            *game.state = serde_json::from_value(data).unwrap();
            game.presentation.selection.units = vec![worker];
            let mut input = InputState::new();
            let panel = crate::panel::build_for_palette(&game.view(), &classic(), false).unwrap();
            let card = panel
                .cards
                .iter()
                .find(|card| card.action.semantic() == Some(Action::ReturnCargo))
                .unwrap();
            assert!(card.enabled);
            assert!(
                std::ptr::eq(card, panel.cards.last().unwrap()),
                "it comes and goes, so it never shifts another card"
            );
            if via_card {
                activate_card(&mut game, &mut input, card.action);
            } else {
                apply_events(&mut game, &mut input, &[key_down(Key::U)]);
            }
            assert!(game.pending.iter().any(|pc| matches!(&pc.command, Command::ReturnCargo { units, foundry: None, repair: false } if units == &[worker])));
            let pending = std::mem::take(&mut game.pending);
            let report = game.state.tick(&pending);
            assert!(
                !report
                    .events
                    .iter()
                    .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. }))
            );
        }
    }
}

#[test]
fn the_dock_stop_square_halts_the_selection_by_click_and_tap() {
    let mut game = headless_game();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: game.presentation.human,
        command: Command::Hunt {
            units: vec![harvester],
            goal: TilePos::new(8, 8),
            queue: false,
        },
    }]);
    game.presentation.selection.units = vec![harvester];
    let square = macroquad::math::Rect::new(8.0, 560.0, 44.0, 44.0);
    for touch in [false, true] {
        let mut input = InputState::new();
        input.now = 1.0;
        let panel =
            crate::panel::build_for_input(&game.view(), &classic(), &input).expect("a panel");
        let action = panel.stop.as_ref().expect("a busy unit can stop").action;
        *game.presentation.panel_model.borrow_mut() = Some(panel);
        let mut layout = bare_layout(680.0, 500.0);
        layout.queue_stop = (square, action);
        game.presentation.layout.set(layout);
        game.pending.clear();
        if touch {
            apply_events(&mut game, &mut input, &[touch_down(1, square.center())]);
            input.now += 0.1;
            apply_events(&mut game, &mut input, &[touch_up(1, square.center())]);
        } else {
            apply_events(
                &mut game,
                &mut input,
                &click(square.center().x, square.center().y),
            );
        }
        assert!(
            game.pending
                .iter()
                .any(|pc| matches!(&pc.command, Command::Stop { units } if units == &[harvester])),
            "touch: {touch}"
        );
    }
}

#[test]
fn return_cargo_foundry_click_keeps_empty_welders_and_loaded_workers() {
    for damaged in [false, true] {
        let mut game = headless_game();
        let workers: Vec<_> = game
            .state
            .units()
            .iter()
            .filter(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
            .map(|u| u.id)
            .collect();
        assert!(workers.len() >= 2);
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == game.presentation.human && b.kind.is_drop_off())
            .unwrap()
            .id;
        let mut data = serde_json::to_value(&*game.state).unwrap();
        let loaded = data["units"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|u| u["id"] == serde_json::json!(workers[0]))
            .unwrap();
        loaded["worker"]["carrying"] = serde_json::json!(4);
        if damaged {
            let building = data["buildings"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|b| b["id"] == serde_json::json!(foundry))
                .unwrap();
            building["hp"] = serde_json::json!(building["hp"].as_u64().unwrap() - 10);
        }
        *game.state = serde_json::from_value(data).unwrap();
        game.presentation.selection.units = workers.clone();
        let b = game.state.building(foundry).unwrap();
        let screen = game
            .presentation
            .camera
            .to_screen(vec2(b.anchor.x as f32 + 0.5, b.anchor.y as f32 + 0.5));
        let mut input = InputState::new();
        apply_events(&mut game, &mut input, &[right_down(screen)]);
        assert!(game.pending.iter().any(|pc| matches!(&pc.command, Command::ReturnCargo { units, foundry: Some(f), repair } if units == &[workers[0]] && *f == foundry && *repair == damaged)));
        if damaged {
            assert!(game.pending.iter().any(|pc| matches!(&pc.command, Command::Repair { units, building, queue: false } if *building == foundry && !units.contains(&workers[0]) && units.contains(&workers[1]))));
        }
    }
}

#[test]
fn return_cargo_empty_selection_hides_the_card_and_refuses_the_shortcut() {
    let mut game = headless_game();
    let worker = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    game.presentation.selection.units = vec![worker];
    let mut input = InputState::new();
    let panel = crate::panel::build_for_palette(&game.view(), &classic(), false).unwrap();
    assert!(
        !panel
            .cards
            .iter()
            .any(|card| card.action.semantic() == Some(Action::ReturnCargo))
    );
    apply_events(&mut game, &mut input, &[key_down(Key::U)]);
    assert!(game.pending.is_empty());
}

#[test]
fn shared_cargo_shortcut_unloads_a_transport() {
    let mut game = skyhook_interaction_game();
    let transport = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Skyhook)
        .unwrap()
        .id;
    let passenger = game
        .state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Sentinel)
        .unwrap()
        .id;
    game.issue(Command::Load {
        units: vec![passenger],
        transport,
        queue: false,
    });
    for _ in 0..180 {
        game.do_tick();
    }
    assert_eq!(game.state.unit(transport).unwrap().cargo.len(), 1);
    game.presentation.selection.units = vec![transport];
    let mut input = InputState::new();
    controls_key(&mut game, &mut input, Key::U);
    assert!(
        matches!(game.pending.as_slice(), [PlayerCommand { command: Command::Unload { transport: id, queue: false, .. }, .. }] if *id == transport)
    );
    for _ in 0..180 {
        game.do_tick();
    }
    assert!(game.state.unit(transport).unwrap().cargo.is_empty());
    assert!(game.state.unit(passenger).is_some());
}

#[test]
fn mixed_workers_use_the_cargo_shortcut_and_keep_other_unit_bindings() {
    let mut game = headless_game();
    let worker = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    let mut data = serde_json::to_value(&*game.state).unwrap();
    let row = data["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|u| u["id"] == serde_json::json!(worker))
        .unwrap();
    row["worker"]["carrying"] = serde_json::json!(4);
    *game.state = serde_json::from_value(data).unwrap();
    game.presentation.selection.units = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .map(|u| u.id)
        .collect();
    assert!(game.presentation.selection.units.len() > 1);
    let mut input = InputState::new();
    input.build_menu = true;
    controls_key(&mut game, &mut input, Key::U);
    assert!(!input.construction_open());
    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::ReturnCargo { .. },
            ..
        }]
    ));
    game.pending.clear();
    controls_key(&mut game, &mut input, Key::G);
    assert!(input.armed(ClickVerb::Run));
    controls_key(&mut game, &mut input, Key::X);
    assert!(matches!(
        game.pending.last().unwrap().command,
        Command::Stop { .. }
    ));
}
