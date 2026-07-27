//! Integration tests for the input funnel: real events through the real
//! resolver against a real (headless) sim.

use super::*;
use oxide_sim::UnitKind;

fn headless_game() -> Game {
    Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0))
        .expect("embedded skirmish builds")
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

    // A plain click (press AND release — the mode now settles at the
    // release, where the placement drag ends) disarms after staging.
    apply_events(
        &mut game,
        &mut input,
        &[
            RawEvent::KeyUp { key: Key::Shift },
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
fn a_right_click_on_ground_stages_an_attack_move() {
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
            .any(|c| matches!(c.command, Command::AttackMove { .. })),
        "fire-at-will ground order staged: {:?}",
        game.pending
    );
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
            {\"name\": \"foe\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": true}
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
    // its order chips.
    let panel = crate::panel::build(&game, &input.bindings).expect("a panel");
    assert!(panel.cards.is_empty(), "no verbs on an ally panel");
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

    // hp and kind only: zero cards, zero chips — order state is intent
    // the fog never licensed.
    let panel = crate::panel::build(&game, &input.bindings).expect("a panel");
    assert!(panel.cards.is_empty(), "no verbs on a hostile panel");
    assert!(panel.queue.is_empty(), "no order chips on a hostile panel");

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
            .any(|c| matches!(c.command, Command::AttackMove { .. })),
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
    game.selection.building = Some(own);
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
        game.selection.building,
        Some(own),
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
        game.selection.building.is_none(),
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
        game.selection.building,
        Some(ally_foundry),
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
            .any(|c| matches!(c.command, Command::AttackMove { .. })),
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
fn the_type_strip_cuts_a_mixed_selection_both_ways() {
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
        .cards
        .iter()
        .filter_map(|c| match c.action {
            crate::panel::CardAction::FilterKind(k) => Some((k, c.title.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(strip.len(), 2, "two kinds, two counted cards: {strip:?}");
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
