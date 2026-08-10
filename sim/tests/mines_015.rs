//! The 0.15 field kit: buried Scuttle Charges (the game's only
//! stealth), the Sapper's one-way demolition, Barricade walls, and the
//! Scrap Depot drop-off.

use chassis::grid::TilePos;
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

fn arena(map: Vec<String>, units: Vec<UnitSpec>, buildings: Vec<BuildingSpec>) -> Scenario {
    Scenario {
        name: "field-kit-arena".into(),
        seed: 17,
        map,
        players: players(800),
        units,
        buildings,
        meta: None,
    }
}

fn open_map() -> Vec<String> {
    vec![
        "########################".into(),
        "#1.....................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#...................2..#".into(),
        "#......................#".into(),
        "########################".into(),
    ]
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

fn building(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

#[test]
fn a_charge_is_invisible_until_scouted() {
    // A Ferrous Warden stands right next to a Cupric charge: full tile
    // sight, zero knowledge.
    let mut state = arena(
        open_map(),
        vec![unit(0, UnitKind::Warden, 9, 4)],
        vec![building(1, BuildingKind::ScuttleCharge, 11, 4)],
        // (The warden sits INSIDE its own aggro ring of the charge: the
        // acquisition gate is under test as much as the command gate.)
    )
    .build()
    .unwrap();
    let warden = state.units()[0].id;
    let charge = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::ScuttleCharge)
        .unwrap()
        .id;
    state.tick(&[]);
    assert!(
        state.vision(PlayerId(0)).ghosts().is_empty(),
        "an undetected charge must never enter ghost memory"
    );
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![warden],
            target: Target::Building(charge),
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::InvalidTarget,
                ..
            }
        )),
        "sight of the ground is not knowledge of the mine"
    );
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::ScuttleCharge),
        "and no idle gun may have auto-attacked the invisible thing"
    );

    // A scout overhead changes everything.
    let mut scouted = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Warden, 3, 4),
            unit(0, UnitKind::Kestrel, 12, 3),
        ],
        vec![building(1, BuildingKind::ScuttleCharge, 11, 4)],
    )
    .build()
    .unwrap();
    let warden = scouted.units()[0].id;
    let charge = scouted
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::ScuttleCharge)
        .unwrap()
        .id;
    scouted.tick(&[]);
    assert!(
        !scouted.vision(PlayerId(0)).ghosts().is_empty(),
        "a scouted charge is honest knowledge"
    );
    let report = scouted.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![warden],
            target: Target::Building(charge),
            queue: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "a detected mine is a legal target: {:?}",
        report.events
    );
}

#[test]
fn a_charge_detonates_under_hostile_treads() {
    let mut state = arena(
        open_map(),
        vec![unit(0, UnitKind::Warden, 5, 4)],
        vec![building(1, BuildingKind::ScuttleCharge, 11, 4)],
    )
    .build()
    .unwrap();
    let warden = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![warden],
            goal: TilePos::new(16, 4),
            queue: false,
        },
    )]);
    let mut boomed = false;
    for _ in 0..400 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::ChargeDetonated { .. }))
        {
            boomed = true;
            break;
        }
    }
    assert!(boomed, "the walk crossed the trigger ring");
    let hp = state.unit(warden).unwrap().hp;
    assert_eq!(
        hp,
        UnitKind::Warden.stats().max_hp - oxide_sim::stats::CHARGE_DAMAGE,
        "the blast takes exactly its damage"
    );
    assert!(
        !state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::ScuttleCharge),
        "the charge is consumed by its own blast"
    );
}

#[test]
fn saturation_fire_clears_a_field_without_detonation() {
    // A Bombard shells a Barricade; its 1.4 splash catches the buried
    // charge sitting one tile over — which is destroyed, never fired.
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Bombard, 6, 4),
            unit(0, UnitKind::Kestrel, 11, 2),
        ],
        vec![
            building(1, BuildingKind::Barricade, 12, 4),
            building(1, BuildingKind::ScuttleCharge, 12, 5),
        ],
    )
    .build()
    .unwrap();
    let gun = state.units()[0].id;
    let wall = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Barricade)
        .unwrap()
        .id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![gun],
            target: Target::Building(wall),
            queue: false,
        },
    )]);
    let mut charge_died = false;
    let mut detonated = false;
    for _ in 0..600 {
        let report = state.tick(&[]);
        for event in &report.events {
            match event {
                Event::BuildingDestroyed { building, .. }
                    if *building
                        == state.buildings().first().map(|b| b.id).unwrap_or(*building) => {}
                _ => {}
            }
            if matches!(event, Event::ChargeDetonated { .. }) {
                detonated = true;
            }
        }
        charge_died = !state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::ScuttleCharge);
        if charge_died {
            break;
        }
    }
    assert!(charge_died, "splash reaches the buried charge");
    assert!(!detonated, "a splashed mine is destroyed, not fired");
}

#[test]
fn the_sapper_cracks_the_wall_and_is_consumed() {
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Sapper, 6, 4),
            // The spotter that makes the cross-map order fog-legal.
            unit(0, UnitKind::Kestrel, 11, 2),
            unit(1, UnitKind::Scuttler, 12, 5),
        ],
        vec![building(1, BuildingKind::Barricade, 12, 4)],
    )
    .build()
    .unwrap();
    let sapper = state.units()[0].id;
    let bystander = state.units()[2].id;
    let wall = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Barricade)
        .unwrap()
        .id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![sapper],
            target: Target::Building(wall),
            queue: false,
        },
    )]);
    for _ in 0..400 {
        state.tick(&[]);
        if state.unit(sapper).is_none() {
            break;
        }
    }
    assert!(
        state.unit(sapper).is_none(),
        "the charge consumes its carrier: {:?}",
        state.unit(sapper)
    );
    let wall_hp = state.building(wall).unwrap().hp;
    assert_eq!(
        wall_hp,
        BuildingKind::Barricade.base_stats().max_hp - oxide_sim::stats::SAPPER_STRUCTURE_DAMAGE,
        "the wall takes the full charge"
    );
    let bystander_hp = state.unit(bystander).map_or(0, |u| u.hp);
    assert!(
        bystander_hp < UnitKind::Scuttler.stats().max_hp,
        "the adjacent machine takes the splash"
    );
}

#[test]
fn the_depot_is_a_real_drop_off() {
    // Node and depot sit far from the Foundry: a fast first deposit
    // proves the pad accepts scrap.
    let map = vec![
        "########################".into(),
        "#1.....................#".into(),
        "#......................#".into(),
        "#..................s...#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#......................#".into(),
        "#...................2..#".into(),
        "#......................#".into(),
        "########################".into(),
    ];
    let mut state = arena(
        map,
        vec![unit(0, UnitKind::Harvester, 18, 4)],
        vec![building(0, BuildingKind::ScrapDepot, 16, 3)],
    )
    .build()
    .unwrap();
    let worker = state.units()[0].id;
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(19, 3),
            queue: false,
        },
    )]);
    for tick in 0..400u32 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::ScrapDeposited { .. }))
        {
            assert!(
                tick < 250,
                "a depot beside the node must beat the cross-map haul (deposited at {tick})"
            );
            return;
        }
    }
    panic!("no deposit ever landed at the depot");
}

#[test]
fn a_barricade_closes_the_corridor() {
    let map = vec![
        "########################".into(),
        "#1.....................#".into(),
        "#......................#".into(),
        "########.###############".into(),
        "#......................#".into(),
        "#..................2...#".into(),
        "#......................#".into(),
        "########################".into(),
    ];
    let mut state = arena(
        map,
        vec![unit(0, UnitKind::Sentinel, 4, 1)],
        vec![building(0, BuildingKind::Barricade, 8, 3)],
    )
    .build()
    .unwrap();
    let walker = state.units()[0].id;
    let report = state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![walker],
            goal: TilePos::new(20, 4),
            queue: false,
        },
    )]);
    let mut stalled = report.events.iter().any(|e| {
        matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::UnreachableGoal,
                ..
            } | Event::OrderStalled { .. }
        )
    });
    for _ in 0..60 {
        let report = state.tick(&[]);
        stalled |= report
            .events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { .. }));
    }
    assert!(stalled, "the bought wall closes the only road");
    let tile = state.unit(walker).unwrap().tile();
    assert!(tile.y <= 2, "nothing walked through the wall: at {tile:?}");
}
