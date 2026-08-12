//! The 0.15 field kit: buried Scuttle Charges (the game's only
//! stealth), the Sapper's one-way demolition, Barricade walls, and the
//! Scrap Depot drop-off.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, CHARGE_ARRAY_DETECT_RADIUS, CHARGE_BASE_ARRAY_DETECT_RADIUS};
use oxide_sim::{
    Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind,
};

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

/// A 40x12 floor: both detection rings — the base mast's 12 and the Deep
/// Array's 22 — fit end to end along row 4, clear of either Foundry.
fn radar_map() -> Vec<String> {
    vec![
        "########################################".into(),
        "#1.....................................#".into(),
        "#......................................#".into(),
        "#......................................#".into(),
        "#......................................#".into(),
        "#......................................#".into(),
        "#......................................#".into(),
        "#......................................#".into(),
        "#......................................#".into(),
        "#...................................2..#".into(),
        "#......................................#".into(),
        "########################################".into(),
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

/// Whether seat `viewer` is allowed to know about the building anchored
/// here — the one stealth authority, asked directly.
fn apparent(state: &State, viewer: u8, anchor: TilePos) -> bool {
    let building = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .expect("a building stands on that anchor");
    state.building_apparent(PlayerId(viewer), building)
}

/// The charge anchors seat `viewer` actually remembers.
fn charge_ghosts(state: &State, viewer: u8) -> Vec<TilePos> {
    state
        .vision(PlayerId(viewer))
        .ghosts()
        .iter()
        .filter(|g| g.kind == BuildingKind::ScuttleCharge)
        .map(|g| g.anchor)
        .collect()
}

#[test]
fn the_base_mast_sweeps_its_own_ring_for_charges() {
    // A standing mast is anti-stealth infrastructure at close range: one
    // charge exactly on the base ring, one a tile past it, and an unarmed
    // spotter holding plain sight of both tiles. The mast's own sight of
    // 9 reaches neither, so detection is doing all the work.
    let mast = TilePos::new(5, 4);
    let inside = TilePos::new(mast.x + CHARGE_BASE_ARRAY_DETECT_RADIUS, mast.y);
    let outside = TilePos::new(inside.x + 1, mast.y);
    let mut state = arena(
        radar_map(),
        vec![unit(0, UnitKind::Harvester, outside.x + 2, mast.y)],
        vec![
            building(0, BuildingKind::Array, mast.x, mast.y),
            building(1, BuildingKind::ScuttleCharge, inside.x, inside.y),
            building(1, BuildingKind::ScuttleCharge, outside.x, outside.y),
        ],
    )
    .build()
    .unwrap();
    state.tick(&[]);
    assert!(
        apparent(&state, 0, inside),
        "the base ring reaches exactly {CHARGE_BASE_ARRAY_DETECT_RADIUS} tiles"
    );
    assert!(
        !apparent(&state, 0, outside),
        "and stops one tile past it, sight of the ground notwithstanding"
    );
    assert_eq!(
        charge_ghosts(&state, 0),
        vec![inside],
        "only the detected mine becomes honest knowledge"
    );
}

#[test]
fn a_mast_under_construction_detects_nothing() {
    // A pile of parts has no sensors — the same rule that keeps sites
    // out of the vision pass.
    let mast = TilePos::new(5, 4);
    let charge = TilePos::new(mast.x + CHARGE_BASE_ARRAY_DETECT_RADIUS, mast.y);
    let mut state = arena(
        radar_map(),
        vec![
            unit(0, UnitKind::Harvester, 3, 8),
            unit(0, UnitKind::Harvester, charge.x + 3, mast.y),
        ],
        vec![building(1, BuildingKind::ScuttleCharge, charge.x, charge.y)],
    )
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Array,
            anchor: mast,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.anchor == mast && !b.built),
        "the site is claimed at once, unfinished"
    );
    assert!(
        !apparent(&state, 0, charge),
        "an unfinished mast sweeps nothing"
    );
    assert!(charge_ghosts(&state, 0).is_empty());

    for _ in 0..1_200 {
        state.tick(&[]);
        if state
            .buildings()
            .iter()
            .any(|b| b.anchor == mast && b.built)
        {
            break;
        }
    }
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.anchor == mast && b.built),
        "the mast never finished"
    );
    assert!(
        apparent(&state, 0, charge),
        "and the finished mast sweeps the very ground its site could not"
    );
}

#[test]
fn the_deep_array_upgrade_buys_the_wide_ring() {
    // The upgrade's product is reach: the same mine, unchanged, sits
    // past the base ring and inside the deep one.
    let mast = TilePos::new(5, 4);
    let far = TilePos::new(mast.x + CHARGE_ARRAY_DETECT_RADIUS, mast.y);
    let mut state = arena(
        radar_map(),
        vec![
            unit(0, UnitKind::Harvester, 6, 6),
            unit(0, UnitKind::Harvester, far.x + 2, mast.y),
        ],
        vec![
            building(0, BuildingKind::Array, mast.x, mast.y),
            building(0, BuildingKind::Fabricator, 2, 7),
            building(1, BuildingKind::ScuttleCharge, far.x, far.y),
        ],
    )
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    let mast_id = state
        .buildings()
        .iter()
        .find(|b| b.anchor == mast)
        .expect("the mast stands")
        .id;
    state.tick(&[]);
    assert!(
        !apparent(&state, 0, far),
        "{CHARGE_ARRAY_DETECT_RADIUS} tiles is past the base mast's reach"
    );

    state.tick(&[cmd(
        0,
        Command::UpgradeBuilding {
            units: vec![builder],
            building: mast_id,
            queue: false,
        },
    )]);
    {
        let b = state
            .building(mast_id)
            .expect("the works survives its own site");
        assert_eq!((b.built, b.tier), (false, 1), "offline as a tier-1 site");
    }
    assert!(
        !apparent(&state, 0, far),
        "a works taken offline detects nothing at either tier"
    );

    for _ in 0..2_000 {
        state.tick(&[]);
        if state.building(mast_id).is_some_and(|b| b.built) {
            break;
        }
    }
    let b = state.building(mast_id).expect("the mast survives");
    assert!(
        b.built && b.tier == 1,
        "the mast stood back up as a Deep Array"
    );
    assert!(
        apparent(&state, 0, far),
        "the deep ring covers ground the base mast never could"
    );
    assert_eq!(charge_ghosts(&state, 0), vec![far]);
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
