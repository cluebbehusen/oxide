//! Integration tests for the input funnel: real events through the real
//! resolver against a real (headless) sim.

use super::*;
use oxide_sim::{PlayerCommand, UnitKind};

fn headless_game() -> Game {
    Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0))
        .expect("embedded skirmish builds")
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

fn click(x: f32, y: f32) -> [RawEvent; 2] {
    [
        RawEvent::MouseDown {
            button: MouseButton::Left,
            x,
            y,
        },
        RawEvent::MouseUp {
            button: MouseButton::Left,
            x,
            y,
        },
    ]
}

#[test]
fn shift_click_selects_and_toggles_same_owner_buildings() {
    let mut game = multi_producer_game();
    let mut input = InputState::new();
    let mut own: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.human)
        .map(|building| building.id)
        .collect();
    own.sort_unstable();
    assert_eq!(own.len(), 2);
    let center = |game: &Game, id| {
        let building = game.state.building(id).unwrap();
        let size = building.kind.stats().size;
        game.camera.to_screen(vec2(
            building.anchor.x as f32 + size.0 as f32 * 0.5,
            building.anchor.y as f32 + size.1 as f32 * 0.5,
        ))
    };

    let first = center(&game, own[0]);
    apply_events(&mut game, &mut input, &click(first.x, first.y));
    assert_eq!(game.selection.buildings, vec![own[0]]);

    let second = center(&game, own[1]);
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: second.x,
                y: second.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: second.x,
                y: second.y,
            },
            RawEvent::KeyUp { key: Key::Shift },
        ],
    );
    assert_eq!(game.selection.buildings, own);

    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: first.x,
                y: first.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: first.x,
                y: first.y,
            },
            RawEvent::KeyUp { key: Key::Shift },
        ],
    );
    assert_eq!(game.selection.buildings, vec![own[1]]);
}

#[test]
fn selected_producers_receive_the_same_context_rally_in_id_order() {
    let mut game = multi_producer_game();
    let mut producers: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.human)
        .map(|building| building.id)
        .collect();
    producers.sort_unstable();
    game.selection.buildings = producers.clone();
    let rally = TilePos::new(14, 9);
    let screen = game
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
    game.selection.buildings = vec![turret, foundry];

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
    game.selection.buildings = vec![foundry, fabricator];

    super::orders::train(&mut game, 2);

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
        .find(|unit| unit.player != game.human && unit.tile() == TilePos::new(12, 4))
        .unwrap();
    assert!(game.my_vision().visible(enemy.tile()));
    let enemy_id = enemy.id;
    let screen = game.camera.to_screen(vec2(
        enemy.pos.x.to_num::<f32>(),
        enemy.pos.y.to_num::<f32>(),
    ));
    game.selection.buildings = vec![turret];

    context_order(&mut game, screen, false);

    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::FocusFire { buildings, target },
            ..
        }] if buildings == &vec![turret] && *target == oxide_sim::Target::Unit(enemy_id)
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
    game.camera.center = world;
    game.camera.pan(Vec2::ZERO);
    let point = game.camera.to_screen(world);
    apply_events(game, input, &click(point.x, point.y));
}

#[test]
fn bookmarks_remember_and_recall_camera_ground() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let saved = game.camera.center;
    let chord = |game: &mut Game, input: &mut InputState, ctrl: bool, key: Key| {
        let mut ev = Vec::new();
        if ctrl {
            ev.push(RawEvent::KeyDown { key: Key::Ctrl });
        }
        ev.push(RawEvent::KeyDown { key });
        ev.push(RawEvent::KeyUp { key });
        if ctrl {
            ev.push(RawEvent::KeyUp { key: Key::Ctrl });
        }
        apply_events(game, input, &ev);
    };
    chord(&mut game, &mut input, true, Key::F5);
    game.camera.center = saved + vec2(6.0, 4.0);
    chord(&mut game, &mut input, false, Key::F5);
    assert!(
        (game.camera.center - saved).length() < 1e-4,
        "recall returns to the remembered ground"
    );
    chord(&mut game, &mut input, false, Key::F6);
    assert!(
        (game.camera.center - saved).length() < 1e-4,
        "an empty slot recalls nothing"
    );
}

#[test]
fn the_cycle_key_walks_idle_harvesters_in_id_order() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let idle = idle_harvesters(&game);
    assert!(idle.len() >= 2, "premise: skirmish opens with idle workers");
    let press = |game: &mut Game, input: &mut InputState| {
        apply_events(
            game,
            input,
            &[
                RawEvent::KeyDown { key: Key::N },
                RawEvent::KeyUp { key: Key::N },
            ],
        );
    };
    press(&mut game, &mut input);
    assert_eq!(game.selection.units, vec![idle[0]]);
    press(&mut game, &mut input);
    assert_eq!(game.selection.units, vec![idle[1]], "id order, forward");
    for _ in 0..idle.len() - 1 {
        press(&mut game, &mut input);
    }
    assert_eq!(game.selection.units, vec![idle[0]], "and wraps");
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
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.human)
        .unwrap()
        .id;
    game.selection.units = vec![harvester];
    input.placing = Some(oxide_sim::BuildingKind::Turret);

    // Skirmish's own foundry footprint is illegal ground: the
    // misclick toasts and stays armed, staging nothing.
    let foundry = game.state.buildings()[0].anchor;
    let bad = game
        .camera
        .to_screen(vec2(foundry.x as f32 + 0.5, foundry.y as f32 + 0.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: bad.x,
            y: bad.y,
        }],
    );
    assert!(input.placing.is_some(), "a misclick must not disarm");
    assert!(game.pending.is_empty(), "and must spend nothing");

    // Shift-click on open visible ground stages and stays armed.
    let open = game
        .camera
        .to_screen(vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: open.x,
                y: open.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: open.x,
                y: open.y,
            },
        ],
    );
    assert_eq!(game.pending.len(), 1, "legal ground stages the site");
    assert!(input.placing.is_some(), "shift keeps the wall going up");

    // A plain click (press AND release — the mode settles at the
    // release, where the placement drag ends) disarms after staging.
    // Skirmish's 150 scrap is spent after the shift stamp, and a
    // BROKE click now refuses and keeps the mode armed — so the
    // disarm half runs in a fresh, still-funded session.
    let mut game = headless_game();
    let mut input = InputState::new();
    game.selection.units = vec![
        game.state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
            .unwrap()
            .id,
    ];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: open.x + 96.0,
                y: open.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: open.x + 96.0,
                y: open.y,
            },
        ],
    );
    assert_eq!(game.pending.len(), 1, "the plain click stages its site");
    assert!(input.placing.is_none(), "a plain click finishes the job");
}

#[test]
fn a_click_on_a_unit_selects_it_headlessly() {
    // The whole event path — resolver, hit-testing, selection —
    // exercised with no window: the C5 extraction's proof.
    let mut game = headless_game();
    let mut input = InputState::new();
    let unit = game.state.units()[0].id;
    let pos = game.state.units()[0].pos;
    let screen = game
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(game.selection.units, vec![unit]);
}

#[test]
fn a_right_click_on_ground_stages_an_advance() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let pos = game.state.units()[0].pos;
    let screen = game
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    let mid = game.camera.to_screen(vec2(
        pos.x.to_num::<f32>() + 4.0,
        pos.y.to_num::<f32>() + 2.0,
    ));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Right,
            x: mid.x,
            y: mid.y,
        }],
    );
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Advance { .. })),
        "zero-chase advance staged: {:?}",
        game.pending
    );
}

#[test]
fn a_context_order_cancels_placement_and_every_deferred_build_ghost() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let builder = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player == game.human && unit.kind == UnitKind::Harvester)
        .expect("a starting Harvester")
        .id;
    let start = game.state.unit(builder).unwrap().tile();
    let kind = oxide_sim::BuildingKind::Turret;
    let claims = [
        PlayerCommand {
            player: game.human,
            command: Command::Build {
                units: vec![builder],
                kind,
                anchor: start.offset(5, 0),
                queue: false,
                defer: true,
            },
        },
        PlayerCommand {
            player: game.human,
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
    game.selection.units = vec![builder];
    input.placing = Some(kind);

    let goal = start.offset(0, 4);
    let point = game
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Right,
            x: point.x,
            y: point.y,
        }],
    );

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
    assert!(matches!(builder.order, oxide_sim::Order::Move { .. }));
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
        .find(|building| building.player == game.human)
        .expect("human Foundry")
        .id;
    game.selection.buildings = vec![foundry];

    let card = macroquad::math::Rect::new(300.0, 700.0, 60.0, 60.0);
    let zero = macroquad::math::Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cards = [(zero, crate::panel::CardAction::None); 16];
    cards[0] = (card, crate::panel::CardAction::ArmRally);
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(1280.0, 800.0),
        1.0,
        680.0,
        500.0,
        zero,
        zero,
        zero,
        zero,
        zero,
        [(zero, crate::panel::CardAction::None); 8],
        0,
        cards,
        1,
        [(zero, crate::panel::CardAction::None); 8],
        0,
    ));

    input.now = 1.0;
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::TouchDown {
                id: 1,
                x: card.x + 20.0,
                y: card.y + 20.0,
            },
            RawEvent::TouchUp {
                id: 1,
                x: card.x + 20.0,
                y: card.y + 20.0,
            },
        ],
    );
    assert_eq!(input.rallying, vec![foundry]);

    let rally = chassis::grid::TilePos::new(14, 9);
    let point = game
        .camera
        .to_screen(vec2(rally.x as f32 + 0.5, rally.y as f32 + 0.5));
    input.now = 2.0;
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::TouchDown {
                id: 2,
                x: point.x,
                y: point.y,
            },
            RawEvent::TouchUp {
                id: 2,
                x: point.x,
                y: point.y,
            },
        ],
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
fn the_armed_mode_ribbon_cancel_is_a_real_touch_action() {
    let mut game = headless_game();
    let mut input = InputState::new();
    input.attacking = true;
    let ribbon = macroquad::math::Rect::new(220.0, 620.0, 280.0, 44.0);
    let cancel = macroquad::math::Rect::new(456.0, 620.0, 44.0, 44.0);
    let zero = macroquad::math::Rect::new(0.0, 0.0, 0.0, 0.0);
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(1280.0, 800.0),
        1.0,
        f32::INFINITY,
        0.0,
        zero,
        zero,
        zero,
        ribbon,
        cancel,
        [(zero, crate::panel::CardAction::None); 8],
        0,
        [(zero, crate::panel::CardAction::None); 16],
        0,
        [(zero, crate::panel::CardAction::None); 8],
        0,
    ));
    let at = cancel.center();
    input.now = 1.0;
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::TouchDown {
                id: 11,
                x: at.x,
                y: at.y,
            },
            RawEvent::TouchUp {
                id: 11,
                x: at.x,
                y: at.y,
            },
        ],
    );
    assert_eq!(input.armed_mode(), None);
    assert!(game.pending.is_empty(), "cancel emits no gameplay command");
}

#[test]
fn every_targeting_mode_has_persistent_human_copy() {
    let mut input = InputState::new();
    input.placing = Some(oxide_sim::BuildingKind::Bastion);
    assert_eq!(input.armed_mode().unwrap().label(), "BUILD BASTION");
    input.disarm_click_verbs();
    input.rallying = vec![oxide_sim::BuildingId(0)];
    assert_eq!(input.armed_mode().unwrap().label(), "SET RALLY");
    input.disarm_click_verbs();
    input.salvaging = true;
    assert_eq!(input.armed_mode().unwrap().label(), "SALVAGE");
    input.disarm_click_verbs();
    input.repairing = true;
    assert_eq!(input.armed_mode().unwrap().label(), "WELD UNIT");
    input.disarm_click_verbs();
    input.running = true;
    assert_eq!(input.armed_mode().unwrap().label(), "RUN");
    input.disarm_click_verbs();
    input.attacking = true;
    assert_eq!(input.armed_mode().unwrap().label(), "ATTACK-MOVE");
    input.disarm_click_verbs();
    input.patrol_route = Some(vec![TilePos::new(1, 1), TilePos::new(2, 2)]);
    assert_eq!(input.armed_mode().unwrap().label(), "PATROL | 2 WAYPOINTS");
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
        .filter(|o| o.kind == kind && o.player == game.human)
        .count();
    assert!(same_kind_total > 1, "premise: kin on screen to sweep up");
    let screen = game
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    input.now = 10.0;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    // A slow second click is just a click...
    input.now = 11.0;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(game.selection.units.len(), 1, "1.0s apart is two clicks");
    // ...a fast one is a kind-sweep.
    input.now = 11.2;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert!(
        game.selection.units.len() > 1,
        "0.2s apart double-clicks into a kind sweep"
    );
}

#[test]
fn wheel_notches_and_trackpad_swipes_land_in_the_same_range() {
    // Windows notches (±120), X11 detents (±1), and a firm trackpad
    // swipe all read as whole steps; small fractional trackpad deltas
    // stay gentle.
    assert_eq!(normalize_wheel(120.0), 1.0);
    assert_eq!(normalize_wheel(-120.0), -1.0);
    assert_eq!(normalize_wheel(1.0), 1.0);
    assert_eq!(normalize_wheel(-1.0), -1.0);
    assert_eq!(normalize_wheel(2.0), 2.0);
    assert_eq!(normalize_wheel(10.0), 1.0);
    assert!(normalize_wheel(0.4) > 0.0 && normalize_wheel(0.4) < 0.1);
}

#[test]
fn wheel_bursts_are_capped() {
    assert_eq!(normalize_wheel(1200.0), 3.0);
    assert_eq!(normalize_wheel(-1200.0), -3.0);
    // The cap also catches fast trackpad flicks below the notch cutoff.
    assert_eq!(normalize_wheel(39.9), 3.0);
}

#[test]
fn every_build_palette_entry_costs_scrap_to_raise() {
    // The palette is exactly what a harvester can place, so each entry
    // must carry construction stats with a real price. A `None` (a
    // Foundry-style scenario-only kind) or a zero cost would offer a
    // ghost the sim can never accept.
    for kind in BUILD_PALETTE {
        let cost = kind
            .stats()
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
    for (i, a) in BUILD_PALETTE.iter().enumerate() {
        for b in BUILD_PALETTE.iter().skip(i + 1) {
            assert_ne!(a, b, "{} appears twice", a.name());
        }
    }
}

#[test]
fn the_build_palette_fits_the_digit_selectors() {
    // `digit_action` indexes the palette with slots 0..=8 (number keys
    // 1-9); an entry past the ninth could never be selected.
    assert!(
        BUILD_PALETTE.len() <= 9,
        "palette overflows the 1-9 digit range"
    );
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
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.human)
        .unwrap()
        .id;
    // Stand a Fabricator site on open visible ground near the base.
    let foundry = game.state.buildings()[0].anchor;
    let anchor = chassis::grid::TilePos::new(foundry.x + 3, foundry.y + 4);
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: game.human,
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
            .any(|b| b.anchor == anchor && !b.built),
        "premise: the site stands"
    );
    // Select the harvester, then right-click the site's far corner.
    game.selection.units = vec![harvester];
    let corner = game
        .camera
        .to_screen(vec2(anchor.x as f32 + 1.5, anchor.y as f32 + 1.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Right,
            x: corner.x,
            y: corner.y,
        }],
    );
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
        \"seed\": 7,
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
        .find(|b| b.player == game.human)
        .unwrap()
        .id;
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.human)
        .unwrap()
        .id;
    let rat = game
        .state
        .units()
        .iter()
        .find(|u| u.player != game.human)
        .unwrap()
        .id;
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: Command::Attack {
            units: vec![rat],
            target: oxide_sim::Target::Building(foundry),
            queue: false,
        },
    }]);
    for _ in 0..120 {
        game.state.tick(&[]);
    }
    let wall = game.state.building(foundry).unwrap();
    assert!(
        wall.hp < wall.kind.stats().max_hp,
        "premise: the rat left scars"
    );
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
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    assert!(
        wall.tiles().any(|t| t == tile),
        "premise: the click lands on the wall ({tile:?})"
    );
    assert!(
        world.distance(rat_pos) <= PICK_RADIUS,
        "premise: the rat is inside the pick radius"
    );
    game.selection.units = vec![harvester];
    let screen = game.camera.to_screen(world);
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Right,
                x: screen.x,
                y: screen.y,
            },
        ],
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
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.human)
        .unwrap()
        .id;
    let turret = game
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::BuildingKind::Turret)
        .unwrap()
        .id;
    game.selection.units = vec![harvester];
    // Arm with the hotkey, exactly as a player would.
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::V },
            RawEvent::KeyUp { key: Key::V },
        ],
    );
    assert!(input.salvaging, "V arms the wrecking crew");

    // A click on the Foundry refuses and stays armed.
    let foundry = game.state.buildings()[0].anchor;
    let on_foundry = game
        .camera
        .to_screen(vec2(foundry.x as f32 + 0.5, foundry.y as f32 + 0.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: on_foundry.x,
            y: on_foundry.y,
        }],
    );
    assert!(game.pending.is_empty(), "the victory token refuses");
    assert!(input.salvaging, "a misclick keeps the mode armed");

    // A click on the turret stages the teardown and stands down.
    let on_turret = game.camera.to_screen(vec2(9.5, 5.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: on_turret.x,
            y: on_turret.y,
        }],
    );
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            Command::Salvage { building, queue: false, .. } if *building == turret
        )),
        "the click sends the crew: {:?}",
        game.pending
    );
    assert!(!input.salvaging, "a plain click finishes the job");
}

#[test]
fn the_armed_run_verb_issues_an_oblivious_move() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.human && u.kind == UnitKind::Sentinel)
        .expect("skirmish authors a sentinel")
        .id;
    game.selection.units = vec![fighter];
    // Arm with the classic hotkey, exactly as a player would.
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::M },
            RawEvent::KeyUp { key: Key::M },
        ],
    );
    assert!(input.running, "M arms the recall");

    // The click sends a plain Move — the OBLIVIOUS walk, not the
    // explicit fighting march armed with F — and stands down.
    let home = game.state.unit(fighter).unwrap().tile();
    let goal = TilePos::new(home.x + 3, home.y);
    let p = game
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
        ],
    );
    assert!(
        game.pending.iter().any(|c| matches!(
            &c.command,
            Command::Move { goal: g, queue: false, .. } if *g == goal
        )),
        "the armed click issues Command::Move: {:?}",
        game.pending
    );
    assert!(!input.running, "a plain click finishes the recall");
    assert!(
        !game
            .pending
            .iter()
            .any(|c| matches!(&c.command, Command::AttackMove { .. })),
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
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.human)
        .unwrap()
        .id;
    game.selection.units = vec![harvester];
    // Placement armed, then M: exactly one verb may hold the cursor —
    // armed_click resolves placement before run, so both live at once
    // would stamp a building under a "run" toast.
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::M },
            RawEvent::KeyUp { key: Key::M },
        ],
    );
    assert!(input.running, "M arms the recall");
    assert!(input.placing.is_none(), "and placement stood down");
    let home = game.state.unit(harvester).unwrap().tile();
    let p = game
        .camera
        .to_screen(vec2(home.x as f32 + 2.5, home.y as f32 + 0.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
        ],
    );
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
            .any(|c| matches!(&c.command, Command::Move { .. })),
        "the click issued the run"
    );
    // And the mirror direction: arming salvage stands run down.
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::M },
            RawEvent::KeyUp { key: Key::M },
            RawEvent::KeyDown { key: Key::V },
            RawEvent::KeyUp { key: Key::V },
        ],
    );
    assert!(input.salvaging, "V arms salvage");
    assert!(!input.running, "and the run stood down");
}

#[test]
fn f_arms_explicit_attack_move_and_the_click_consumes_it() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.human && u.kind.stats().can_fight())
        .expect("a starting combat unit")
        .id;
    game.selection.units = vec![fighter];
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::F },
            RawEvent::KeyUp { key: Key::F },
        ],
    );
    assert!(input.attacking, "F arms the fighting march");
    assert!(!input.running, "attack-move and run are mutually exclusive");

    let goal = game.state.unit(fighter).unwrap().tile().offset(4, 1);
    let p = game
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    apply_events(&mut game, &mut input, &click(p.x, p.y));

    assert!(game.pending.iter().any(|command| matches!(
        command.command,
        Command::AttackMove {
            goal: staged,
            queue: false,
            ..
        } if staged == goal
    )));
    assert!(!input.attacking, "a plain click consumes the armed verb");
}

#[test]
fn the_attack_move_card_is_touchable_and_arms_the_same_world_tap() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.human && u.kind.stats().can_fight())
        .expect("a starting combat unit")
        .id;
    game.selection.units = vec![fighter];

    let card = macroquad::math::Rect::new(300.0, 700.0, 60.0, 60.0);
    let zero = macroquad::math::Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cards = [(zero, crate::panel::CardAction::None); 16];
    cards[0] = (card, crate::panel::CardAction::Dispatch(Action::AttackMove));
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(1280.0, 800.0),
        1.0,
        680.0,
        500.0,
        zero,
        zero,
        zero,
        zero,
        zero,
        [(zero, crate::panel::CardAction::None); 8],
        0,
        cards,
        1,
        [(zero, crate::panel::CardAction::None); 8],
        0,
    ));

    input.now = 2.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: card.x + 20.0,
            y: card.y + 20.0,
        }],
    );
    input.now = 2.1;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 1,
            x: card.x + 20.0,
            y: card.y + 20.0,
        }],
    );
    assert!(input.attacking, "the fingertip arms the panel verb");

    let goal = game.state.unit(fighter).unwrap().tile().offset(4, 1);
    let point = game
        .camera
        .to_screen(vec2(goal.x as f32 + 0.5, goal.y as f32 + 0.5));
    input.now = 3.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 2,
            x: point.x,
            y: point.y,
        }],
    );
    input.now = 3.1;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 2,
            x: point.x,
            y: point.y,
        }],
    );

    assert!(game.pending.iter().any(|command| matches!(
        command.command,
        Command::AttackMove {
            goal: staged,
            queue: false,
            ..
        } if staged == goal
    )));
    assert!(!input.attacking, "the world tap consumes the armed verb");
}

#[test]
fn a_paused_stroke_bills_each_kind_at_its_own_price() {
    // Bank 360: one staged turret (100) plus an armed bastion (250)
    // is affordable at the ACTUAL sum (350). The old count-times-
    // current-kind math priced the staged turret as a second bastion
    // (500) and refused a funded placement.
    let mut game = drag_arena(360);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    let p = game.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
        ],
    );
    assert_eq!(staged_builds(&game), 1, "the turret staged");
    // The clock never ran (paused shell): the turret is still pending
    // when the palette switches kinds.
    input.placing = Some(oxide_sim::BuildingKind::Bastion);
    let p2 = game.camera.to_screen(vec2(9.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p2.x,
                y: p2.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p2.x,
                y: p2.y,
            },
        ],
    );
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
    game.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    // Stroke A stamps a turret; the clock never runs, so the site
    // exists only in pending — live state still shows open ground.
    let p = game.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
        ],
    );
    assert_eq!(staged_builds(&game), 1, "stroke A staged its site");
    // Stroke B opens on the same tile: the ground is spoken for, and
    // acknowledging the stamp would hand the sim a doomed command.
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
        ],
    );
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
    game.selection.units = vec![builder];
    // Three queued walks staged while paused: the builder's program
    // will hold them the moment the clock runs, so a build stroke
    // must see three fewer free slots even though live state still
    // reads an idle unit.
    for x in [14, 15, 16] {
        game.issue(Command::Move {
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
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::KeyDown { key: Key::Shift }],
    );
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
    game.selection.units = vec![builder];
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
    let shift = [RawEvent::KeyDown { key: Key::Shift }];
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
    game.selection.units = vec![builder];

    // Leave exactly one queue slot for the opening Shift stamp.
    let mut fill = vec![PlayerCommand {
        player: game.human,
        command: Command::Move {
            units: vec![builder],
            goal: TilePos::new(14, 7),
            queue: false,
        },
    }];
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP - 1 {
        fill.push(PlayerCommand {
            player: game.human,
            command: Command::Move {
                units: vec![builder],
                goal: TilePos::new(15, 7),
                queue: true,
            },
        });
    }
    game.state.tick(&fill);

    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::KeyDown { key: Key::Shift }],
    );
    let first = game.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: first.x,
            y: first.y,
        }],
    );
    assert_eq!(staged_builds(&game), 1, "the last free slot was used");

    game.issue(Command::Stop {
        units: vec![builder],
    });
    let second = game.camera.to_screen(vec2(6.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseMove {
            x: second.x,
            y: second.y,
        }],
    );
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
    game.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    let first = game.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: first.x,
            y: first.y,
        }],
    );
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP {
        game.issue(Command::Move {
            units: vec![builder],
            goal: TilePos::new(15, 7),
            queue: true,
        });
    }
    let second = game.camera.to_screen(vec2(6.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseMove {
            x: second.x,
            y: second.y,
        }],
    );
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
        \"seed\": 9,
        \"players\": [
            {\"name\": \"me\", \"faction\": \"ferrous\", \"scrap\": 100, \"bot\": false, \"team\": 1},
            {\"name\": \"pal\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true, \"team\": 1,
             \"bot_config\": {\"level\": \"easy\"}},
            {\"name\": \"foe\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true,
             \"bot_config\": {\"level\": \"hard\"}}
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
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(game.selection.units, vec![ally], "allies are selectable");

    // The panel is read-only: no command cards; a single ally shows
    // static combat capability and its order chips.
    let panel = crate::panel::build(&game, &input.bindings).expect("a panel");
    assert!(panel.cards.is_empty(), "no verbs on an ally panel");
    assert!(panel.sub.contains("Easy"), "bot difficulty stays visible");
    assert_eq!(panel.combat.len(), 2);
    assert_eq!(panel.combat[0].icon, crate::panel::CombatIcon::Unarmed);
    assert_eq!(panel.combat[0].text, "unarmed");
    assert_eq!(panel.combat[1].icon, crate::panel::CombatIcon::Speed);
    assert!(!panel.queue.is_empty(), "the ally's orders show");
    assert_eq!(
        panel.faction,
        oxide_sim::Faction::Cupric,
        "its colors, not mine"
    );

    // Every command path refuses: right-click stages nothing…
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::MouseDown {
            button: MouseButton::Right,
            x: screen.x + 60.0,
            y: screen.y,
        }],
    );
    assert!(game.pending.is_empty(), "ally units take no orders");
    // …and group assignment drops the foreign pick.
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Ctrl },
            RawEvent::KeyDown { key: Key::Num1 },
            RawEvent::KeyUp { key: Key::Num1 },
            RawEvent::KeyUp { key: Key::Ctrl },
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
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(game.selection.units, vec![foe], "a visible foe inspects");

    // Static kind-level combat facts are safe to inspect. Command cards
    // and order chips stay absent because order state reveals intent.
    let panel = crate::panel::build(&game, &input.bindings).expect("a panel");
    assert!(panel.cards.is_empty(), "no verbs on a hostile panel");
    assert!(panel.queue.is_empty(), "no order chips on a hostile panel");
    assert!(panel.sub.contains("Hard"), "enemy difficulty stays visible");
    assert_eq!(panel.combat.len(), 2);
    assert_eq!(panel.combat[0].icon, crate::panel::CombatIcon::Weapon);
    assert!(panel.combat[0].text.contains("dmg"));
    assert!(panel.combat[0].text.contains("tiles"));
    assert!(panel.combat[0].text.contains("ground"));

    // And no breadcrumbs, whatever program the enemy runs.
    let unit = game.state.unit(foe).unwrap();
    assert!(
        crate::render::entities::breadcrumb_points(&game, unit).is_empty(),
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
        .camera
        .to_screen(vec2(my_pos.x.to_num::<f32>(), my_pos.y.to_num::<f32>()));
    let ally_screen = game
        .camera
        .to_screen(vec2(ally_pos.x.to_num::<f32>(), ally_pos.y.to_num::<f32>()));
    // Own selected, shift-click the ally: REPLACE, never merge.
    apply_events(&mut game, &mut input, &click(my_screen.x, my_screen.y));
    assert_eq!(game.selection.units, vec![mine]);
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: ally_screen.x,
                y: ally_screen.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: ally_screen.x,
                y: ally_screen.y,
            },
            RawEvent::KeyUp { key: Key::Shift },
        ],
    );
    assert_eq!(
        game.selection.units,
        vec![ally],
        "a different owner replaces the selection"
    );
    // A box over both takes the OWN units only.
    let a = game.camera.to_screen(vec2(6.0, 1.5));
    let b = game.camera.to_screen(vec2(9.0, 5.0));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: a.x,
                y: a.y,
            },
            RawEvent::MouseMove { x: b.x, y: b.y },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: b.x,
                y: b.y,
            },
        ],
    );
    assert_eq!(
        game.selection.units,
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
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    // A short still touch is a tap: select.
    input.now = 5.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: screen.x,
            y: screen.y,
        }],
    );
    input.now = 5.1;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 1,
            x: screen.x,
            y: screen.y,
        }],
    );
    assert_eq!(game.selection.units, vec![unit], "a tap selects");

    // A finger held still past the window fires the context order for
    // the live selection — a long-press is touch's right-click.
    let ground = game.camera.to_screen(vec2(
        pos.x.to_num::<f32>() + 4.0,
        pos.y.to_num::<f32>() + 2.0,
    ));
    input.now = 6.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 2,
            x: ground.x,
            y: ground.y,
        }],
    );
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

#[test]
fn an_armed_build_completes_on_a_tap() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let harvester = game
        .state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Harvester && u.player == game.human)
        .unwrap()
        .id;
    game.selection.units = vec![harvester];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    let foundry = game.state.buildings()[0].anchor;
    let open = game
        .camera
        .to_screen(vec2(foundry.x as f32 + 3.5, foundry.y as f32 + 3.5));
    input.now = 5.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: open.x,
            y: open.y,
        }],
    );
    input.now = 5.1;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 1,
            x: open.x,
            y: open.y,
        }],
    );
    assert!(
        game.pending
            .iter()
            .any(|c| matches!(c.command, Command::Build { .. })),
        "the tap after an armed card places the site, not a select: {:?}",
        game.pending
    );
    assert!(
        input.placing.is_none(),
        "an unmodified tap disarms like a plain click"
    );
    assert_eq!(
        game.selection.units,
        vec![harvester],
        "the armed tap never re-selected under the fingertip"
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
        .find(|b| b.player == game.human)
        .unwrap()
        .id;
    game.selection.buildings = vec![own];
    // The enemy Foundry's ground is unexplored — but an omniscient
    // entity probe would still see the building there and flip the
    // gesture from rally to select, leaking hidden occupancy.
    let foe = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player != game.human)
        .unwrap();
    let center = vec2(foe.anchor.x as f32 + 1.0, foe.anchor.y as f32 + 1.0);
    let foe_tile = foe.anchor;
    assert!(
        !game.my_vision().visible(foe_tile),
        "the probe point must sit under fog for this test to bite"
    );
    game.camera.center = center;
    let screen = game.camera.to_screen(center);
    input.now = 9.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 7,
            x: screen.x,
            y: screen.y,
        }],
    );
    input.now = 9.9;
    update_touch(&mut game, &mut input);
    assert_eq!(
        game.selection.buildings,
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
    let before = game.camera.center;
    // One moved finger pans the world under the hand.
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: 400.0,
            y: 300.0,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchMove {
            id: 1,
            x: 340.0,
            y: 300.0,
        }],
    );
    assert!(
        game.camera.center.x > before.x,
        "dragging left shows ground to the east"
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 1,
            x: 340.0,
            y: 300.0,
        }],
    );
    assert!(
        game.selection.units.is_empty(),
        "a drag is never a tap-select"
    );

    // Two steady fingers box-select everything between them.
    let a = game.camera.to_screen(vec2(2.0, 2.0));
    let b = game.camera.to_screen(vec2(12.0, 10.0));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: a.x,
            y: a.y,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 2,
            x: b.x,
            y: b.y,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 2,
            x: b.x,
            y: b.y,
        }],
    );
    assert!(
        !game.selection.units.is_empty(),
        "the finger-box swept the base"
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
        name: "ally-site-arena".into(),
        seed: 7,
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
        p.bot_config = Some(BotConfig {
            level: oxide_sim::bot::Level::Medium,
            aggression: None,
            style: None,
            variant: None,
            team_role: None,
        });
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
        command: Command::Move {
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
    game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: Command::Move {
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
        assert!(!site.built, "test premise: unfinished");
        assert!(
            !site.tiles().any(|t| game.my_vision().visible(t)),
            "test premise: the site sits under fog"
        );
        vec2(site.anchor.x as f32 + 0.5, site.anchor.y as f32 + 0.5)
    };
    game.camera.center = site_center;
    let screen = game.camera.to_screen(site_center);
    input.now = 5.0; // clicks land at the viewport center; keep them
    apply_events(&mut game, &mut input, &click(screen.x, screen.y)); // out of double-click range
    assert!(
        game.selection.buildings.is_empty(),
        "a fogged ally site must refuse the blind click"
    );
    // The built ally foundry selects through shared team sight.
    let (ally_foundry, center) = {
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == oxide_sim::PlayerId(1) && b.built)
            .unwrap();
        (
            foundry.id,
            vec2(foundry.anchor.x as f32 + 1.0, foundry.anchor.y as f32 + 1.0),
        )
    };
    game.camera.center = center;
    let screen = game.camera.to_screen(center);
    input.now = 10.0;
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(
        game.selection.buildings,
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
        \"seed\": 9,
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
    let a = game.camera.to_screen(vec2(9.0, 1.2));
    let b = game.camera.to_screen(vec2(21.5, 3.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: a.x,
                y: a.y,
            },
            RawEvent::MouseMove { x: b.x, y: b.y },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: b.x,
                y: b.y,
            },
        ],
    );
    assert_eq!(
        game.selection.units,
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
        \"seed\": 4,
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
        .camera
        .to_screen(vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(screen.x, screen.y));
    assert_eq!(game.selection.units, vec![foe]);
    // Send my only nearby eyes home; the foe's bot recalls its
    // harvester east to mine — both walks end my sight of it, and the
    // selection must end with the sight (the machine itself lives on).
    let mine = game.state.units()[0].id;
    game.issue(Command::Move {
        units: vec![mine],
        goal: TilePos::new(3, 4),
        queue: false,
    });
    for _ in 0..600 {
        game.do_tick();
        if game.selection.units.is_empty() {
            break;
        }
    }
    assert!(
        game.state.unit(foe).is_some(),
        "test premise: the machine is alive, only unseen"
    );
    assert!(
        game.selection.units.is_empty(),
        "the inspection let go with the sight"
    );
}

/// Publishes a layout whose minimap owns the window's bottom-right
/// corner — the chrome-ownership tests need real rects, exactly as the
/// renderer would publish them.
fn publish_minimap(game: &Game) -> macroquad::math::Rect {
    let minimap = macroquad::math::Rect::new(1060.0, 590.0, 200.0, 190.0);
    let zero = macroquad::math::Rect::new(0.0, 0.0, 0.0, 0.0);
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(1280.0, 800.0),
        1.0,
        f32::INFINITY,
        0.0,
        zero,
        minimap,
        zero,
        zero,
        zero,
        [(zero, crate::panel::CardAction::None); 8],
        0,
        [(zero, crate::panel::CardAction::None); 16],
        0,
        [(zero, crate::panel::CardAction::None); 8],
        0,
    ));
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
fn chrome_born_touches_never_drive_world_gestures() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let center_before = game.camera.center;

    // A swipe that LANDS on the minimap must not pan the world
    // behind it, however far it travels.
    input.now = 2.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: minimap.x + 20.0,
            y: minimap.y + 20.0,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchMove {
            id: 1,
            x: 400.0,
            y: 300.0,
        }],
    );
    assert_eq!(
        game.camera.center, center_before,
        "a chrome-born swipe keeps its hands off the camera"
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 1,
            x: 400.0,
            y: 300.0,
        }],
    );

    // The same swipe born on open ground pans.
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 2,
            x: 400.0,
            y: 300.0,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchMove {
            id: 2,
            x: 300.0,
            y: 260.0,
        }],
    );
    assert_ne!(
        game.camera.center, center_before,
        "a world-born swipe still drags the world"
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 2,
            x: 300.0,
            y: 260.0,
        }],
    );

    // A two-finger box with one chrome-born corner selects nothing —
    // even when the pair spans the whole own base.
    let own = game.state.units()[0].pos;
    let base = game
        .camera
        .to_screen(vec2(own.x.to_num::<f32>(), own.y.to_num::<f32>()));
    game.selection.units.clear();
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 3,
            x: minimap.x + 30.0,
            y: minimap.y + 30.0,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 4,
            x: base.x - 80.0,
            y: base.y - 80.0,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 4,
            x: base.x - 80.0,
            y: base.y - 80.0,
        }],
    );
    assert!(
        game.selection.units.is_empty(),
        "a chrome-born corner must not box the base: {:?}",
        game.selection.units
    );
}

#[test]
fn a_tap_on_the_idle_badge_cycles_workers() {
    let mut game = headless_game();
    let mut input = InputState::new();
    // Publish chrome with a live idle badge in the top bar — the
    // bare-chrome swallow used to eat fingertip taps on it while the
    // mouse path cycled workers.
    let badge = macroquad::math::Rect::new(200.0, 4.0, 60.0, 24.0);
    let zero = macroquad::math::Rect::new(0.0, 0.0, 0.0, 0.0);
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(1280.0, 800.0),
        1.0,
        f32::INFINITY,
        0.0,
        zero,
        zero,
        badge,
        zero,
        zero,
        [(zero, crate::panel::CardAction::None); 8],
        0,
        [(zero, crate::panel::CardAction::None); 16],
        0,
        [(zero, crate::panel::CardAction::None); 8],
        0,
    ));
    let before = game.camera.center;
    input.now = 3.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: badge.x + 10.0,
            y: badge.y + 10.0,
        }],
    );
    input.now = 3.1;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 1,
            x: badge.x + 10.0,
            y: badge.y + 10.0,
        }],
    );
    // Cycling an idle worker selects it and jumps the camera to it —
    // either effect proves the badge answered the fingertip.
    assert!(
        !game.selection.units.is_empty() || game.camera.center != before,
        "the badge answers a tap like it answers a click"
    );
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
        .find(|u| u.player != game.human)
        .unwrap()
        .id;
    game.selection.units = vec![foe];
    let right = |x: f32, y: f32| RawEvent::MouseDown {
        button: MouseButton::Right,
        x,
        y,
    };
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
        .find(|u| u.player == game.human)
        .unwrap()
        .id;
    game.selection.units = vec![mine];
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
fn touch_respects_chrome_ownership() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let minimap = publish_minimap(&game);
    let (mx, my) = (minimap.x + 40.0, minimap.y + 40.0);
    // Zoom in so the camera has travel (the whole small map fits the
    // default view and clamping would eat any jump).
    game.camera.zoom_at(vec2(640.0, 400.0), 4.0);
    game.camera.update(1.0); // land the glide: headless has no frames
    game.camera.center = vec2(4.0, 4.0);
    game.camera.pan(macroquad::prelude::Vec2::ZERO);
    // A tap on the minimap jumps the camera — it must not select the
    // world ground hiding under the chrome pixel.
    let before = game.camera.center;
    game.selection.units = vec![game.state.units()[0].id];
    let selected = game.selection.units.clone();
    input.now = 3.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 9,
            x: mx,
            y: my,
        }],
    );
    input.now = 3.1;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 9,
            x: mx,
            y: my,
        }],
    );
    assert_ne!(game.camera.center, before, "the tap steered the camera");
    assert_eq!(game.selection.units, selected, "and stole no selection");

    // A long-press there orders nothing: chrome owns its ground for
    // the held finger too.
    input.now = 4.0;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 10,
            x: mx,
            y: my,
        }],
    );
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
    game.selection.units = vec![game.state.units()[0].id];
    let keep = game.selection.units.clone();
    let zoom_before = game.camera.zoom;
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 1,
            x: 600.0,
            y: 400.0,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 2,
            x: 640.0,
            y: 400.0,
        }],
    );
    // Sub-pixel-per-event spread: forty gentle half-pixel steps sum to
    // a real pinch even though no single event crosses a threshold.
    for i in 0..40 {
        let x = 640.0 + (i as f32) * 0.9;
        apply_events(
            &mut game,
            &mut input,
            &[RawEvent::TouchMove { id: 2, x, y: 400.0 }],
        );
    }
    assert!(input.pinching, "the cumulative spread reads as a pinch");
    game.camera.update(1.0); // land the glide: headless has no frames
    assert!(game.camera.zoom > zoom_before, "and it zoomed in");
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 2,
            x: 676.0,
            y: 400.0,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 1,
            x: 600.0,
            y: 400.0,
        }],
    );
    assert_eq!(
        game.selection.units, keep,
        "a pinch's release never box-selects"
    );

    // And the NEXT pair starts undecided: a fresh steady pair still
    // commits its box (pinch state must not outlive its fingers).
    let a = game.camera.to_screen(vec2(2.0, 2.0));
    let b = game.camera.to_screen(vec2(12.0, 10.0));
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 3,
            x: a.x,
            y: a.y,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchDown {
            id: 4,
            x: b.x,
            y: b.y,
        }],
    );
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::TouchUp {
            id: 4,
            x: b.x,
            y: b.y,
        }],
    );
    assert!(
        !game.selection.units.is_empty(),
        "the fresh pair's box landed"
    );
}

#[test]
fn a_placement_drag_stamps_a_row_of_queued_builds() {
    // A funded arena: one harvester, 1000 scrap — room for a wall.
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Drag Range",
            "seed": 3,
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
    game.selection.units = vec![builder];
    input.placing = Some(oxide_sim::BuildingKind::Turret);

    // Screen points at the centers of three adjacent open tiles.
    let at = |x: i32, y: i32| game.camera.to_screen(vec2(x as f32 + 0.5, y as f32 + 0.5));
    let (a, b, c) = (at(7, 3), at(8, 3), at(9, 3));
    let mut events = vec![RawEvent::MouseDown {
        button: MouseButton::Left,
        x: a.x,
        y: a.y,
    }];
    events.push(RawEvent::MouseMove { x: b.x, y: b.y });
    events.push(RawEvent::MouseMove { x: c.x, y: c.y });
    events.push(RawEvent::MouseUp {
        button: MouseButton::Left,
        x: c.x,
        y: c.y,
    });
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
        .filter(|u| u.player == game.human)
        .map(|u| u.id)
        .collect();
    game.selection.units = mine.clone();
    let panel = crate::panel::build(&game, &input.bindings).expect("panel");
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
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::KeyDown { key: Key::Ctrl }],
    );
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::FilterKind(UnitKind::Harvester),
    );
    assert!(
        !game.selection.units.is_empty()
            && game
                .selection
                .units
                .iter()
                .all(|id| game.state.unit(*id).unwrap().kind != UnitKind::Harvester),
        "Ctrl cuts the named kind out"
    );
    apply_events(&mut game, &mut input, &[RawEvent::KeyUp { key: Key::Ctrl }]);

    // ...and the plain click keeps only the named kind.
    game.selection.units = mine;
    activate_card(
        &mut game,
        &mut input,
        crate::panel::CardAction::FilterKind(UnitKind::Sentinel),
    );
    assert!(
        !game.selection.units.is_empty()
            && game
                .selection
                .units
                .iter()
                .all(|id| game.state.unit(*id).unwrap().kind == UnitKind::Sentinel),
        "a plain click narrows to the kind"
    );
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
/// anchor the box where it LANDED, and the box must be drawable on the
/// very frame of the press. The polled adapter could do neither — it
/// stamped every button with the frame's LAST cursor position, so the
/// anchor jumped forward by a frame of travel and `mouse ==
/// drag_origin` made the rect zero-sized until the next frame.
#[test]
fn a_press_mid_flight_anchors_the_box_where_it_landed() {
    use macroquad::miniquad::EventHandler;
    // Retina: the platform speaks backing-store pixels, the shell logical ones.
    let mut stream = PointerStream::new(2.0);
    stream.mouse_motion_event(600.0, 400.0);
    stream.mouse_motion_event(760.0, 400.0);
    stream.mouse_button_down_event(macroquad::miniquad::MouseButton::Left, 768.0, 400.0);
    stream.mouse_motion_event(900.0, 400.0);
    stream.mouse_motion_event(1000.0, 400.0);
    assert_eq!(
        stream.events,
        vec![
            RawEvent::MouseMove { x: 300.0, y: 200.0 },
            RawEvent::MouseMove { x: 380.0, y: 200.0 },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: 384.0,
                y: 200.0
            },
            RawEvent::MouseMove { x: 450.0, y: 200.0 },
            RawEvent::MouseMove { x: 500.0, y: 200.0 },
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

/// The selection consequence of the same frame: a unit sitting between
/// the press point and where the pointer ended the frame belongs in the
/// box. The old adapter threw that stretch away.
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
        .filter(|u| u.player == game.human)
        .collect();
    let (lo, hi) = (mine[0].pos, mine[mine.len() - 1].pos);
    let a = game
        .camera
        .to_screen(vec2(lo.x.to_num::<f32>() - 1.0, lo.y.to_num::<f32>() - 1.0));
    let b = game
        .camera
        .to_screen(vec2(hi.x.to_num::<f32>() + 1.0, hi.y.to_num::<f32>() + 1.0));
    let want: Vec<_> = mine.iter().map(|u| u.id).collect();

    // One frame: the pointer flies past `a`, the button lands there,
    // and the pointer carries on to `b` before the frame ends.
    let mut stream = PointerStream::new(1.0);
    stream.mouse_motion_event(a.x - 40.0, a.y - 40.0);
    stream.mouse_button_down_event(macroquad::miniquad::MouseButton::Left, a.x, a.y);
    stream.mouse_motion_event(b.x, b.y);
    stream.mouse_button_up_event(macroquad::miniquad::MouseButton::Left, b.x, b.y);
    apply_events(&mut game, &mut input, &stream.events);
    let mut got = game.selection.units.clone();
    got.sort_unstable();
    assert_eq!(got, want, "the whole sweep selects, press point included");

    // The shape the polled adapter produced for that same frame: one
    // MouseMove at the frame's END, and a press and release stamped
    // there too. Origin == release, so the sweep read as a bare click.
    let mut input = InputState::new();
    input.ui = 1.0;
    game.selection.units.clear();
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::MouseMove { x: b.x, y: b.y },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: b.x,
                y: b.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: b.x,
                y: b.y,
            },
        ],
    );
    assert_ne!(
        game.selection.units, want,
        "premise: coalescing the frame into its last position loses the drag"
    );
}

/// The drag arena, parameterized by bank: same shape as the funded
/// test's fixture, one harvester, open ground.
fn drag_arena(scrap: u32) -> Game {
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Drag Bank",
            "seed": 3,
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

fn drag_over(game: &mut Game, input: &mut InputState, tiles: &[(i32, i32)]) {
    let at =
        |game: &Game, x: i32, y: i32| game.camera.to_screen(vec2(x as f32 + 0.5, y as f32 + 0.5));
    let first = at(game, tiles[0].0, tiles[0].1);
    let mut events = vec![RawEvent::MouseDown {
        button: MouseButton::Left,
        x: first.x,
        y: first.y,
    }];
    apply_events(game, input, &events);
    events.clear();
    for &(x, y) in &tiles[1..] {
        let p = at(game, x, y);
        apply_events(game, input, &[RawEvent::MouseMove { x: p.x, y: p.y }]);
    }
    let last = at(game, tiles[tiles.len() - 1].0, tiles[tiles.len() - 1].1);
    apply_events(
        game,
        input,
        &[RawEvent::MouseUp {
            button: MouseButton::Left,
            x: last.x,
            y: last.y,
        }],
    );
}

/// `drag_over` with the frame loop's heartbeat: pending drains into
/// the sim between pointer events, the way real drags actually run.
fn drag_over_ticking(game: &mut Game, input: &mut InputState, tiles: &[(i32, i32)]) -> usize {
    let at =
        |game: &Game, x: i32, y: i32| game.camera.to_screen(vec2(x as f32 + 0.5, y as f32 + 0.5));
    let mut rejections = 0;
    let mut drain = |game: &mut Game| {
        let commands = std::mem::take(&mut game.pending);
        let report = game.state.tick(&commands);
        rejections += report
            .events
            .iter()
            .filter(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
            .count();
    };
    let first = at(game, tiles[0].0, tiles[0].1);
    apply_events(
        game,
        input,
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: first.x,
            y: first.y,
        }],
    );
    drain(game);
    for &(x, y) in &tiles[1..] {
        let p = at(game, x, y);
        apply_events(game, input, &[RawEvent::MouseMove { x: p.x, y: p.y }]);
        drain(game);
    }
    let last = at(game, tiles[tiles.len() - 1].0, tiles[tiles.len() - 1].1);
    apply_events(
        game,
        input,
        &[RawEvent::MouseUp {
            button: MouseButton::Left,
            x: last.x,
            y: last.y,
        }],
    );
    drain(game);
    rejections
}

#[test]
fn a_ticking_drag_spends_the_whole_bank() {
    // Ten turrets, exactly funded — and the tick charging earlier
    // stamps mid-drag must not make the gate bill them twice (the
    // double-count cut a funded wall to half its length).
    let mut game = drag_arena(1000);
    let mut input = InputState::new();
    game.selection.units = vec![game.state.units()[0].id];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    // Two short rows bracketing the builder: every anchor stays inside
    // someone's sight even as the builder walks to its first site —
    // a wall drawn off into fog refuses honestly, which is a
    // different test.
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
        game.state.player(game.human).scrap,
        0,
        "the bank spends to exactly zero"
    );
}

#[test]
fn a_broke_opening_click_toasts_instead_of_pinging() {
    let mut game = drag_arena(50);
    let mut input = InputState::new();
    game.selection.units = vec![game.state.units()[0].id];
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
    game.selection.units = vec![game.state.units()[0].id];
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
    game.selection.units = vec![game.state.units()[0].id];
    input.placing = Some(oxide_sim::BuildingKind::Turret);
    drag_over(&mut game, &mut input, &[(9, 4), (10, 4), (11, 4)]);
    assert_eq!(staged_builds(&game), 1, "150 scrap affords one turret");
}

#[test]
fn a_shift_stroke_spends_only_the_builders_headroom() {
    let mut game = drag_arena(50_000);
    let mut input = InputState::new();
    let builder = game.state.units()[0].id;
    game.selection.units = vec![builder];
    // Pre-load the program through the sim: one active move plus 30
    // queued — headroom 2.
    let mut fill = vec![PlayerCommand {
        player: game.human,
        command: Command::Move {
            units: vec![builder],
            goal: TilePos::new(3, 7),
            queue: false,
        },
    }];
    for _ in 0..30 {
        fill.push(PlayerCommand {
            player: game.human,
            command: Command::Move {
                units: vec![builder],
                goal: TilePos::new(4, 7),
                queue: true,
            },
        });
    }
    game.state.tick(&fill);
    assert_eq!(game.state.unit(builder).unwrap().queue.len(), 30);

    input.placing = Some(oxide_sim::BuildingKind::Turret);
    apply_events(
        &mut game,
        &mut input,
        &[RawEvent::KeyDown { key: Key::Shift }],
    );
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
    game.selection.units = vec![builder];
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
    game.selection.units = vec![builder];
    // Fill the program to the brim in the SIM: one active order plus a
    // full queue — zero headroom for the stamp the click would append.
    game.state.tick(&[PlayerCommand {
        player: game.human,
        command: Command::Move {
            units: vec![builder],
            goal: TilePos::new(15, 2),
            queue: false,
        },
    }]);
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP {
        game.state.tick(&[PlayerCommand {
            player: game.human,
            command: Command::Move {
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
    let p = game.camera.to_screen(vec2(4.5, 2.5));
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
        ],
    );
    assert!(
        game.pending.is_empty(),
        "zero headroom refuses the opening stamp instead of pinging a doomed build"
    );
    assert!(
        game.sounds_pending
            .iter()
            .any(|(k, _)| matches!(k, crate::game::SoundKind::Denied)),
        "the refusal is audible"
    );
    assert!(input.placing.is_some(), "and the mode stays armed");
}

#[test]
fn a_fogged_leg_leaves_a_gap_in_the_waypoint_numbers() {
    let mut game = headless_game();
    let fighter = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.human && u.kind == UnitKind::Sentinel)
        .expect("skirmish authors a sentinel")
        .id;
    game.selection.units = vec![fighter];
    // First leg into unexplored ground (its goal draws nothing), then
    // a leg back onto explored home turf.
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
            player: game.human,
            command: Command::AttackMove {
                units: vec![fighter],
                goal: fogged,
                queue: false,
            },
        },
        PlayerCommand {
            player: game.human,
            command: Command::AttackMove {
                units: vec![fighter],
                goal: home,
                queue: true,
            },
        },
    ]);
    let unit = game.state.unit(fighter).unwrap();
    assert_eq!(unit.queue.len(), 1, "two-leg program");
    let points = crate::render::entities::breadcrumb_points(&game, unit);
    assert_eq!(points.len(), 1, "the fogged leg draws nothing");
    assert_eq!(
        points[0].0, 1,
        "the survivor wears its PROGRAM position — chip 2 is waypoint 2, \
         never renumbered down into chip 1's seat"
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
            "seed": 9,
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
    game.selection.units = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .map(|u| u.id)
        .collect();
    let subject = crate::panel::subject_unit(&game).expect("a subject");
    assert_eq!(
        game.selection.units.iter().position(|id| *id == subject),
        Some(12),
        "premise: the subject sits exactly past the old cap's cut"
    );
    let decor = crate::render::entities::decor_units(&game);
    assert_eq!(decor.len(), 12, "the cap holds");
    assert_eq!(decor[0], subject, "the subject draws first, never dropped");
    assert!(
        decor.iter().all(|id| game.selection.units.contains(id)),
        "decor only draws selected machines"
    );

    // A lone selection degrades to itself.
    let one = game.selection.units[0];
    game.selection.units = vec![one];
    assert_eq!(crate::render::entities::decor_units(&game), vec![one]);
}

#[test]
fn the_tutorial_survives_its_own_literal_instructions() {
    // The regression gate for the 150-scrap dead end: every lesson,
    // played exactly as its card words it (keyboard alternatives the
    // text itself offers, world clicks for the rest), must stay
    // affordable with the shipped numbers. If a cost change or a bank
    // change re-opens the trap, this fails before a player finds it.
    use crate::tutorial::{Tutorial, tutorial_scenario};

    let harvester_cost = UnitKind::Harvester.stats().cost;
    let turret_cost = BUILD_PALETTE[0]
        .stats()
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
    game.camera.center = vec2(8.0, 5.0);
    let key = |game: &mut Game, input: &mut InputState, key: Key| {
        apply_events(
            game,
            input,
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
        );
    };
    let right_click = |game: &mut Game, input: &mut InputState, world: Vec2| {
        let p = game.camera.to_screen(world);
        apply_events(
            game,
            input,
            &[RawEvent::MouseDown {
                button: MouseButton::Right,
                x: p.x,
                y: p.y,
            }],
        );
    };
    let bank = |game: &Game| game.state.player(game.human).scrap;

    // Lesson 1 — "or press H": the Foundry fallback trains a Harvester.
    assert!(t.advance(&game.demo));
    assert_eq!(t.step, 0);
    assert!(bank(&game) >= harvester_cost, "lesson 1 must be affordable");
    key(&mut game, &mut input, Key::H);
    game.do_tick();
    assert!(t.advance(&game.demo));
    assert_eq!(t.step, 1, "training graduates lesson 1");

    // Lesson 2 — select a harvester, right-click a scrap pile; the
    // card holds until the first load lands.
    let hauler = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
        .map(|u| (u.id, u.pos))
        .expect("a starting harvester");
    let p = game
        .camera
        .to_screen(vec2(hauler.1.x.to_num::<f32>(), hauler.1.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    assert_eq!(
        game.selection.units,
        vec![hauler.0],
        "the harvester is in hand"
    );
    right_click(&mut game, &mut input, vec2(7.5, 2.5)); // the home scrap pile
    game.do_tick();
    assert!(game.demo.harvested, "the order was accepted");
    assert!(t.advance(&game.demo));
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
    assert!(t.advance(&game.demo));
    assert_eq!(t.step, 2, "income graduates the mining lesson");

    // Lesson 3 — "Pick a DIFFERENT harvester": the hauler keeps its
    // program while an idle machine raises the palette's structure.
    assert!(bank(&game) >= turret_cost, "lesson 3 must be affordable");
    let idle = game
        .state
        .units()
        .iter()
        .find(|u| {
            u.player == game.human
                && u.kind == UnitKind::Harvester
                && matches!(u.order, oxide_sim::Order::Idle)
        })
        .map(|u| (u.id, u.pos))
        .expect("an idle harvester to build with");
    let p = game
        .camera
        .to_screen(vec2(idle.1.x.to_num::<f32>(), idle.1.y.to_num::<f32>()));
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    assert_eq!(game.selection.units.len(), 1, "one builder in hand");
    let picked = game.selection.units[0];
    assert!(
        game.state
            .unit(picked)
            .is_some_and(|u| !matches!(u.order, oxide_sim::Order::Harvest { .. })),
        "the literal reading leaves the hauler hauling"
    );
    key(&mut game, &mut input, Key::B);
    key(&mut game, &mut input, Key::Num1);
    let ground = game.camera.to_screen(vec2(10.5, 4.5));
    apply_events(&mut game, &mut input, &click(ground.x, ground.y));
    assert!(
        game.pending.iter().any(
            |c| matches!(&c.command, Command::Build { kind, .. } if *kind == BUILD_PALETTE[0])
        ),
        "B, digit, ground click staged the build: {:?}",
        game.pending
    );
    game.do_tick();
    assert!(t.advance(&game.demo));
    assert_eq!(t.step, 3, "the site graduates the building lesson");
    assert!(
        game.state
            .units()
            .iter()
            .any(|u| u.player == game.human && matches!(u.order, oxide_sim::Order::Harvest { .. })),
        "income survives the building lesson"
    );

    // Lesson 4 — the trap's teeth: the fighter must be payable here.
    assert!(
        bank(&game) >= sentinel_cost,
        "the fighter lesson re-opened the dead end: bank {} vs {} needed",
        bank(&game),
        sentinel_cost
    );
    key(&mut game, &mut input, Key::S); // train slot 2: the Sentinel
    game.do_tick();
    assert!(t.advance(&game.demo));
    assert_eq!(t.step, 4, "the fighter graduates the arming lesson");

    // Lesson 5 — right-click ground with a fighter selected.
    let sentinel = game
        .state
        .units()
        .iter()
        .find(|u| u.player == game.human && u.kind == UnitKind::Sentinel)
        .map(|u| (u.id, u.pos))
        .expect("the starting sentinel stands");
    let p = game.camera.to_screen(vec2(
        sentinel.1.x.to_num::<f32>(),
        sentinel.1.y.to_num::<f32>(),
    ));
    apply_events(&mut game, &mut input, &click(p.x, p.y));
    right_click(&mut game, &mut input, vec2(12.5, 9.5));
    game.do_tick();
    assert!(t.advance(&game.demo));
    assert_eq!(t.step, 5, "advance graduates the march lesson");

    // Lesson 6 is the pause menu, a frame-loop act outside the
    // command stream; its flag flips in main.rs.
    game.demo.paused_menu = true;
    assert!(!t.advance(&game.demo), "school is out");
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
        .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    let spot = TilePos::new(18, 4);
    // Scout the spot, then walk home so it stays explored but unseen.
    let walk = |game: &mut Game, goal: TilePos| {
        game.state.tick(&[PlayerCommand {
            player: game.human,
            command: Command::Move {
                units: vec![harvester],
                goal,
                queue: false,
            },
        }]);
    };
    walk(&mut game, TilePos::new(18, 5));
    for _ in 0..600 {
        if game.state.can_see(game.human, spot) {
            break;
        }
        game.state.tick(&[]);
    }
    assert!(
        game.state.can_see(game.human, spot),
        "scout reached the spot"
    );
    walk(&mut game, TilePos::new(7, 5));
    for _ in 0..600 {
        if !game.state.can_see(game.human, spot) {
            break;
        }
        game.state.tick(&[]);
    }
    assert!(!game.state.can_see(game.human, spot));
    assert!(game.state.vision(game.human).explored(spot));

    game.selection.units = vec![harvester];
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
    assert!(!game.state.vision(game.human).explored(dark));
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
        .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
        .expect("skirmish authors a harvester")
        .id;
    let kind = oxide_sim::BuildingKind::Fabricator;
    let first = TilePos::new(18, 4);
    let replacement = TilePos::new(19, 4);
    let cost = kind
        .stats()
        .construction
        .expect("fabricator is constructible")
        .cost;
    let scrap = game.state.player(game.human).scrap;
    assert!(
        cost <= scrap && scrap < cost.saturating_mul(2),
        "premise: the bank funds one fabricator, not both"
    );

    let walk = |game: &mut Game, goal: TilePos| {
        game.state.tick(&[PlayerCommand {
            player: game.human,
            command: Command::Move {
                units: vec![builder],
                goal,
                queue: false,
            },
        }]);
    };
    walk(&mut game, TilePos::new(19, 6));
    for _ in 0..600 {
        if [first, replacement].iter().all(|anchor| {
            let (w, h) = kind.stats().size;
            (0..h).all(|dy| (0..w).all(|dx| game.state.can_see(game.human, anchor.offset(dx, dy))))
        }) {
            break;
        }
        game.state.tick(&[]);
    }
    walk(&mut game, TilePos::new(7, 5));
    for _ in 0..600 {
        if [first, replacement].iter().all(|anchor| {
            let (w, h) = kind.stats().size;
            (0..h).all(|dy| (0..w).all(|dx| !game.state.can_see(game.human, anchor.offset(dx, dy))))
        }) {
            break;
        }
        game.state.tick(&[]);
    }
    for anchor in [first, replacement] {
        let (w, h) = kind.stats().size;
        for dy in 0..h {
            for dx in 0..w {
                let tile = anchor.offset(dx, dy);
                assert!(game.state.vision(game.human).explored(tile));
                assert!(!game.state.can_see(game.human, tile));
            }
        }
    }

    game.selection.units = vec![builder];
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

    let queued = pending_build_projection(&game, kind, replacement, true).funds;
    assert_eq!(queued.scrap, scrap);
    assert_eq!(
        queued.reserved, cost,
        "Shift preserves the pending claim and its reservation"
    );
    assert_eq!(
        placement_refusal(&game, kind, replacement, true),
        Some(oxide_sim::PlaceRefusal::Building),
        "Shift preserves the overlapping pending footprint"
    );
    let replacing = pending_build_projection(&game, kind, replacement, false).funds;
    assert_eq!(replacing.scrap, scrap);
    assert_eq!(
        replacing.reserved, 0,
        "a plain click replaces the pending claim before it can charge"
    );
    assert_eq!(
        placement_refusal(&game, kind, replacement, false),
        None,
        "the abandoned pending footprint no longer blocks its replacement"
    );

    build_click(&mut game, &mut input, kind, replacement);
    assert_eq!(
        game.pending.len(),
        2,
        "one-build bank accepts the replacement instead of billing both claims"
    );

    let scrap = game.state.player(game.human).scrap;
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
        game.state.player(game.human).scrap,
        scrap,
        "the founder is still walking, so neither deferred command charged"
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
            unit.player == game.human && unit.kind == UnitKind::Harvester && unit.id != builder
        })
        .expect("skirmish authors another harvester")
        .id;
    game.pending.push(PlayerCommand {
        player: game.human,
        command: Command::Stop {
            units: vec![builder],
        },
    });
    game.selection.units = vec![other_builder];
    assert_eq!(
        placement_refusal(&game, kind, first, false),
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
fn pending_projection_keeps_sites_but_stop_clears_unpaid_claims() {
    let mut game = drag_arena(500);
    let builder = game.state.units()[0].id;
    let kind = oxide_sim::BuildingKind::Turret;
    let anchor = TilePos::new(4, 2);
    let cost = kind
        .stats()
        .construction
        .expect("turret is constructible")
        .cost;
    game.selection.units = vec![builder];
    game.pending.push(PlayerCommand {
        player: game.human,
        command: Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer: false,
        },
    });
    let committed = pending_build_projection(&game, kind, anchor, false).funds;
    assert_eq!(
        committed.scrap,
        500 - cost,
        "an immediate site charges before its builder is reprogrammed"
    );
    assert_eq!(committed.reserved, 0);
    assert_eq!(
        placement_refusal(&game, kind, anchor, false),
        Some(oxide_sim::PlaceRefusal::Building),
        "an immediate site's ground stays committed after replacement"
    );

    game.pending.clear();
    game.pending.push(PlayerCommand {
        player: game.human,
        command: Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer: true,
        },
    });
    assert_eq!(
        pending_build_projection(&game, kind, anchor, true).funds,
        PendingBuildFunds {
            scrap: 500,
            reserved: cost,
        }
    );
    assert_eq!(
        placement_refusal(&game, kind, anchor, true),
        Some(oxide_sim::PlaceRefusal::Building)
    );
    game.pending.push(PlayerCommand {
        player: game.human,
        command: Command::Patrol {
            units: vec![builder],
            waypoints: Vec::new(),
        },
    });
    assert_eq!(
        pending_build_projection(&game, kind, anchor, true).funds,
        PendingBuildFunds {
            scrap: 500,
            reserved: cost,
        },
        "a rejected pending command cannot release the claim"
    );
    assert_eq!(
        placement_refusal(&game, kind, anchor, true),
        Some(oxide_sim::PlaceRefusal::Building)
    );
    game.pending.pop();

    game.pending.push(PlayerCommand {
        player: game.human,
        command: Command::Stop {
            units: vec![builder],
        },
    });
    assert_eq!(
        pending_build_projection(&game, kind, anchor, true).funds,
        PendingBuildFunds {
            scrap: 500,
            reserved: 0,
        },
        "Stop clears the deferred promise before it can charge"
    );
    assert_eq!(
        placement_refusal(&game, kind, anchor, true),
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
        .filter(|unit| unit.player == game.human && unit.kind == UnitKind::Harvester)
        .map(|unit| unit.id)
        .take(2)
        .collect();
    assert_eq!(workers.len(), 2, "skirmish authors two human workers");
    let kind = oxide_sim::BuildingKind::Turret;
    let anchor = TilePos::new(10, 4);
    let cost = kind
        .stats()
        .construction
        .expect("turret is constructible")
        .cost;
    let scrap = game.state.player(game.human).scrap;
    game.pending.extend([
        PlayerCommand {
            player: game.human,
            command: Command::Build {
                units: vec![workers[0]],
                kind,
                anchor,
                queue: false,
                defer: true,
            },
        },
        PlayerCommand {
            player: game.human,
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
            assert!(
                projected.has_own_unfinished_site(game.human, kind, anchor),
                "the later immediate command paid for the deferred claim's site"
            );
        });

    game.selection.units = vec![workers[1]];
    let projection = pending_build_projection_for(&game, kind, TilePos::new(13, 4), true, false);
    assert_eq!(
        projection.funds,
        PendingBuildFunds {
            scrap: scrap - cost,
            reserved: 0,
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
        .filter(|unit| unit.player == game.human && unit.kind == UnitKind::Harvester)
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
        player: game.human,
        command: Command::Move {
            units: vec![workers[0]],
            goal: far,
            queue: false,
        },
    }];
    for _ in 0..oxide_sim::stats::ORDER_QUEUE_CAP {
        fill.push(PlayerCommand {
            player: game.human,
            command: Command::Move {
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
        .filter(|&tile| game.state.can_place(game.human, kind, tile))
        .min_by_key(|tile| (tile.x - low.x).abs() + (tile.y - low.y).abs())
        .expect("visible reachable ground remains");
    game.selection.units = workers.clone();
    assert!(
        pending_build_projection_for(&game, kind, anchor, true, true).queue_has_room,
        "deferred construction succeeds when any selected worker can take the claim"
    );
    assert!(
        !pending_build_projection_for(&game, kind, anchor, true, false).queue_has_room,
        "immediate construction is still gated by the lowest-id founder"
    );

    let command = |defer| PlayerCommand {
        player: game.human,
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
        .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
        .expect("skirmish authors a harvester")
        .id;
    let kind = oxide_sim::BuildingKind::Fabricator;
    let old_spot = TilePos::new(10, 4);
    let new_spot = TilePos::new(11, 4);
    for spot in [old_spot, new_spot] {
        assert!(
            game.state
                .place_intent_refusal(game.human, kind, spot)
                .is_none(),
            "premise: the visible site starts open"
        );
    }
    let report = game.state.tick(&[PlayerCommand {
        player: game.human,
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
    assert!(matches!(
        game.state.unit(builder).unwrap().order,
        oxide_sim::Order::Found {
            kind: claimed_kind,
            anchor,
        } if claimed_kind == kind && anchor == old_spot
    ));
    assert!(
        game.state.buildings().iter().all(|b| b.anchor != old_spot),
        "the founder is still walking, so only its claim occupies the ground"
    );

    game.selection.units = vec![builder];
    input.placing = Some(kind);
    game.camera.center = vec2(new_spot.x as f32 + 0.5, new_spot.y as f32 + 0.5);
    game.camera.pan(Vec2::ZERO);
    let p = game
        .camera
        .to_screen(vec2(new_spot.x as f32 + 0.5, new_spot.y as f32 + 0.5));

    assert_eq!(
        placement_refusal(&game, kind, new_spot, true),
        Some(oxide_sim::PlaceRefusal::Building),
        "Shift appends, so the live claim remains a blocker"
    );
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyDown { key: Key::Shift },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: p.x,
                y: p.y,
            },
            RawEvent::KeyUp { key: Key::Shift },
        ],
    );
    assert!(
        game.pending.is_empty(),
        "the conservative Shift preflight stages no duplicate claim"
    );
    assert!(input.placing.is_some(), "the refused click stays armed");

    assert_eq!(
        placement_refusal(&game, kind, new_spot, false),
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
