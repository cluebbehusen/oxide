//! Doctrine tests for the scripted brain (the channel policy the
//! Overseer commands through): artillery standoff, air defense, repair
//! audits, wreck salvage, raid discipline — and the orientation
//! involution that keeps every new observation field seat-fair.

use chassis::grid::TilePos;
use oxide_sim::bot::observation::OBSERVATION_VERSION;
use oxide_sim::bot::{BuildingObs, Intent, Observation, Orientation, UnitObs};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{BuildingId, Faction, PlayerId, UnitId, UnitKind};

fn obs_base() -> Observation {
    Observation {
        version: OBSERVATION_VERSION,
        tick: 0,
        me: PlayerId(0),
        scrap: 0,
        map_width: 24,
        map_height: 13,
        my_units: Vec::new(),
        my_buildings: Vec::new(),
        my_queues: Vec::new(),
        ally_units: Vec::new(),
        ally_buildings: Vec::new(),
        enemy_units: Vec::new(),
        enemy_buildings: Vec::new(),
        explored: vec![true; 24 * 13],
        known_scrap: Vec::new(),
        known_rock: Vec::new(),
        known_frames: Vec::new(),
        known_peaks: Vec::new(),
        known_wrecks: Vec::new(),
        blips: Vec::new(),
        faction: Faction::Ferrous,
        my_shells: 0,
        incoming_shells: Vec::new(),
    }
}

fn unit_obs(id: u32, player: u8, kind: UnitKind, x: i32, y: i32) -> UnitObs {
    UnitObs {
        id: UnitId(id),
        player: PlayerId(player),
        kind,
        tile: TilePos::new(x, y),
        hp: kind.stats().max_hp,
        idle: true,
        carrying: 0,
        cargo: 0,
        site: None,
        salvaging: None,
        founding: None,
    }
}

fn building_obs(id: u32, player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingObs {
    BuildingObs {
        id: BuildingId(id),
        player: PlayerId(player),
        kind,
        anchor: TilePos::new(x, y),
        hp: kind.base_stats().max_hp,
        built: true,
        seen: true,
        tier: 0,
    }
}

/// A fully-populated observation: every positioned field carries data,
/// so a field the orientation forgets to flip fails the round-trip.
fn full_obs() -> Observation {
    let mut obs = obs_base();
    obs.explored.fill(false);
    let explored = TilePos::new(4, 3);
    let explored_index = usize::try_from(explored.y * obs.map_width + explored.x).unwrap();
    obs.explored[explored_index] = true;
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Harvester, 3, 2),
        unit_obs(1, 0, UnitKind::Bombard, 4, 5),
    ];
    obs.my_buildings = vec![building_obs(0, 0, BuildingKind::Foundry, 2, 2)];
    obs.my_queues = vec![vec![UnitKind::Sentinel]];
    obs.ally_units = vec![unit_obs(7, 1, UnitKind::Wisp, 6, 6)];
    obs.ally_buildings = vec![building_obs(3, 1, BuildingKind::Array, 8, 3)];
    obs.enemy_units = vec![unit_obs(9, 2, UnitKind::Darter, 19, 9)];
    obs.enemy_buildings = vec![building_obs(5, 2, BuildingKind::Bastion, 18, 8)];
    obs.known_scrap = vec![(TilePos::new(5, 5), 200), (TilePos::new(12, 4), 400)];
    obs.known_rock = vec![TilePos::new(10, 6), TilePos::new(11, 6)];
    obs.known_peaks = vec![TilePos::new(11, 6)];
    obs.known_wrecks = vec![(TilePos::new(9, 9), 30)];
    obs.blips = vec![TilePos::new(15, 2), TilePos::new(16, 11)];
    obs
}

#[test]
fn orientation_is_an_involution_over_every_positioned_field() {
    let obs = full_obs();
    // A southeast home flips both axes — the maximal transform.
    let orientation = Orientation::for_home(&obs, TilePos::new(20, 10));
    assert!(!orientation.is_identity());
    let flipped = orientation.observe(&obs);
    assert!(flipped.explored(orientation.tile(TilePos::new(4, 3))));
    assert!(!flipped.explored(TilePos::new(4, 3)));
    assert_ne!(
        serde_json::to_string(&obs).unwrap(),
        serde_json::to_string(&flipped).unwrap(),
        "test premise: the flip must actually move things"
    );
    let mut back = orientation.observe(&flipped);
    // The involution restores positions; only sort order may differ, so
    // normalize before comparing.
    back.known_scrap.sort_by_key(|(p, _)| (p.y, p.x));
    back.known_wrecks.sort_by_key(|(p, _)| (p.y, p.x));
    back.known_rock.sort_by_key(|p| (p.y, p.x));
    back.known_peaks.sort_by_key(|p| (p.y, p.x));
    back.blips.sort_by_key(|p| (p.y, p.x));
    let mut want = obs.clone();
    want.known_scrap.sort_by_key(|(p, _)| (p.y, p.x));
    want.known_wrecks.sort_by_key(|(p, _)| (p.y, p.x));
    want.known_rock.sort_by_key(|p| (p.y, p.x));
    want.known_peaks.sort_by_key(|p| (p.y, p.x));
    want.blips.sort_by_key(|p| (p.y, p.x));
    assert_eq!(
        serde_json::to_string(&want).unwrap(),
        serde_json::to_string(&back).unwrap(),
        "flipping twice must restore every positioned field"
    );
}

#[test]
fn positioned_intents_flip_and_positionless_ones_pass() {
    let obs = full_obs();
    let orientation = Orientation::for_home(&obs, TilePos::new(20, 10));
    let intents = vec![
        Intent::RaidAir {
            target: TilePos::new(4, 4),
        },
        Intent::Repair {
            building: BuildingId(3),
        },
    ];
    let emitted = orientation.emit(intents);
    match &emitted[0] {
        Intent::RaidAir { target } => {
            assert_eq!(
                *target,
                TilePos::new(24 - 1 - 4, 13 - 1 - 4),
                "a raid target must flip into world space"
            );
        }
        other => panic!("unexpected intent {other:?}"),
    }
    assert_eq!(
        emitted[1],
        Intent::Repair {
            building: BuildingId(3)
        },
        "a repair names a building, not a place — it passes through"
    );
}

// --- channel tests: drive the utility policy on synthetic worlds -----

use oxide_sim::bot::{Dials, UtilityPolicy};

/// A base observation with a built home Foundry (the policy's anchor).
fn obs_with_home() -> Observation {
    let mut obs = obs_base();
    obs.my_buildings = vec![building_obs(0, 0, BuildingKind::Foundry, 2, 2)];
    obs.my_queues = vec![Vec::new()];
    obs.scrap = 400;
    obs
}

fn think(policy: &mut UtilityPolicy, obs: &Observation) -> Vec<Intent> {
    policy.think(&Dials::full(), obs, &[], &[])
}

#[test]
fn enemy_air_pulls_anti_air_out_of_the_fabricator() {
    let mut obs = obs_with_home();
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Fabricator, 5, 2));
    obs.my_queues.push(Vec::new());
    obs.enemy_units = vec![unit_obs(9, 1, UnitKind::Darter, 15, 6)];
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(
        intents.iter().any(|i| matches!(
            i,
            Intent::TrainAt {
                kind: UnitKind::Flakhound,
                ..
            }
        )),
        "a Ferrous seat answers the sky with its own AA variant: {intents:?}"
    );

    // The same threat through Cupric eyes buys the Stinger.
    obs.faction = Faction::Cupric;
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(intents.iter().any(|i| matches!(
        i,
        Intent::TrainAt {
            kind: UnitKind::Stinger,
            ..
        }
    )));
}

#[test]
fn wounded_buildings_draw_a_weld_order_when_funded() {
    let mut obs = obs_with_home();
    obs.my_buildings[0].hp = 300; // foundry at 300/800
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, Intent::Repair { building } if *building == BuildingId(0))),
        "a funded seat welds its wounds: {intents:?}"
    );

    // Broke: the torch stays cold.
    obs.scrap = 10;
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(!intents.iter().any(|i| matches!(i, Intent::Repair { .. })));
}

#[test]
fn repairs_never_recrew_an_active_salvage() {
    // Repair and salvage evict each other in the sim, so a repair
    // intent on a building an own crew is stripping would reverse the
    // liquidation the bot itself ordered. The gym's lowering filters
    // these; the scripted channel must too.
    let mut obs = obs_with_home();
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Turret, 5, 2));
    obs.my_queues.push(Vec::new());
    obs.my_buildings
        .push(building_obs(2, 0, BuildingKind::Turret, 8, 2));
    obs.my_queues.push(Vec::new());
    obs.my_buildings[1].hp = 100; // both wounded below the weld line
    obs.my_buildings[2].hp = 100;
    let mut stripper = unit_obs(0, 0, UnitKind::Harvester, 5, 3);
    stripper.salvaging = Some(BuildingId(1));
    obs.my_units = vec![stripper];
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(
        !intents
            .iter()
            .any(|i| matches!(i, Intent::Repair { building } if *building == BuildingId(1))),
        "a building being liquidated draws no weld: {intents:?}"
    );
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, Intent::Repair { building } if *building == BuildingId(2))),
        "the untouched wound still welds: {intents:?}"
    );
}

#[test]
fn idle_harvesters_take_wreck_fields_as_readily_as_nodes() {
    let mut obs = obs_with_home();
    obs.my_units = vec![unit_obs(0, 0, UnitKind::Harvester, 6, 6)];
    obs.known_wrecks = vec![(TilePos::new(8, 6), 40)];
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(
        intents.iter().any(|i| matches!(
            i,
            Intent::AssignHarvest { node, .. } if *node == TilePos::new(8, 6)
        )),
        "battlefield salvage is work: {intents:?}"
    );
}

#[test]
fn air_raids_launch_at_bare_economies_and_scrub_against_flak() {
    let mut obs = obs_with_home();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Buzzard, 4, 4),
        unit_obs(1, 0, UnitKind::Buzzard, 5, 4),
        unit_obs(2, 0, UnitKind::Buzzard, 4, 5),
    ];
    obs.enemy_units = vec![unit_obs(9, 1, UnitKind::Harvester, 18, 9)];
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, Intent::RaidAir { target } if *target == TilePos::new(18, 9))),
        "three idle wings and a bare harvest line is a raid: {intents:?}"
    );

    // Known flak over the target scrubs it.
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::FlakTurret, 17, 8)];
    let mut policy = UtilityPolicy::new();
    let intents = think(&mut policy, &obs);
    assert!(
        !intents.iter().any(|i| matches!(i, Intent::RaidAir { .. })),
        "no wing flies into known flak: {intents:?}"
    );
}

#[test]
fn a_pushed_army_holds_its_artillery_at_standoff() {
    use oxide_sim::bot::Executive;
    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(1, 0, UnitKind::Sentinel, 4, 3),
        unit_obs(2, 0, UnitKind::Bombard, 3, 4),
    ];
    let mut exec = Executive::new();
    let staging = TilePos::new(4, 4);
    let _ = exec.apply(PlayerId(0), &obs, &[Intent::FormArmy { staging, size: 3 }]);
    assert_eq!(exec.armies()[0].members.len(), 3);
    let army = exec.armies()[0].id;
    let target = TilePos::new(20, 10);
    let commands = exec.apply(PlayerId(0), &obs, &[Intent::PushArmy { army, target }]);
    // Two marching orders: escorts onto the target, the gun short of it.
    let goals: Vec<TilePos> = commands
        .iter()
        .filter_map(|c| match &c.command {
            oxide_sim::Command::AttackMove { goal, units, .. } => Some((*goal, units.clone())),
            _ => None,
        })
        .map(|(g, _)| g)
        .collect();
    assert!(goals.contains(&target), "escorts march onto the target");
    assert!(
        goals
            .iter()
            .any(|g| *g != target && g.chebyshev(target) <= 8),
        "the Bombard holds a standoff short of the target: {goals:?}"
    );
}

// --- 0.15 Overseer channels: the ferry and the lane mines -------------

/// An island world: home in the northwest, a known-rock wall severing
/// the map top to bottom, and the enemy Foundry remembered across it.
fn island_obs() -> Observation {
    let mut obs = obs_with_home();
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(12, y)).collect();
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::Foundry, 18, 8)];
    obs
}

#[test]
fn the_ferry_lifts_a_squad_over_a_severed_gulf() {
    let mut obs = island_obs();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(2, 0, UnitKind::Sentinel, 4, 3),
        unit_obs(3, 0, UnitKind::Sentinel, 5, 3),
        unit_obs(10, 0, UnitKind::Skyhook, 4, 4),
    ];
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &obs, &[], &[]);
    let load = intents.iter().find_map(|i| match i {
        Intent::Load { transport, riders } => Some((*transport, riders.clone())),
        _ => None,
    });
    let (transport, riders) = load.expect("an idle skyhook and three idle fighters make a lift");
    assert_eq!(transport, UnitId(10));
    assert_eq!(
        riders,
        vec![UnitId(1), UnitId(2), UnitId(3)],
        "the nearest fighters board, ties to the lowest id"
    );

    // Two fighters are a trickle, not a squad: no lift yet.
    let mut short = obs.clone();
    short.my_units.remove(0);
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &short, &[], &[]);
    assert!(
        !intents.iter().any(|i| matches!(i, Intent::Load { .. })),
        "the ferry waits for a squad: {intents:?}"
    );

    // The same world under the 0.14 dials never ferries.
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::full(), &obs, &[], &[]);
    assert!(
        !intents.iter().any(|i| matches!(i, Intent::Load { .. })),
        "the ferry is dial-gated: {intents:?}"
    );
}

#[test]
fn a_loaded_skyhook_drops_beside_the_island_base() {
    let mut obs = island_obs();
    let mut sky = unit_obs(10, 0, UnitKind::Skyhook, 13, 8);
    sky.cargo = 3;
    obs.my_units = vec![sky];
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &obs, &[], &[]);
    let at = intents.iter().find_map(|i| match i {
        Intent::Unload { transport, at } if *transport == UnitId(10) => Some(*at),
        _ => None,
    });
    let at = at.expect("a settled, loaded skyhook flies the drop");
    let base = TilePos::new(18, 8);
    assert!(
        at.chebyshev(base) <= 6,
        "the drop lands beside the island base: {at:?}"
    );
    assert!(
        !obs.known_rock_at(at),
        "the drop centers on known-walkable ground: {at:?}"
    );
}

#[test]
fn the_skyhook_is_bought_only_for_an_island_war() {
    let mut obs = island_obs();
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Airworks, 5, 5));
    obs.my_queues.push(Vec::new());

    // A lifter without riders is dead capital: no squad, no purchase.
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &obs, &[], &[]);
    assert!(
        !intents.iter().any(|i| matches!(
            i,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )),
        "the fighters come before the lifter: {intents:?}"
    );

    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(2, 0, UnitKind::Sentinel, 4, 3),
        unit_obs(3, 0, UnitKind::Sentinel, 5, 3),
    ];
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &obs, &[], &[]);
    assert!(
        intents.iter().any(|i| matches!(
            i,
            Intent::TrainAt { kind: UnitKind::Skyhook, building } if *building == BuildingId(1)
        )),
        "a severed gulf and a standing squad buy the lifter at the Airworks: {intents:?}"
    );

    // With the road open there is no island war and no lifter.
    let mut open = obs.clone();
    open.known_rock.clear();
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &open, &[], &[]);
    assert!(
        !intents.iter().any(|i| matches!(
            i,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )),
        "a walkable enemy base buys no lifter: {intents:?}"
    );

    // One lifter is the cap.
    obs.my_units = vec![unit_obs(10, 0, UnitKind::Skyhook, 4, 4)];
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &obs, &[], &[]);
    assert!(
        !intents.iter().any(|i| matches!(
            i,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )),
        "a live skyhook satisfies the ferry: {intents:?}"
    );
}

#[test]
fn lane_mines_bury_along_the_known_approach() {
    let mut obs = obs_with_home();
    // Full queues keep the production channel quiet so the bank stays
    // for the construction arms under test.
    obs.my_queues[0] = vec![UnitKind::Sentinel, UnitKind::Sentinel];
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Fabricator, 5, 2));
    obs.my_queues.push(vec![UnitKind::Lancer, UnitKind::Lancer]);
    obs.my_buildings
        .push(building_obs(2, 0, BuildingKind::Airworks, 8, 2));
    obs.my_queues
        .push(vec![UnitKind::Buzzard, UnitKind::Buzzard]);
    obs.my_units = (0..5)
        .map(|i| unit_obs(i, 0, UnitKind::Harvester, 3 + i as i32, 5))
        .collect();
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::Foundry, 18, 8)];
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &obs, &[], &[]);
    let anchor = intents.iter().find_map(|i| match i {
        Intent::Build {
            kind: BuildingKind::ScuttleCharge,
            anchor,
        } => Some(*anchor),
        _ => None,
    });
    let anchor = anchor.expect("a known ground road draws a buried charge");
    let home = TilePos::new(2, 2);
    assert!(
        anchor.chebyshev(home) <= 2 * (5 + 7),
        "the field sits a few tiles out from home: {anchor:?}"
    );
    assert!(
        anchor.x >= home.x && anchor.y >= home.y,
        "the charge leans toward the enemy, never behind the base: {anchor:?}"
    );

    // The 0.14 dials never mine.
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::full(), &obs, &[], &[]);
    assert!(
        !intents.iter().any(|i| matches!(
            i,
            Intent::Build {
                kind: BuildingKind::ScuttleCharge,
                ..
            }
        )),
        "mining is dial-gated: {intents:?}"
    );
}

#[test]
fn the_army_draft_never_conscripts_the_air_wing() {
    use oxide_sim::bot::Executive;
    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(1, 0, UnitKind::Buzzard, 4, 3),
        unit_obs(2, 0, UnitKind::Talon, 3, 4),
        unit_obs(3, 0, UnitKind::Sentinel, 5, 3),
    ];
    let mut exec = Executive::new();
    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: TilePos::new(4, 4),
            size: 4,
        }],
    );
    let members = &exec.armies()[0].members;
    assert_eq!(
        members.as_slice(),
        &[UnitId(0), UnitId(3)],
        "armies are ground bodies; wings stay free for the raid channel"
    );
}
