//! The 0.15 roster additions: the Warden line, the Tender's torch, the
//! Excavator's double-pace labor and its tech gate, and the scout and
//! interceptor wings.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Command, Event, Faction, PlayerCommand, PlayerId, Scenario, UnitKind};

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

fn arena(scrap: u32, fabricator: bool, units: Vec<UnitSpec>) -> Scenario {
    let mut buildings = Vec::new();
    if fabricator {
        buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 2,
            y: 5,
        });
    }
    Scenario {
        name: "roster-arena".into(),
        seed: 9,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#....s.............#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#...............2..#".into(),
            "#..................#".into(),
            "####################".into(),
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

#[test]
fn the_excavator_waits_on_the_fabricator_and_builds_at_double_pace() {
    // Without the tech hall, the Foundry refuses the Excavator by name.
    let mut bare = arena(1_000, false, vec![]).build().unwrap();
    let foundry = bare.buildings()[0].id;
    let report = bare.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Excavator,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::MissingPrerequisite,
            ..
        }
    )));

    // Side by side, the Excavator's site finishes first.
    let race = |builder_kind: UnitKind| -> u64 {
        let mut state = arena(1_000, true, vec![unit(0, builder_kind, 8, 4)])
            .build()
            .unwrap();
        let builder = state.units()[0].id;
        state.tick(&[cmd(
            0,
            Command::Build {
                units: vec![builder],
                kind: BuildingKind::Turret,
                anchor: TilePos::new(9, 4),
                queue: false,
                defer: false,
            },
        )]);
        let site = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Turret)
            .unwrap()
            .id;
        for tick in 0..2_000u64 {
            state.tick(&[]);
            if state.building(site).is_some_and(|b| b.built) {
                return tick;
            }
        }
        panic!("the turret never stood");
    };
    let harvester_pace = race(UnitKind::Harvester);
    let excavator_pace = race(UnitKind::Excavator);
    assert!(
        excavator_pace * 2 <= harvester_pace + 4,
        "double hands: excavator {excavator_pace} vs harvester {harvester_pace}"
    );
}

#[test]
fn the_tender_welds_but_never_harvests() {
    let mut state = arena(
        1_000,
        true,
        vec![
            unit(0, UnitKind::Tender, 8, 4),
            unit(0, UnitKind::Sentinel, 9, 4),
            unit(1, UnitKind::Scuttler, 10, 4),
        ],
    )
    .build()
    .unwrap();
    let (tender, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );

    // The gathering verb refuses a machine with no cargo gear. (One empty
    // tick first so the node is inside reconciled vision.)
    state.tick(&[]);
    assert!(
        state.map().scrap_at(TilePos::new(5, 4)) > 0,
        "the arena's scrap node is where the test thinks it is"
    );
    assert!(
        state.vision(PlayerId(0)).visible(TilePos::new(5, 4)),
        "the node sits inside the Tender's sight"
    );
    let report = state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![tender],
            node: TilePos::new(5, 4),
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::NoValidUnits,
                ..
            }
        )),
        "no bucket, no harvest: {:?}",
        report.events
    );

    // Wound the sentinel, then the Tender welds it back.
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![raider],
            target: oxide_sim::Target::Unit(patient),
            queue: false,
        },
    )]);
    for _ in 0..60 {
        state.tick(&[]);
        if state.unit(patient).unwrap().hp < 40 {
            break;
        }
    }
    state.tick(&[cmd(
        1,
        Command::Stop {
            units: vec![raider],
        },
    )]);
    let wounded = state.unit(patient).unwrap().hp;
    assert!(wounded < UnitKind::Sentinel.stats().max_hp);
    let report = state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![tender],
            target: patient,
            queue: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "the torch is accepted"
    );
    for _ in 0..400 {
        state.tick(&[]);
        if state.unit(patient).unwrap().hp > wounded {
            return;
        }
    }
    panic!("the Tender never welded a single hp");
}

#[test]
fn interceptors_rule_the_sky_and_ignore_the_ground() {
    let mut state = arena(
        1_000,
        false,
        vec![
            unit(0, UnitKind::Shrike, 8, 4),
            // The Kestrel spots the distant gnat so the ordered attack is
            // fog-legal; both scouts are unarmed and out of shrike aggro.
            unit(0, UnitKind::Kestrel, 15, 2),
            unit(1, UnitKind::Gnat, 17, 2),
            unit(1, UnitKind::Scuttler, 8, 6),
        ],
    )
    .build()
    .unwrap();
    let (shrike, gnat, crawler) = (
        state.units()[0].id,
        state.units()[2].id,
        state.units()[3].id,
    );

    // A ground target refuses the air-only gun.
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![shrike],
            target: oxide_sim::Target::Unit(crawler),
            queue: false,
        },
    )]);
    let ground_hit = (0..40).any(|_| {
        state
            .tick(&[])
            .events
            .iter()
            .any(|e| matches!(e, Event::AttackHit { attacker, .. } if *attacker == shrike))
    });
    drop(report);
    assert!(!ground_hit, "an interceptor cannot touch the ground");

    // The scout falls to it.
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![shrike],
            target: oxide_sim::Target::Unit(gnat),
            queue: false,
        },
    )]);
    let mut fell = false;
    for _ in 0..400 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == gnat))
        {
            fell = true;
            break;
        }
    }
    assert!(
        fell,
        "the scout has no answer: shrike {:?} gnat {:?}",
        state.unit(shrike),
        state.unit(gnat)
    );
}
