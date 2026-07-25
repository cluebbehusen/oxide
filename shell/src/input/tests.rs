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
        ],
    );
    assert_eq!(game.pending.len(), 1, "legal ground stages the site");
    assert!(input.placing.is_some(), "shift keeps the wall going up");

    // A plain click disarms after staging.
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
            {\"name\": \"C\", \"faction\": \"cupric\", \"scrap\": 100, \"bot\": false}
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
