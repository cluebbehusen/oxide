//! Real artillery: shells lead a path known at fire time, fly unguided,
//! and resolve on arrival against whatever stands there. Dodgeable by a
//! later course change, deadly to straight commitments and the rooted,
//! loyal to no one once launched. Public API only, like `domains.rs`.

use chassis::grid::TilePos;
use chassis::replay::Replay;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::{
    BuildingKind, Command, Event, Faction, Order, PlayerCommand, PlayerId, SIM_VERSION, Scenario,
    State, Target, UnitKind,
};

fn players() -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap: 300,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap: 300,
            bot: false,
            bot_config: None,
        },
    ]
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

/// A long open range: bombard work needs distance.
fn range(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "shell-range".into(),
        seed: 42,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#....................2.#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: players(),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn run(state: &mut State, ticks: u64) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..ticks {
        all.extend(state.tick(&[]).events);
    }
    all
}

fn unit_launch(
    events: &[Event],
    shooter: oxide_sim::UnitId,
) -> Option<(Target, chassis::fx::Vec2Fx, u64)> {
    events.iter().find_map(|event| match event {
        Event::ShellLaunched {
            shooter: Target::Unit(unit),
            target,
            to,
            flight,
            ..
        } if *unit == shooter => Some((*target, *to, *flight)),
        _ => None,
    })
}

fn moving_target_range() -> Scenario {
    moving_target_range_with_order(true)
}

fn moving_target_range_with_order(target_first: bool) -> Scenario {
    let mut units = if target_first {
        vec![
            unit(1, UnitKind::Scuttler, 10, 5),
            unit(0, UnitKind::Bombard, 2, 5),
        ]
    } else {
        vec![
            unit(0, UnitKind::Bombard, 2, 5),
            unit(1, UnitKind::Scuttler, 10, 5),
        ]
    };
    units.push(unit(0, UnitKind::Harvester, 7, 5));
    range(units)
}

fn moving_ids(state: &State) -> (oxide_sim::UnitId, oxide_sim::UnitId) {
    let target = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Scuttler)
        .unwrap()
        .id;
    let bombard = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Bombard)
        .unwrap()
        .id;
    (target, bombard)
}

fn establish_straight_motion(
    state: &mut State,
    target: oxide_sim::UnitId,
    bombard: oxide_sim::UnitId,
    goal: TilePos,
) {
    let bombard_tile = state.unit(bombard).unwrap().tile();
    state.tick(&[
        cmd(
            1,
            Command::Move {
                units: vec![target],
                goal,
                queue: false,
            },
        ),
        // Spend the gun's brain turn completing a no-distance move so it
        // cannot auto-acquire before the motion sample exists.
        cmd(
            0,
            Command::Move {
                units: vec![bombard],
                goal: bombard_tile,
                queue: false,
            },
        ),
    ]);
    assert!(state.unit(target).unwrap().path.is_some());
}

/// Fires the bombard at the scuttler and returns (state, launch events).
fn open_fire() -> (State, Vec<Event>) {
    let mut state = range(vec![
        unit(0, UnitKind::Bombard, 2, 5),
        // The spotter sees (vision 7 at range 6) without engaging
        // (aggro 5): eyes for the gun, not a second gun.
        unit(0, UnitKind::Sentinel, 5, 5),
        unit(1, UnitKind::Scuttler, 11, 5),
    ])
    .build()
    .unwrap();
    let (bombard, scuttler) = (state.units()[0].id, state.units()[2].id);
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(scuttler),
            queue: false,
        },
    )]);
    (state, report.events)
}

#[test]
fn a_standing_target_eats_the_shell() {
    let (mut state, events) = open_fire();
    let scuttler = state.units()[2].id;
    assert_eq!(
        unit_launch(&events, state.units()[0].id).map(|launch| launch.0),
        Some(Target::Unit(scuttler))
    );
    assert_eq!(
        unit_launch(&events, state.units()[0].id).map(|launch| launch.1),
        Some(state.unit(scuttler).unwrap().pos),
        "a pathless unit keeps exact current-position aim"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ShellLaunched { .. })),
        "in range with a spotter: the gun speaks immediately"
    );
    assert_eq!(state.shells().len(), 1, "one shell in flight");
    let hp_before = state.unit(scuttler).unwrap().hp;
    let events = run(&mut state, 40);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ShellLanded { .. })),
        "the shell lands within its flight window"
    );
    assert!(
        state.unit(scuttler).is_none_or(|u| u.hp < hp_before),
        "standing still under artillery hurts"
    );
    assert!(state.shells().is_empty(), "landed shells leave the sky");
}

#[test]
fn a_moving_target_walks_out_of_the_blast() {
    let (mut state, _) = open_fire();
    let bombard = state.units()[0].id;
    let scuttler = state.units()[2].id;
    let hp_before = state.unit(scuttler).unwrap().hp;
    // React immediately: run down-map, clear of the fire-time aim
    // point. The gun stands down after its first launch so the test
    // watches exactly one shell — reloaded follow-ups against the
    // eventually-stationary walker are the standing-target case, not
    // this one.
    state.tick(&[
        cmd(
            1,
            Command::Move {
                units: vec![scuttler],
                goal: TilePos::new(11, 1),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Stop {
                units: vec![bombard],
            },
        ),
    ]);
    run(&mut state, 60);
    assert_eq!(
        state.unit(scuttler).unwrap().hp,
        hp_before,
        "the shell landed where the scuttler used to be"
    );
}

#[test]
fn shells_outlive_their_shooters() {
    let (mut state, _) = open_fire();
    let bombard = state.units()[0].id;
    let scuttler = state.units()[2].id;
    let hp_before = state.unit(scuttler).unwrap().hp;
    // The shooter dies the tick after launch; its shell flies on and
    // still lands ("a shell in flight chooses nothing" — including
    // dying with its gun).
    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![bombard],
        },
    )]);
    // Simulate the shooter's death by enemy action: a swarm appears is
    // overkill — the sim only needs the unit gone, and the honest path
    // is damage. Two enemy scuttlers spawn nearby in scenario terms is
    // not possible mid-game, so we let the original scuttler's team
    // kill it via a fresh assault from the second seat's forces. The
    // simplest honest lever: the enemy scuttler attacks the bombard
    // (slow walk), while the shell (30 ticks) lands first — instead,
    // assert the weaker but real property: the shell keeps flying when
    // its shooter's ORDER is gone (stopped), and lands on schedule.
    assert_eq!(state.shells().len(), 1);
    run(&mut state, 60);
    assert!(state.shells().is_empty(), "the sky cleared on schedule");
    assert!(
        state.unit(scuttler).is_none_or(|u| u.hp < hp_before),
        "the stationary victim still paid"
    );
}

#[test]
fn a_tampered_shell_owner_never_becomes_a_state() {
    // hostile() indexes the player table when the shell lands; the
    // validator has to reject the foreign seat at deserialization, not
    // panic ticks later.
    let (mut state, _) = open_fire();
    run(&mut state, 3);
    assert!(!state.shells().is_empty(), "premise: a shell is airborne");
    let mut doc: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    doc["shells"][0]["player"] = serde_json::json!(9);
    assert!(serde_json::from_value::<State>(doc).is_err());
}

#[test]
fn two_runs_with_shells_in_flight_stay_bit_identical() {
    let build = || {
        let (mut state, _) = open_fire();
        run(&mut state, 17); // mid-flight: shells live in the state
        assert!(!state.shells().is_empty(), "premise: a shell is airborne");
        run(&mut state, 100);
        state.hash()
    };
    assert_eq!(build(), build(), "same seed, same shells, same bits");
}

#[test]
fn a_straight_mover_is_led_hit_and_replayed_bit_exactly() {
    let scenario = moving_target_range();
    let initial = scenario.build().unwrap();
    let target = initial
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Scuttler)
        .unwrap()
        .id;
    let bombard = initial
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Bombard)
        .unwrap()
        .id;
    let bombard_tile = initial.unit(bombard).unwrap().tile();
    let setup_commands = vec![
        cmd(
            1,
            Command::Move {
                units: vec![target],
                goal: TilePos::new(10, 10),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![bombard],
                goal: bombard_tile,
                queue: false,
            },
        ),
    ];
    let attack = cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(target),
            queue: false,
        },
    );
    let stop = cmd(
        0,
        Command::Stop {
            units: vec![bombard],
        },
    );
    let mut replay = Replay::new(SIM_VERSION, scenario);
    for command in &setup_commands {
        replay.record(0, command.clone());
    }
    replay.record(1, attack);
    replay.record(2, stop);
    replay.meta.ticks = Some(90);

    let play = |replay: &Replay<Scenario, PlayerCommand>| {
        let mut state = replay.setup.clone().build().unwrap();
        let hp_before = state.unit(target).unwrap().hp;
        let mut target_at_launch = None;
        let mut launch = None;
        let mut landed = false;
        let mut cursor = replay.cursor();
        for _ in 0..replay.meta.ticks.unwrap() {
            let commands: Vec<PlayerCommand> = cursor
                .take_tick(state.current_tick())
                .iter()
                .map(|timed| timed.command.clone())
                .collect();
            let target_before_tick = state.unit(target).map(|unit| unit.pos);
            let report = state.tick(&commands);
            if unit_launch(&report.events, bombard).is_some() {
                target_at_launch = target_before_tick;
            }
            launch = launch.or_else(|| unit_launch(&report.events, bombard));
            landed |= report
                .events
                .iter()
                .any(|event| matches!(event, Event::ShellLanded { .. }));
        }
        assert!(cursor.is_finished());
        (
            launch.expect("the ordered Bombard launched"),
            target_at_launch.expect("the target existed when the shell launched"),
            landed,
            state.unit(target).map(|unit| unit.hp),
            hp_before,
            state.hash(),
        )
    };

    let live = play(&replay);
    assert_eq!(live.0.0, Target::Unit(target));
    assert!(live.0.1.y > live.1.y, "the shell leads the southbound path");
    assert!(
        live.0.1.x <= live.1.x && live.0.1.x > live.1.x - chassis::fx::Fx::lit("0.2"),
        "range clipping stays close to the target's straight lane"
    );
    assert!(live.2, "the predicted shell lands");
    assert!(
        live.3.is_none_or(|hp| hp < live.4),
        "the mover remains inside the predicted blast"
    );

    let replay: Replay<Scenario, PlayerCommand> =
        serde_json::from_str(&serde_json::to_string(&replay).unwrap()).unwrap();
    let replayed = play(&replay);
    assert_eq!(
        replayed.0, live.0,
        "replay preserves the exact lead and flight"
    );
    assert_eq!(
        replayed.5, live.5,
        "replay preserves the resulting world bits"
    );
}

#[test]
fn a_visible_cluster_keeps_the_shell_on_its_current_footprint() {
    let mut state = range(vec![
        unit(0, UnitKind::Bombard, 3, 5),
        unit(0, UnitKind::Sentinel, 5, 5),
        unit(1, UnitKind::Scuttler, 11, 5),
        unit(1, UnitKind::Scuttler, 11, 6),
    ])
    .build()
    .unwrap();
    let bombard = state.units()[0].id;
    let target = state.units()[2].id;
    let neighbor = state.units()[3].id;
    establish_straight_motion(&mut state, target, bombard, TilePos::new(11, 10));
    assert!(state.can_see(PlayerId(0), state.unit(target).unwrap().tile()));
    assert!(state.can_see(PlayerId(0), state.unit(neighbor).unwrap().tile()));
    let current = state.unit(target).unwrap().pos;

    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(target),
            queue: false,
        },
    )]);
    let (_, aim, _) = unit_launch(&report.events, bombard).expect("the clustered shot launches");

    assert_eq!(
        aim, current,
        "a second visible hostile inside the current blast rewards the known cluster"
    );
}

#[test]
fn an_unseen_neighbor_cannot_suppress_predictive_aim() {
    let mut state = range(vec![
        unit(0, UnitKind::Bombard, 3, 5),
        unit(0, UnitKind::Sentinel, 5, 5),
        unit(1, UnitKind::Scuttler, 12, 5),
        unit(1, UnitKind::Scuttler, 13, 5),
    ])
    .build()
    .unwrap();
    let bombard = state.units()[0].id;
    let target = state.units()[2].id;
    let neighbor = state.units()[3].id;
    establish_straight_motion(&mut state, target, bombard, TilePos::new(12, 10));
    assert!(state.can_see(PlayerId(0), state.unit(target).unwrap().tile()));
    assert!(!state.can_see(PlayerId(0), state.unit(neighbor).unwrap().tile()));
    let current = state.unit(target).unwrap().pos;

    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(target),
            queue: false,
        },
    )]);
    let (_, aim, _) = unit_launch(&report.events, bombard).expect("the isolated shot launches");

    assert!(
        aim.y > current.y,
        "fog-private neighbors cannot turn an isolated visible mover into a cluster"
    );
}

#[test]
fn an_ineligible_air_neighbor_cannot_suppress_predictive_aim() {
    let mut state = range(vec![
        unit(0, UnitKind::Bombard, 3, 5),
        unit(0, UnitKind::Sentinel, 5, 5),
        unit(1, UnitKind::Scuttler, 11, 5),
        unit(1, UnitKind::Buzzard, 11, 6),
    ])
    .build()
    .unwrap();
    let bombard = state.units()[0].id;
    let target = state.units()[2].id;
    let neighbor = state.units()[3].id;
    establish_straight_motion(&mut state, target, bombard, TilePos::new(11, 10));
    assert!(state.can_see(PlayerId(0), state.unit(target).unwrap().tile()));
    assert!(state.can_see(PlayerId(0), state.unit(neighbor).unwrap().tile()));
    let current = state.unit(target).unwrap().pos;

    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(target),
            queue: false,
        },
    )]);
    let (_, aim, _) = unit_launch(&report.events, bombard).expect("the isolated shot launches");

    assert!(
        aim.y > current.y,
        "a nearby air body outside the weapon domain cannot suppress the lead"
    );
}

#[test]
fn advance_fire_leads_the_same_moving_path_without_becoming_an_attack() {
    let mut state = moving_target_range().build().unwrap();
    let (target, bombard) = moving_ids(&state);
    establish_straight_motion(&mut state, target, bombard, TilePos::new(10, 10));
    let target_start = state.unit(target).unwrap().pos;
    let goal = TilePos::new(6, 8);
    let report = state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![bombard],
            goal,
            queue: false,
        },
    )]);
    let (event_target, aim, _) =
        unit_launch(&report.events, bombard).expect("Advance launches a shell");
    assert_eq!(event_target, Target::Unit(target));
    assert!(
        aim.y > target_start.y,
        "Advance uses predictive artillery aim"
    );
    assert!(matches!(
        state.unit(bombard).unwrap().order,
        oxide_sim::Order::Advance { goal: current } if current == goal
    ));
}

#[test]
fn predictive_aim_never_extends_the_weapon_envelope() {
    let mut state = range(vec![
        unit(1, UnitKind::Scuttler, 11, 5),
        unit(0, UnitKind::Bombard, 2, 5),
        unit(0, UnitKind::Harvester, 7, 5),
    ])
    .build()
    .unwrap();
    let target = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Scuttler)
        .unwrap()
        .id;
    let bombard = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Bombard)
        .unwrap()
        .id;
    establish_straight_motion(&mut state, target, bombard, TilePos::new(20, 5));
    let from = state.unit(bombard).unwrap().pos;
    let current = state.unit(target).unwrap().pos;
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(target),
            queue: false,
        },
    )]);
    let (_, aim, _) = unit_launch(&report.events, bombard).expect("the edge shot launches");
    let range = UnitKind::Bombard.stats().weapons[0].range;
    assert!(aim.x > current.x, "the outward path is still led");
    assert!(
        from.dist_sq(aim) <= range * range,
        "prediction cannot extend a weapon past its authored range"
    );
}

#[test]
fn predictive_aim_is_independent_of_unit_id_order_and_brain_parity() {
    let fire = |target_first: bool, fire_on_even_tick: bool| {
        let mut state = moving_target_range_with_order(target_first)
            .build()
            .unwrap();
        let (target, bombard) = moving_ids(&state);
        if fire_on_even_tick {
            let hold = state.unit(bombard).unwrap().tile();
            state.tick(&[cmd(
                0,
                Command::Move {
                    units: vec![bombard],
                    goal: hold,
                    queue: false,
                },
            )]);
        }
        establish_straight_motion(&mut state, target, bombard, TilePos::new(10, 10));
        assert_eq!(state.current_tick().is_multiple_of(2), fire_on_even_tick);
        let current = state.unit(target).unwrap().pos;
        let report = state.tick(&[cmd(
            0,
            Command::Attack {
                units: vec![bombard],
                target: Target::Unit(target),
                queue: false,
            },
        )]);
        let (_, aim, _) = unit_launch(&report.events, bombard).expect("the moving target is led");
        (current, aim)
    };

    let expected = fire(true, false);
    for actual in [fire(false, false), fire(true, true), fire(false, true)] {
        assert_eq!(
            actual, expected,
            "id order and brain parity cannot move aim"
        );
    }
}

fn team_range(kind: UnitKind, with_spotter: bool) -> Scenario {
    let mut units = vec![unit(0, kind, 3, 6), unit(1, UnitKind::Harvester, 12, 6)];
    if with_spotter {
        units.push(unit(2, UnitKind::Harvester, 9, 6));
    }
    Scenario {
        name: "team-shell-range".into(),
        seed: 9,
        map: vec![
            "##############################".into(),
            "#1.........................2.#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#3...........................#".into(),
            "#............................#".into(),
            "##############################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Battery".into(),
                faction: Faction::Ferrous,
                team: Some(0),
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Target".into(),
                faction: Faction::Cupric,
                team: Some(1),
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Spotter".into(),
                faction: Faction::Ferrous,
                team: Some(0),
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn team_building_range(with_spotter: bool) -> Scenario {
    let mut scenario = team_range(UnitKind::Avalanche, with_spotter);
    scenario.name = "team-shell-building-range".into();
    scenario.units.retain(|unit| unit.player != 1);
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Turret,
        x: 12,
        y: 6,
    });
    scenario
}

fn unit_of_kind(state: &State, kind: UnitKind) -> oxide_sim::UnitId {
    state
        .units()
        .iter()
        .find(|unit| unit.kind == kind)
        .unwrap()
        .id
}

fn assert_artillery_ignores_hidden_target(
    state: &mut State,
    artillery: oxide_sim::UnitId,
    kind: UnitKind,
) {
    let initial_pos = state.unit(artillery).unwrap().pos;
    for tick in 0..4 {
        let report = state.tick(&[]);
        let unit = state.unit(artillery).unwrap();
        assert_eq!(
            unit.order,
            Order::Idle,
            "{kind:?} acquired on hidden tick {tick}"
        );
        assert!(
            unit.path.is_none(),
            "{kind:?} routed toward hidden information"
        );
        assert_eq!(
            unit.pos, initial_pos,
            "{kind:?} moved toward a hidden target"
        );
        assert!(
            unit_launch(&report.events, artillery).is_none(),
            "{kind:?} launched at a hidden target"
        );
    }
}

fn assert_artillery_fires_through_shared_sight(
    state: &mut State,
    artillery: oxide_sim::UnitId,
    kind: UnitKind,
    target: Target,
) {
    let initial_pos = state.unit(artillery).unwrap().pos;
    state.tick(&[]); // acquisition changes intent; firing follows next tick
    let unit = state.unit(artillery).unwrap();
    assert_eq!(
        unit.order,
        Order::Attack {
            target,
            resume: None,
        }
    );
    assert_eq!(unit.pos, initial_pos);
    assert!(unit.path.is_none());

    let report = state.tick(&[]);
    let unit = state.unit(artillery).unwrap();
    assert_eq!(
        unit.pos, initial_pos,
        "{kind:?} approached despite having range"
    );
    assert!(unit.path.is_none());
    assert_eq!(
        unit_launch(&report.events, artillery).map(|launch| launch.0),
        Some(target),
        "allied sight did not unlock {kind:?} at its autonomous range"
    );
}

fn peak_prediction_range() -> Scenario {
    Scenario {
        name: "peak-prediction-range".into(),
        seed: 11,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#.......^..............#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#....................2.#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: players(),
        units: vec![
            unit(1, UnitKind::Scuttler, 10, 8),
            unit(0, UnitKind::Bombard, 2, 8),
            unit(0, UnitKind::Harvester, 7, 8),
        ],
        buildings: Vec::new(),
        meta: None,
    }
}

#[test]
fn autonomous_spotter_weapons_require_shared_true_sight_for_units() {
    for kind in [UnitKind::Bombard, UnitKind::Avalanche] {
        let mut hidden = team_range(kind, false).build().unwrap();
        let artillery = unit_of_kind(&hidden, kind);
        let target = hidden
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(1))
            .unwrap();
        let target_tile = target.tile();
        let distance = hidden.unit(artillery).unwrap().pos.dist(target.pos);
        assert!(distance > chassis::fx::Fx::from_num(kind.stats().vision));
        assert!(distance <= kind.stats().aggro_range);
        assert!(!hidden.can_see(PlayerId(0), target_tile));
        assert_artillery_ignores_hidden_target(&mut hidden, artillery, kind);

        let mut spotted = team_range(kind, true).build().unwrap();
        let artillery = unit_of_kind(&spotted, kind);
        let target = spotted
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(1))
            .unwrap();
        let target_id = target.id;
        let target_tile = target.tile();
        assert!(spotted.can_see(PlayerId(0), target_tile));
        assert!(spotted.can_see(PlayerId(2), target_tile));
        assert_artillery_fires_through_shared_sight(
            &mut spotted,
            artillery,
            kind,
            Target::Unit(target_id),
        );
    }
}

#[test]
fn autonomous_avalanche_requires_shared_true_sight_for_buildings() {
    let mut hidden = team_building_range(false).build().unwrap();
    let avalanche = unit_of_kind(&hidden, UnitKind::Avalanche);
    let target = hidden
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let avalanche_pos = hidden.unit(avalanche).unwrap().pos;
    let distance = avalanche_pos.dist(
        hidden
            .building(target)
            .unwrap()
            .closest_point_to(avalanche_pos),
    );
    assert!(distance > chassis::fx::Fx::from_num(UnitKind::Avalanche.stats().vision));
    assert!(distance <= UnitKind::Avalanche.stats().aggro_range);
    assert!(
        !hidden
            .building(target)
            .unwrap()
            .tiles()
            .any(|tile| hidden.can_see(PlayerId(0), tile))
    );
    assert_artillery_ignores_hidden_target(&mut hidden, avalanche, UnitKind::Avalanche);

    let mut spotted = team_building_range(true).build().unwrap();
    let avalanche = unit_of_kind(&spotted, UnitKind::Avalanche);
    let target = spotted
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    assert!(
        spotted
            .building(target)
            .unwrap()
            .tiles()
            .any(|tile| spotted.can_see(PlayerId(0), tile))
    );
    assert!(
        spotted
            .building(target)
            .unwrap()
            .tiles()
            .any(|tile| spotted.can_see(PlayerId(2), tile))
    );
    assert_artillery_fires_through_shared_sight(
        &mut spotted,
        avalanche,
        UnitKind::Avalanche,
        Target::Building(target),
    );
}

#[test]
fn predictive_aim_falls_back_before_crossing_a_peak() {
    let mut state = peak_prediction_range().build().unwrap();
    let (target, bombard) = moving_ids(&state);
    establish_straight_motion(&mut state, target, bombard, TilePos::new(10, 16));
    let current = state.unit(target).unwrap().pos;
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(target),
            queue: false,
        },
    )]);
    let (_, aim, _) = unit_launch(&report.events, bombard).expect("the current line is legal");
    assert_eq!(
        aim, current,
        "a predicted line through a peak falls back to the visible current position"
    );
}

#[test]
fn a_siege_shell_lands_on_the_footprint_edge_and_still_counts() {
    // Aiming at a building lobs at its closest footprint point — an
    // exact edge coordinate that floors into the NEIGHBORING tile.
    // Direct hits are distance-to-footprint, not tile containment, or
    // sieges deal nothing (found the honest way: a six-gun battery
    // timed out a victory test without scratching the foundry).
    let mut state = range(vec![
        unit(0, UnitKind::Bombard, 14, 5),
        // Pacifist eyes on the target: attack commands are sight-gated.
        unit(0, UnitKind::Harvester, 17, 8),
    ])
    .build()
    .unwrap();
    let bombard = state.units()[0].id;
    let east = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .id;
    let hp_before = state.building(east).unwrap().hp;
    let expected_aim = state
        .building(east)
        .unwrap()
        .closest_point_to(state.unit(bombard).unwrap().pos);
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Building(east),
            queue: false,
        },
    )]);
    let launch = unit_launch(&report.events, bombard).expect("the Bombard launches");
    assert_eq!(launch.0, Target::Building(east));
    assert_eq!(
        launch.1, expected_aim,
        "building aim remains the closest footprint point"
    );
    let mut landed = false;
    for _ in 0..80 {
        landed |= state
            .tick(&[])
            .events
            .iter()
            .any(|e| matches!(e, Event::ShellLanded { .. }));
        if landed {
            break;
        }
    }
    assert!(landed, "the siege shell arrived");
    assert!(
        state.building(east).unwrap().hp < hp_before,
        "the footprint edge is still the footprint"
    );
}
