//! The 0.15 strike wing: attack runs, the Moth's stick, the Crucible
//! gates, and the tier-three heavies.

use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Command, Event, Faction, PlayerCommand, PlayerId, Scenario, Target, UnitKind};

fn players(scrap: u32) -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
    ]
}

fn arena(scrap: u32, units: Vec<UnitSpec>, buildings: Vec<BuildingSpec>) -> Scenario {
    Scenario {
        name: "strike-arena".into(),
        seed: 11,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#..................2...#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: players(scrap),
        units,
        buildings,
        meta: None,
    }
}

fn peak_strike_arena() -> Scenario {
    Scenario {
        name: "peak-strike-arena".into(),
        seed: 11,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#.................^^...#".into(),
            "#..............2..^^...#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: players(1_000),
        units: vec![
            unit(0, UnitKind::Condor, 8, 5),
            unit(0, UnitKind::Kestrel, 14, 6),
        ],
        buildings: Vec::new(),
        meta: None,
    }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

/// The victim turret every strike test bombs: ground-only guns cannot
/// answer an aircraft, so the pass geometry is the whole story.
fn victim_turret() -> BuildingSpec {
    BuildingSpec {
        player: 1,
        kind: BuildingKind::Turret,
        x: 15,
        y: 4,
    }
}

#[test]
fn the_condor_bombs_on_passes_and_never_hovers_to_strafe() {
    let mut state = arena(
        1_000,
        vec![
            unit(0, UnitKind::Condor, 5, 4),
            // Spotter so the attack command is fog-legal at issue time.
            unit(0, UnitKind::Kestrel, 13, 3),
        ],
        vec![victim_turret()],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![condor],
            target: Target::Building(turret),
            queue: false,
        },
    )]);
    let mut release_ticks = Vec::new();
    let mut still_streak = 0u32;
    let mut max_still_streak = 0u32;
    let mut last_pos = state.unit(condor).unwrap().pos;
    for _ in 0..900 {
        let report = state.tick(&[]);
        for event in &report.events {
            if matches!(event, Event::ShellLaunched { shooter: Target::Unit(u), .. } if *u == condor)
            {
                release_ticks.push(state.current_tick());
            }
        }
        let pos = state.unit(condor).unwrap().pos;
        if pos == last_pos {
            still_streak += 1;
            max_still_streak = max_still_streak.max(still_streak);
        } else {
            still_streak = 0;
        }
        last_pos = pos;
        if release_ticks.len() >= 2 {
            break;
        }
    }
    assert!(
        release_ticks.len() >= 2,
        "two passes expected, saw releases at {release_ticks:?}"
    );
    let gap = release_ticks[1] - release_ticks[0];
    assert!(
        gap >= u64::from(UnitKind::Condor.stats().weapons[0].cooldown_ticks),
        "passes closer than the bay reload: {gap}"
    );
    // A committed airframe is always flying: it must never sit parked
    // through a whole reload the way a stop-and-strafe gunship would.
    assert!(
        max_still_streak < 60,
        "the condor parked for {max_still_streak} ticks mid-attack"
    );
}

#[test]
fn a_condor_replans_when_its_wide_turn_meets_a_peak() {
    let mut state = peak_strike_arena().build().unwrap();
    let condor = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Condor)
        .unwrap()
        .id;
    let foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1))
        .unwrap()
        .id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![condor],
            target: Target::Building(foundry),
            queue: false,
        },
    )]);

    let mut release_ticks = Vec::new();
    let mut still_streak = 0u32;
    let mut max_still_streak = 0u32;
    let mut last_pos = state.unit(condor).unwrap().pos;
    let mut last_heading = state.unit(condor).unwrap().heading;
    let rate = i16::from(UnitKind::Condor.stats().turn_rate);
    for _ in 0..1_200 {
        let report = state.tick(&[]);
        if report.events.iter().any(
            |event| matches!(event, Event::ShellLaunched { shooter: Target::Unit(id), .. } if *id == condor),
        ) {
            release_ticks.push(state.current_tick());
        }
        let unit = state.unit(condor).unwrap();
        let terrain = state.map().tile(unit.tile()).unwrap().terrain;
        assert_ne!(
            terrain,
            oxide_sim::map::Terrain::Peak,
            "the turn never enters the mountain"
        );
        let delta = i16::from(unit.heading.wrapping_sub(last_heading) as i8).abs();
        assert!(
            delta <= rate,
            "obstacle avoidance turned {delta} steps in one tick (rate {rate})"
        );
        if unit.pos == last_pos {
            still_streak += 1;
            max_still_streak = max_still_streak.max(still_streak);
        } else {
            still_streak = 0;
        }
        last_pos = unit.pos;
        last_heading = unit.heading;
        if release_ticks.len() >= 2 {
            break;
        }
    }
    assert!(
        max_still_streak < 60,
        "the Condor parked against the Peak for {max_still_streak} ticks at {:?} heading {} path {:?}",
        state.unit(condor).unwrap().pos,
        state.unit(condor).unwrap().heading,
        state.unit(condor).unwrap().path,
    );
    assert!(
        release_ticks.len() >= 2,
        "the Condor never completed a second pass: {release_ticks:?}"
    );
}

#[test]
fn the_moth_lays_its_whole_stick_in_one_release() {
    let mut state = arena(
        1_000,
        vec![
            unit(1, UnitKind::Moth, 5, 4),
            unit(1, UnitKind::Gnat, 13, 3),
        ],
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 15,
            y: 4,
        }],
    )
    .build()
    .unwrap();
    let moth = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    state.tick(&[]);
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![moth],
            target: Target::Building(turret),
            queue: false,
        },
    )]);
    for _ in 0..600 {
        let report = state.tick(&[]);
        let impacts: Vec<_> = report
            .events
            .iter()
            .filter_map(|event| match event {
                Event::ShellLaunched {
                    shooter: Target::Unit(u),
                    to,
                    ..
                } if *u == moth => Some(*to),
                _ => None,
            })
            .collect();
        if impacts.is_empty() {
            continue;
        }
        assert_eq!(impacts.len(), 6, "the stick is six bombs, all at once");
        // Laid in a line: consecutive impacts sit a fixed spacing apart,
        // and the whole stick spans several tiles.
        let first = impacts[0];
        let last = impacts[5];
        let span = first.dist(last);
        assert!(
            span > chassis::fx::Fx::lit("3.5") && span < chassis::fx::Fx::lit("4.5"),
            "stick span off: {span}"
        );
        return;
    }
    panic!("the moth never released");
}

#[test]
fn bombers_wait_on_the_crucible_and_heavies_train_inside_it() {
    let mut state = arena(
        5_000,
        vec![],
        vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 4,
                y: 2,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Airworks,
                x: 4,
                y: 5,
            },
        ],
    )
    .build()
    .unwrap();
    let airworks = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Airworks)
        .unwrap()
        .id;
    // No Crucible anywhere: the Airworks refuses the bomber by name.
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: airworks,
            kind: UnitKind::Condor,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::MissingPrerequisite,
                ..
            }
        )),
        "no Crucible, no bomber"
    );

    // Stand a Crucible up and both gates open: the bomber trains at the
    // Airworks and the Breaker trains at the Crucible itself.
    let mut with_crucible = arena(
        5_000,
        vec![],
        vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 4,
                y: 2,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Airworks,
                x: 4,
                y: 5,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Crucible,
                x: 8,
                y: 2,
            },
        ],
    )
    .build()
    .unwrap();
    let airworks = with_crucible
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Airworks)
        .unwrap()
        .id;
    let crucible = with_crucible
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Crucible)
        .unwrap()
        .id;
    let report = with_crucible.tick(&[
        cmd(
            0,
            Command::Train {
                building: airworks,
                kind: UnitKind::Condor,
            },
        ),
        cmd(
            0,
            Command::Train {
                building: crucible,
                kind: UnitKind::Breaker,
            },
        ),
    ]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "both tier-three orders should be accepted"
    );
    let condor_ticks = UnitKind::Condor.stats().train_ticks;
    let breaker_ticks = UnitKind::Breaker.stats().train_ticks;
    for _ in 0..=condor_ticks.max(breaker_ticks) + 10 {
        with_crucible.tick(&[]);
    }
    assert!(
        with_crucible
            .units()
            .iter()
            .any(|u| u.kind == UnitKind::Condor),
        "the condor never rolled out"
    );
    assert!(
        with_crucible
            .units()
            .iter()
            .any(|u| u.kind == UnitKind::Breaker),
        "the breaker never rolled out"
    );
}

#[test]
fn a_bomber_never_turns_faster_than_its_rate() {
    let mut state = arena(
        1_000,
        vec![
            unit(0, UnitKind::Condor, 5, 4),
            unit(0, UnitKind::Kestrel, 13, 3),
        ],
        vec![victim_turret()],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![condor],
            target: Target::Building(turret),
            queue: false,
        },
    )]);
    let rate = i16::from(UnitKind::Condor.stats().turn_rate);
    let mut heading = state.unit(condor).unwrap().heading;
    for _ in 0..600 {
        state.tick(&[]);
        let next = state.unit(condor).unwrap().heading;
        let delta = i16::from(next.wrapping_sub(heading) as i8).abs();
        assert!(
            delta <= rate,
            "heading jumped {delta} steps in one tick (rate {rate})"
        );
        heading = next;
    }
}

/// A taller arena for boundary tests: the east wall is at x = 24 and the
/// south wall at y = 16, so a Condor can be parked close to a corner.
fn edge_arena(units: Vec<UnitSpec>, buildings: Vec<BuildingSpec>) -> Scenario {
    Scenario {
        name: "edge-arena".into(),
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
            "#......................#".into(),
            "#......................#".into(),
            "#..2...................#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: players(1_000),
        units,
        buildings,
        meta: None,
    }
}

fn turret(player: u8, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec {
        player,
        kind: BuildingKind::Turret,
        x,
        y,
    }
}

fn enemy_building(state: &oxide_sim::State, x: i32, y: i32) -> oxide_sim::ids::BuildingId {
    state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1) && b.anchor == chassis::grid::TilePos::new(x, y))
        .expect("the authored enemy building exists")
        .id
}

fn attack(player: u8, unit: oxide_sim::ids::UnitId, target: Target) -> PlayerCommand {
    cmd(
        player,
        Command::Attack {
            units: vec![unit],
            target,
            queue: false,
        },
    )
}

/// What a watched flight did: the longest run of ticks spent motionless,
/// the ticks spent pressed against the world envelope, and the ticks at
/// which a bomb was released.
struct Flight {
    max_still: u32,
    pinned: u32,
    releases: Vec<u64>,
}

/// Runs `ticks` ticks, asserting every tick that the aircraft stays inside
/// the world envelope, and reports how the flight went.
fn fly_and_watch(
    state: &mut oxide_sim::State,
    condor: oxide_sim::ids::UnitId,
    ticks: u32,
    stop_after_releases: usize,
) -> Flight {
    let width = chassis::fx::Fx::from_num(state.map().width());
    let height = chassis::fx::Fx::from_num(state.map().height());
    let half = chassis::fx::HALF;
    let mut release_ticks = Vec::new();
    let mut still_streak = 0u32;
    let mut max_still_streak = 0u32;
    let mut pinned = 0u32;
    let mut last_pos = state.unit(condor).unwrap().pos;
    for _ in 0..ticks {
        let report = state.tick(&[]);
        if report.events.iter().any(
            |event| matches!(event, Event::ShellLaunched { shooter: Target::Unit(id), .. } if *id == condor),
        ) {
            release_ticks.push(state.current_tick());
        }
        let Some(unit) = state.unit(condor) else {
            break;
        };
        assert!(
            unit.pos.x >= half
                && unit.pos.x <= width - half
                && unit.pos.y >= half
                && unit.pos.y <= height - half,
            "the aircraft left the world at {:?} (tick {})",
            unit.pos,
            state.current_tick()
        );
        if unit.pos.x == half
            || unit.pos.x == width - half
            || unit.pos.y == half
            || unit.pos.y == height - half
        {
            pinned += 1;
        }
        if unit.pos == last_pos {
            still_streak += 1;
            max_still_streak = max_still_streak.max(still_streak);
        } else {
            still_streak = 0;
        }
        last_pos = unit.pos;
        if release_ticks.len() >= stop_after_releases {
            break;
        }
    }
    Flight {
        max_still: max_still_streak,
        pinned,
        releases: release_ticks,
    }
}

#[test]
fn a_bomber_ordered_onto_a_corner_building_behind_it_never_touches_the_wall() {
    // A Condor flying north a few tiles from the east wall is ordered onto
    // a turret in the corner behind it. The shorter rotation bends east into
    // the wall; the aircraft must plan a run that keeps it off the boundary
    // entirely and still come around to bomb.
    let mut state = edge_arena(
        vec![
            unit(0, UnitKind::Condor, 19, 12),
            unit(0, UnitKind::Kestrel, 20, 12),
        ],
        vec![turret(1, 22, 14)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let target = enemy_building(&state, 22, 14);
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: chassis::grid::TilePos::new(19, 2),
            queue: false,
        },
    )]);
    let mut ready = false;
    for _ in 0..200 {
        state.tick(&[]);
        let unit = state.unit(condor).unwrap();
        // Northbound within a few compass steps: collision nudges keep the
        // nose oscillating around the exact goal ray.
        if unit.heading.wrapping_sub(189) <= 6 && unit.pos.y < chassis::fx::Fx::from_num(8) {
            ready = true;
            break;
        }
    }
    assert!(
        ready,
        "the Condor never settled onto a northbound leg: {:?}",
        state.unit(condor)
    );
    state.tick(&[attack(0, condor, Target::Building(target))]);
    let flight = fly_and_watch(&mut state, condor, 900, 1);
    assert!(
        flight.max_still < 10,
        "the Condor parked for {} ticks",
        flight.max_still
    );
    assert_eq!(
        flight.pinned, 0,
        "the Condor was pressed against the world's edge"
    );
    assert!(
        !flight.releases.is_empty(),
        "the Condor never came around to bomb"
    );
}

#[test]
fn a_bomber_attacking_an_edge_building_head_on_never_touches_the_wall() {
    let mut state = edge_arena(
        vec![
            unit(0, UnitKind::Condor, 6, 8),
            unit(0, UnitKind::Kestrel, 20, 6),
        ],
        vec![turret(1, 22, 8)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let target = enemy_building(&state, 22, 8);
    state.tick(&[]);
    state.tick(&[attack(0, condor, Target::Building(target))]);
    let flight = fly_and_watch(&mut state, condor, 1_500, 3);
    assert!(
        flight.max_still < 10,
        "the Condor parked for {} ticks",
        flight.max_still
    );
    assert_eq!(
        flight.pinned, 0,
        "the Condor was pressed against the world's edge"
    );
    assert!(
        flight.releases.len() >= 3,
        "three edge passes expected, saw {:?}",
        flight.releases
    );
}

#[test]
fn a_bomber_attacking_a_corner_building_never_leaves_the_world() {
    let mut state = edge_arena(
        vec![
            unit(0, UnitKind::Condor, 8, 3),
            unit(0, UnitKind::Kestrel, 20, 13),
        ],
        vec![turret(1, 22, 14)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let target = enemy_building(&state, 22, 14);
    state.tick(&[]);
    state.tick(&[attack(0, condor, Target::Building(target))]);
    let flight = fly_and_watch(&mut state, condor, 1_500, 3);
    assert!(
        flight.max_still < 10,
        "the Condor parked for {} ticks",
        flight.max_still
    );
    assert_eq!(
        flight.pinned, 0,
        "the Condor was pressed against the world's edge"
    );
    assert!(
        flight.releases.len() >= 3,
        "three corner passes expected, saw {:?}",
        flight.releases
    );
}

#[test]
fn a_warm_bomber_retargeted_inside_its_acceptance_ring_keeps_flying() {
    // Release on the first turret, then immediately retarget onto a
    // second one sitting where the egress leg ends. When the reload is
    // half done the aircraft is inside its own turn-acceptance ring of
    // the new target; it must fly a departure leg and come back around
    // rather than hang over the target until the bay is cold.
    let mut state = arena(
        1_000,
        vec![
            unit(0, UnitKind::Condor, 3, 4),
            unit(0, UnitKind::Kestrel, 14, 3),
        ],
        vec![turret(1, 10, 4), turret(1, 19, 4)],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let first = enemy_building(&state, 10, 4);
    let second = enemy_building(&state, 19, 4);
    state.tick(&[]);
    state.tick(&[attack(0, condor, Target::Building(first))]);
    let flight = fly_and_watch(&mut state, condor, 400, 1);
    assert_eq!(
        flight.releases.len(),
        1,
        "premise: one release on the first turret"
    );
    state.tick(&[attack(0, condor, Target::Building(second))]);
    let flight = fly_and_watch(&mut state, condor, 900, 1);
    assert!(
        flight.max_still < 10,
        "the warm Condor hung over its new target for {} ticks",
        flight.max_still
    );
    assert!(
        !flight.releases.is_empty(),
        "the Condor never came around to bomb the second turret"
    );
}

#[test]
fn an_idle_bomber_orbits_instead_of_hanging_in_place() {
    // Parked far from the enemy Foundry so nothing enters acquisition range.
    let mut state = arena(1_000, vec![unit(0, UnitKind::Condor, 4, 4)], vec![])
        .build()
        .unwrap();
    let condor = state.units()[0].id;
    let start = state.unit(condor).unwrap().pos;
    let radius = UnitKind::Condor.stats().turn_radius();
    let rate = UnitKind::Condor.stats().turn_rate;
    let mut heading = state.unit(condor).unwrap().heading;
    let mut last_pos = start;
    for _ in 0..300 {
        state.tick(&[]);
        let unit = state.unit(condor).unwrap();
        assert_eq!(
            unit.order,
            oxide_sim::state::Order::Idle,
            "premise: nothing to fight"
        );
        assert_ne!(unit.pos, last_pos, "an idle airframe never hangs in place");
        let delta = i16::from(unit.heading.wrapping_sub(heading) as i8).abs();
        assert_eq!(delta, i16::from(rate), "an orbit is a constant-rate turn");
        assert!(
            unit.pos.dist(start) <= radius + radius + chassis::fx::HALF,
            "the orbit wandered {} tiles from where the aircraft went idle",
            unit.pos.dist(start)
        );
        heading = unit.heading;
        last_pos = unit.pos;
    }
}

#[test]
fn a_bomber_whose_target_dies_keeps_flying() {
    let mut state = arena(
        1_000,
        vec![
            unit(0, UnitKind::Condor, 4, 4),
            unit(0, UnitKind::Kestrel, 12, 3),
            unit(1, UnitKind::Harvester, 14, 4),
        ],
        vec![],
    )
    .build()
    .unwrap();
    let condor = state.units()[0].id;
    let harvester = state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    state.tick(&[]);
    state.tick(&[attack(0, condor, Target::Unit(harvester))]);
    let mut killed = false;
    for _ in 0..600 {
        state.tick(&[]);
        if state.unit(harvester).is_none() {
            killed = true;
            break;
        }
    }
    assert!(
        killed,
        "premise: one bomb kills a Harvester; harvester {:?}, condor {:?}",
        state.unit(harvester),
        state.unit(condor)
    );
    let flight = fly_and_watch(&mut state, condor, 150, usize::MAX);
    assert!(
        flight.max_still < 10,
        "the Condor hung in the air for {} ticks after its target died",
        flight.max_still
    );
    assert_ne!(
        state.unit(condor).unwrap().order,
        oxide_sim::state::Order::Attack {
            target: Target::Unit(harvester),
            resume: None,
        },
        "the direct attack hands back once its victim is gone"
    );
}
