//! Doctrine tests for the 0.8 scripted brain: artillery standoff, air
//! defense, repair audits, wreck salvage, raid discipline — and the
//! orientation involution that keeps every new observation field
//! seat-fair.

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
        known_scrap: Vec::new(),
        known_rock: Vec::new(),
        known_wrecks: Vec::new(),
        blips: Vec::new(),
        faction: Faction::Ferrous,
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
        site: None,
    }
}

fn building_obs(id: u32, player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingObs {
    BuildingObs {
        id: BuildingId(id),
        player: PlayerId(player),
        kind,
        anchor: TilePos::new(x, y),
        hp: kind.stats().max_hp,
        built: true,
        seen: true,
    }
}

/// A fully-populated observation: every positioned field carries data,
/// so a field the orientation forgets to flip fails the round-trip.
fn full_obs() -> Observation {
    let mut obs = obs_base();
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
    back.blips.sort_by_key(|p| (p.y, p.x));
    let mut want = obs.clone();
    want.known_scrap.sort_by_key(|(p, _)| (p.y, p.x));
    want.known_wrecks.sort_by_key(|(p, _)| (p.y, p.x));
    want.known_rock.sort_by_key(|p| (p.y, p.x));
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
