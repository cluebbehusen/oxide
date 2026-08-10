//! Pit terrain: the bottomless excavation. Ground can never cross or
//! stand on it, the sky above it is open, and fire of every kind —
//! direct, anti-air, artillery arcs — crosses it freely: machines trade
//! shots over a void neither can walk. Wrecks that fall in are gone.
//! Vision deliberately ignores it, like all terrain.

use chassis::grid::TilePos;
use oxide_sim::map::Terrain;
use oxide_sim::scenario::{PlayerSpec, ScenarioError, UnitSpec};
use oxide_sim::stats::Domain;
use oxide_sim::{
    Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind,
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

/// A chasm at x=`wall_x` (default 12) severing east from west on the
/// ground. Both Foundries sit west — the build gate requires the seats
/// ground-connected, so the east half hosts only orphaned machines.
/// `width` widens the chasm eastward for range experiments.
fn chasm(width: i32, units: Vec<UnitSpec>) -> Scenario {
    let mut map = vec!["#".repeat(24)];
    for y in 1..11 {
        let mut row = String::from("#");
        for x in 1..23 {
            row.push(match (x, y) {
                (1, 1) => '1',
                (1, 9) => '2',
                (x, _) if (12..12 + width).contains(&x) => '~',
                _ => '.',
            });
        }
        row.push('#');
        map.push(row);
    }
    map.push("#".repeat(24));
    Scenario {
        name: "chasm".into(),
        seed: 9,
        map,
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

#[test]
fn the_tilde_parses_renders_and_splits_the_domains() {
    let state = chasm(1, vec![]).build().unwrap();
    let pit = TilePos::new(12, 5);
    assert_eq!(state.map().tile(pit).unwrap().terrain, Terrain::Pit);
    assert!(!state.passable(pit), "no ground machine stands over a void");
    assert!(
        state.passable_for(Domain::Air, pit),
        "the sky over a pit is open"
    );
    let rows = state.map().ascii_rows();
    assert_eq!(
        rows[5].chars().nth(12),
        Some('~'),
        "the glyph round-trips through ascii_rows"
    );
}

#[test]
fn ground_cannot_cross_but_air_flies_straight_over() {
    let mut state = chasm(
        1,
        vec![
            unit(0, UnitKind::Scuttler, 8, 5),
            unit(0, UnitKind::Wisp, 8, 7),
        ],
    )
    .build()
    .unwrap();
    let (walker, flyer) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![walker],
                goal: TilePos::new(18, 5),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![flyer],
                goal: TilePos::new(18, 7),
                queue: false,
            },
        ),
    ]);
    run(&mut state, 400);
    assert!(
        state.unit(walker).unwrap().tile().x < 12,
        "the ground half stays ground-half"
    );
    assert_eq!(
        state.unit(flyer).unwrap().tile(),
        TilePos::new(18, 7),
        "the flyer crossed the void without a detour"
    );
}

#[test]
fn direct_fire_crosses_the_void_that_rock_would_block() {
    // Two Sentinels flanking a one-tile chasm: 2 tiles apart, range 2.5.
    let mut state = chasm(
        1,
        vec![
            unit(0, UnitKind::Sentinel, 11, 5),
            unit(1, UnitKind::Sentinel, 13, 5),
        ],
    )
    .build()
    .unwrap();
    let (west, east) = (state.units()[0].id, state.units()[1].id);
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![west],
            target: Target::Unit(east),
            queue: false,
        },
    )]);
    let mut events = report.events;
    events.extend(run(&mut state, 40));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::AttackHit { attacker, .. } if *attacker == west)),
        "the shot crosses the pit"
    );

    // The identical geometry over rock: full cover, no firing position.
    let mut walled = chasm(1, vec![]);
    for row in walled.map.iter_mut() {
        *row = row.replace('~', "#");
    }
    walled.units = vec![
        unit(0, UnitKind::Sentinel, 11, 5),
        unit(1, UnitKind::Sentinel, 13, 5),
    ];
    let mut state = walled.build().unwrap();
    let (west, east) = (state.units()[0].id, state.units()[1].id);
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![west],
            target: Target::Unit(east),
            queue: false,
        },
    )]);
    let mut events = report.events;
    events.extend(run(&mut state, 40));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::AttackHit { attacker, .. } if *attacker == west)),
        "the same line over rock is covered"
    );
}

#[test]
fn artillery_arcs_sail_over_the_chasm() {
    // A Bombard west of a four-wide chasm, its spotter hovering over the
    // void; the target harvester works the east rim. Range 9.5 covers the
    // 9-tile line. On a peak ridge this exact shape never launches.
    let mut state = chasm(
        4,
        vec![
            unit(0, UnitKind::Bombard, 8, 5),
            unit(0, UnitKind::Wisp, 14, 5),
            unit(1, UnitKind::Harvester, 17, 5),
        ],
    )
    .build()
    .unwrap();
    let (gun, victim) = (state.units()[0].id, state.units()[2].id);
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![gun],
            target: Target::Unit(victim),
            queue: false,
        },
    )]);
    let mut events = report.events;
    events.extend(run(&mut state, 60));
    assert!(
        events.iter().any(
            |e| matches!(e, Event::ShellLaunched { shooter, .. } if *shooter == Target::Unit(gun))
        ),
        "the arc crosses the void"
    );
}

#[test]
fn a_flyer_downed_over_the_void_leaves_nothing() {
    let mut state = chasm(
        2,
        vec![
            unit(0, UnitKind::Flakhound, 10, 5),
            unit(1, UnitKind::Wisp, 12, 5),
        ],
    )
    .build()
    .unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![flak],
            target: Target::Unit(wisp),
            queue: false,
        },
    )]);
    let events = run(&mut state, 400);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == wisp)),
        "the wisp went down"
    );
    for x in 12..14 {
        for y in 1..11 {
            assert_eq!(
                state.map().wreck_at(TilePos::new(x, y)),
                0,
                "salvage over the void at ({x}, {y}) should be gone forever"
            );
        }
    }
}

#[test]
fn foundations_refuse_the_void() {
    let mut state = chasm(1, vec![unit(0, UnitKind::Harvester, 10, 5)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::BuildingKind::Turret,
            anchor: TilePos::new(12, 5),
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "a pit tile can never hold a foundation"
    );
}

#[test]
fn ground_goals_on_the_void_snap_to_the_rim() {
    let mut state = chasm(1, vec![unit(0, UnitKind::Scuttler, 9, 5)])
        .build()
        .unwrap();
    let walker = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![walker],
            goal: TilePos::new(12, 5),
            queue: false,
        },
    )]);
    run(&mut state, 200);
    let landed = state.unit(walker).unwrap().tile();
    assert!(
        landed != TilePos::new(12, 5) && (landed.x - 12).abs() <= 3,
        "the goal snapped to standable ground near the rim, got {landed:?}"
    );
}

#[test]
fn a_chasm_map_reproduces_bit_identically() {
    let scenario = chasm(
        2,
        vec![
            unit(0, UnitKind::Sentinel, 10, 4),
            unit(1, UnitKind::Sentinel, 15, 6),
            unit(0, UnitKind::Wisp, 9, 7),
        ],
    );
    let mut a = scenario.clone().build().unwrap();
    let mut b = scenario.build().unwrap();
    let orders = |state: &State| {
        vec![cmd(
            0,
            Command::AttackMove {
                units: state
                    .units()
                    .iter()
                    .filter(|u| u.player == PlayerId(0))
                    .map(|u| u.id)
                    .collect(),
                goal: TilePos::new(18, 6),
                queue: false,
            },
        )]
    };
    let first = orders(&a);
    a.tick(&first);
    let second = orders(&b);
    b.tick(&second);
    for _ in 0..300 {
        a.tick(&[]);
        b.tick(&[]);
    }
    assert_eq!(a.hash(), b.hash());
}

#[test]
fn a_chasm_severs_scenario_ground_connectivity() {
    // Anchors on opposite rims of a full-height chasm: the build gate
    // must refuse — victory would be unreachable on the ground.
    let mut map = vec!["#".repeat(24)];
    for y in 1..11 {
        let mut row = String::from("#");
        for x in 1..23 {
            row.push(match (x, y) {
                (1, 1) => '1',
                (20, 9) => '2',
                (12, _) => '~',
                _ => '.',
            });
        }
        row.push('#');
        map.push(row);
    }
    map.push("#".repeat(24));
    let scenario = Scenario {
        name: "severed".into(),
        seed: 9,
        map,
        players: players(),
        units: Vec::new(),
        buildings: Vec::new(),
        meta: None,
    };
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::Disconnected(_, _))
    ));
}
