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
