//! Strategic-channel tests for the rules-based controller: artillery
//! standoff, air defense, repair audits, wreck salvage, raid discipline,
//! and the orientation involution that keeps observations seat-fair.

use chassis::grid::TilePos;
use oxide_sim::bot::observation::OBSERVATION_VERSION;
use oxide_sim::bot::{BuildingObs, Intent, Observation, Orientation, UnitObs};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{BuildingId, Command, Faction, PlayerId, UnitId, UnitKind};

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
    obs.known_frames = vec![TilePos::new(13, 7), TilePos::new(3, 8)];
    obs.incoming_shells = vec![TilePos::new(7, 7)];
    obs.my_shells = 2;
    obs
}

#[test]
fn frames_flip_like_footprints_not_tiles() {
    // The involution below is satisfied by an untransformed field
    // trivially (flipping nothing twice is nothing), which is exactly
    // how the forgotten known_frames shipped: this one-way check pins
    // the transform itself. A frame is a 2x2, so its anchor image is
    // the building rule, offset by the footprint — not the tile rule.
    let obs = full_obs();
    let orientation = Orientation::for_home(&obs, TilePos::new(20, 10));
    assert!(!orientation.is_identity());
    let flipped = orientation.observe(&obs);
    let mut expected: Vec<TilePos> = obs
        .known_frames
        .iter()
        .map(|f| orientation.anchor(*f, (2, 2)))
        .collect();
    expected.sort_by_key(|p| (p.y, p.x));
    assert_eq!(flipped.known_frames, expected);
    assert!(
        flipped
            .known_frames
            .windows(2)
            .all(|w| { (w[0].y, w[0].x) <= (w[1].y, w[1].x) }),
        "oriented frames keep the canonical order"
    );
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
    let mut want = obs;
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
fn upgrade_policy_prefers_economic_plant_and_respects_its_tech_gate() {
    let mut obs = obs_with_home();
    obs.scrap = 1_000;
    obs.my_buildings.extend([
        building_obs(1, 0, BuildingKind::Fabricator, 5, 2),
        building_obs(2, 0, BuildingKind::Reclaimer, 8, 2),
        building_obs(3, 0, BuildingKind::Turret, 10, 2),
    ]);
    obs.my_queues = vec![
        vec![UnitKind::Sentinel, UnitKind::Sentinel],
        vec![UnitKind::Lancer, UnitKind::Lancer],
        Vec::new(),
        Vec::new(),
    ];
    let mut dials = Dials::full();
    dials.upgrades = true;

    let intents = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(matches!(
        intents.iter().find(|intent| matches!(intent, Intent::Upgrade { .. })),
        Some(Intent::Upgrade { building }) if *building == BuildingId(2)
    ));

    obs.my_buildings
        .retain(|building| building.kind != BuildingKind::Fabricator);
    obs.my_queues.remove(1);
    let intents = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(
        !intents
            .iter()
            .any(|intent| matches!(intent, Intent::Upgrade { .. })),
        "neither upgrade starts before its Fabricator prerequisite stands: {intents:?}"
    );
}

#[test]
fn repairs_never_recrew_an_active_salvage() {
    // Repair and salvage evict each other in the sim, so a repair
    // intent on a building an own crew is stripping would reverse the
    // liquidation the bot itself ordered. The policy must leave that
    // active job alone.
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

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(1, 0, UnitKind::Bombard, 3, 4),
        unit_obs(2, 0, UnitKind::Bombard, 4, 4),
        unit_obs(3, 0, UnitKind::Bombard, 5, 4),
    ];
    let mut exec = Executive::new();
    let _ = exec.apply(PlayerId(0), &obs, &[Intent::FormArmy { staging, size: 4 }]);
    let army = exec.armies()[0].id;
    let commands = exec.apply(PlayerId(0), &obs, &[Intent::PushArmy { army, target }]);
    assert!(commands.iter().any(|command| matches!(
        &command.command,
        Command::AttackMove { units, goal, queue: false }
            if units == &vec![UnitId(0)] && *goal == target
    )));
    let mut parked = commands
        .iter()
        .find_map(|command| match &command.command {
            Command::Move {
                units,
                goal,
                queue: false,
            } if *goal == staging => Some(units.clone()),
            _ => None,
        })
        .expect("the artillery-majority body parks its guns at the rally");
    parked.sort_unstable();
    assert_eq!(
        parked,
        vec![UnitId(1), UnitId(2), UnitId(3)],
        "the single escort advances without dragging unsupported guns: {commands:?}"
    );
}

#[test]
fn air_superiority_mass_does_not_rout_a_ground_only_engagement() {
    use oxide_sim::bot::{ArmyState, Executive};

    let mut obs = obs_base();
    obs.my_units = vec![unit_obs(0, 0, UnitKind::Warden, 6, 5)];
    obs.enemy_units = (10..22)
        .map(|id| unit_obs(id, 1, UnitKind::Shrike, 7, 5))
        .collect();
    let mut exec = Executive::new();
    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: TilePos::new(6, 5),
            size: 1,
        }],
    );

    let _ = exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(
        exec.armies()[0].state,
        ArmyState::Staging,
        "ground-only armor and air-only interceptors cannot engage each other"
    );

    obs.enemy_units
        .push(unit_obs(30, 1, UnitKind::Sentinel, 7, 5));
    let _ = exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(exec.armies()[0].state, ArmyState::Engaging);
    let _ = exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(
        exec.armies()[0].state,
        ArmyState::Engaging,
        "irrelevant interceptors must not turn one ordinary ground contact into a rout"
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
fn a_routeless_ground_scout_yields_to_a_purpose_built_flyer() {
    let mut obs = obs_with_home();
    obs.scrap = 1_000;
    obs.my_buildings.extend([
        building_obs(1, 0, BuildingKind::Fabricator, 5, 2),
        building_obs(2, 0, BuildingKind::Airworks, 8, 2),
    ]);
    obs.my_queues = vec![
        vec![UnitKind::Sentinel, UnitKind::Sentinel],
        vec![UnitKind::Lancer, UnitKind::Lancer],
        Vec::new(),
    ];
    obs.my_units = (0..5)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .collect();
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(12, y)).collect();
    let mut policy = UtilityPolicy::new();
    let dials = Dials::full();

    let first = policy.think(&dials, &obs, &[], &[]);
    let ground_scout = first.iter().find_map(|intent| match intent {
        Intent::Scout { unit, .. } => Some(*unit),
        _ => None,
    });
    assert_eq!(
        ground_scout,
        Some(UnitId(0)),
        "without an aircraft, the first sweep borrows the lowest-id Harvester"
    );

    // The simulation reports an unreachable Move by returning the unit
    // to idle where it started. On the next think, that is direct route
    // testimony rather than a reason to cycle the same ground scout.
    obs.tick = dials.cadence;
    let bounced = policy.think(&dials, &obs, &[], &[]);
    assert!(
        bounced
            .iter()
            .all(|intent| !matches!(intent, Intent::Scout { .. })),
        "a bounced ground scout must be released: {bounced:?}"
    );

    obs.tick += dials.cadence;
    let replacement = policy.think(&dials, &obs, &[], &[]);
    assert!(
        replacement.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                building: BuildingId(2),
                kind: UnitKind::Kestrel,
            }
        )),
        "a known need for air reconnaissance buys one faction scout: {replacement:?}"
    );
    assert!(
        replacement
            .iter()
            .all(|intent| !matches!(intent, Intent::Scout { .. })),
        "ground units stay free while the airborne replacement is being built"
    );

    obs.my_units.push(unit_obs(99, 0, UnitKind::Kestrel, 8, 3));
    obs.tick += dials.cadence;
    let airborne = policy.think(&dials, &obs, &[], &[]);
    assert!(
        airborne.iter().any(|intent| matches!(
            intent,
            Intent::Scout {
                unit: UnitId(99),
                ..
            }
        )),
        "the completed scout flyer takes over the sweep: {airborne:?}"
    );
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

    let commands = oxide_sim::bot::Executive::new().apply(
        PlayerId(0),
        &obs,
        &[Intent::Unload {
            transport: UnitId(10),
            at,
        }],
    );
    assert!(matches!(
        commands.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::Unload {
                transport,
                at: command_at,
                queue: false,
            },
            ..
        }] if *transport == UnitId(10) && *command_at == at
    ));

    obs.my_units[0].idle = false;
    let intents = UtilityPolicy::new().think(&Dials::overseer(), &obs, &[], &[]);
    assert!(
        !intents
            .iter()
            .any(|intent| matches!(intent, Intent::Unload { .. })),
        "an outbound loaded transport keeps its current flight instead of churning orders: {intents:?}"
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
    obs.scrap = UnitKind::Skyhook.stats().cost;
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::overseer(), &obs, &[], &[]);
    assert!(
        !intents
            .iter()
            .any(|intent| matches!(intent, Intent::TrainAt { .. })),
        "a partial ferry fund stays banked instead of leaking into a cheaper unit: {intents:?}"
    );

    obs.scrap = 400;
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

#[test]
fn one_worker_cannot_be_promised_to_two_jobs_in_one_think() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![unit_obs(0, 0, UnitKind::Harvester, 5, 4)];
    obs.my_buildings = vec![
        building_obs(0, 0, BuildingKind::Turret, 6, 4),
        building_obs(1, 0, BuildingKind::Turret, 8, 4),
    ];
    obs.my_queues = vec![Vec::new(), Vec::new()];

    let commands = Executive::new().apply(
        PlayerId(0),
        &obs,
        &[
            Intent::Salvage {
                building: BuildingId(0),
            },
            Intent::Repair {
                building: BuildingId(1),
            },
            Intent::AssignHarvest {
                unit: UnitId(0),
                node: TilePos::new(10, 4),
            },
        ],
    );
    assert_eq!(
        commands,
        vec![oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: Command::Salvage {
                units: vec![UnitId(0)],
                building: BuildingId(0),
                queue: false,
            },
        }],
        "the earliest strategic job owns the only worker for this think"
    );

    let commands = Executive::new().apply(
        PlayerId(0),
        &obs,
        &[
            Intent::Repair {
                building: BuildingId(1),
            },
            Intent::Salvage {
                building: BuildingId(0),
            },
        ],
    );
    assert_eq!(
        commands,
        vec![oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: Command::Repair {
                units: vec![UnitId(0)],
                building: BuildingId(1),
                queue: false,
            },
        }],
        "changing intent order changes the winner, never the one-worker limit"
    );
}

#[test]
fn boarding_riders_leave_the_army_before_its_next_order() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 3, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 4, 4),
        unit_obs(3, 0, UnitKind::Sentinel, 5, 4),
        unit_obs(10, 0, UnitKind::Skyhook, 4, 5),
    ];
    let staging = TilePos::new(6, 4);
    let mut exec = Executive::new();
    let _ = exec.apply(PlayerId(0), &obs, &[Intent::FormArmy { staging, size: 3 }]);
    let army = exec.armies()[0].id;
    let target = TilePos::new(20, 8);

    let commands = exec.apply(
        PlayerId(0),
        &obs,
        &[
            Intent::Load {
                transport: UnitId(10),
                riders: vec![UnitId(1), UnitId(2)],
            },
            Intent::PushArmy { army, target },
        ],
    );

    assert!(matches!(
        commands.first().map(|command| &command.command),
        Some(Command::Load { units, transport, queue: false })
            if units == &vec![UnitId(1), UnitId(2)] && *transport == UnitId(10)
    ));
    assert!(commands.iter().any(|command| matches!(
        &command.command,
        Command::AttackMove { units, goal, queue: false }
            if units == &vec![UnitId(3)] && *goal == target
    )));
    assert_eq!(exec.armies()[0].members, vec![UnitId(3)]);
}

#[test]
fn repeated_refused_marches_restage_even_when_the_target_changes() {
    use oxide_sim::bot::{ArmyState, Executive};

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(2, 0, UnitKind::Sentinel, 4, 3),
    ];
    let mut exec = Executive::new();
    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: TilePos::new(7, 5),
            size: 2,
        }],
    );
    let army = exec.armies()[0].id;

    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army,
            target: TilePos::new(20, 8),
        }],
    );
    obs.tick = 8;
    let _ = exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(exec.armies()[0].state, ArmyState::Pushing);

    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army,
            target: TilePos::new(19, 9),
        }],
    );
    obs.tick = 16;
    let _ = exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));

    let army = &exec.armies()[0];
    assert_eq!(army.state, ArmyState::Staging);
    assert_eq!(army.staging, TilePos::new(3, 3));
    assert_eq!(army.target, None);
    assert_eq!(army.progress, None);
}

#[test]
fn a_march_that_started_but_stopped_eventually_releases_the_army() {
    use oxide_sim::bot::{ArmyState, Executive};

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 11, 7),
        unit_obs(2, 0, UnitKind::Sentinel, 12, 7),
    ];
    let original_staging = TilePos::new(4, 3);
    let mut exec = Executive::new();
    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: original_staging,
            size: 2,
        }],
    );
    let army = exec.armies()[0].id;
    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army,
            target: TilePos::new(21, 9),
        }],
    );

    // A non-idle line proves that the command started, so this is not the
    // immediate refused-order recovery. It has simply stopped making ground.
    for unit in &mut obs.my_units {
        unit.idle = false;
    }
    obs.tick = 1;
    let _ = exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(exec.armies()[0].state, ArmyState::Pushing);
    obs.tick = 1_202;
    let _ = exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));

    let army = &exec.armies()[0];
    assert_eq!(army.state, ArmyState::Staging);
    assert_eq!(army.staging, TilePos::new(11, 7));
    assert_ne!(army.staging, original_staging);
    assert_eq!(army.target, None);
    assert_eq!(army.progress, None);
}

#[test]
fn an_unreachable_extractor_frame_does_not_starve_the_tech_tree() {
    let mut obs = obs_with_home();
    obs.scrap = 2_000;
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Fabricator, 5, 2));
    obs.my_queues.push(Vec::new());
    obs.my_units = (0..5)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .collect();
    obs.known_frames = vec![TilePos::new(19, 8)];
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(12, y)).collect();

    let intents = UtilityPolicy::new().think(&Dials::overseer(), &obs, &[], &[]);
    assert!(
        !intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Extractor,
                ..
            }
        )),
        "the fixed frame has no known ground route: {intents:?}"
    );
    assert!(
        intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Airworks,
                ..
            }
        )),
        "an impossible restoration must yield the construction think to Airworks: {intents:?}"
    );
}

#[test]
fn construction_placement_respects_a_walking_founders_promised_footprint() {
    let mut obs = obs_with_home();
    obs.scrap = 1_000;
    obs.my_units = (0..3)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .collect();
    let dials = Dials::full();

    let first = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    let promised = first
        .iter()
        .find_map(|intent| match intent {
            Intent::Build {
                kind: BuildingKind::Fabricator,
                anchor,
            } => Some(*anchor),
            _ => None,
        })
        .expect("the baseline commander picks a Fabricator site");

    obs.my_units[0].founding = Some((BuildingKind::Fabricator, promised));
    let second = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    let replacement = second
        .iter()
        .find_map(|intent| match intent {
            Intent::Build {
                kind: BuildingKind::Fabricator,
                anchor,
            } => Some(*anchor),
            _ => None,
        })
        .expect("other builders can choose a distinct site");
    let (width, height) = BuildingKind::Fabricator.base_stats().size;
    let overlaps = replacement.x < promised.x + width
        && promised.x < replacement.x + width
        && replacement.y < promised.y + height
        && promised.y < replacement.y + height;
    assert!(
        !overlaps,
        "a second foundation {replacement:?} overlaps the promised footprint {promised:?}"
    );
}

#[test]
fn expansion_capital_is_reserved_until_the_frontier_foundry_is_claimed() {
    let mut obs = obs_with_home();
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Fabricator, 5, 2));
    obs.my_queues.push(Vec::new());
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .chain((4..7).map(|id| unit_obs(id, 0, UnitKind::Sentinel, 3 + id as i32, 6)))
        .collect();
    obs.known_scrap = vec![(TilePos::new(20, 9), 500)];
    obs.scrap = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are buildable expansions")
        .cost
        + 70;
    let mut dials = Dials::full();
    dials.deep_tech = false;
    dials.expansion = true;

    let intents = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(
        !intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Sentinel,
                ..
            }
        )),
        "the unbounded military drip must not spend the expansion fund: {intents:?}"
    );
    let anchor = intents.iter().find_map(|intent| match intent {
        Intent::Build {
            kind: BuildingKind::Foundry,
            anchor,
        } => Some(*anchor),
        _ => None,
    });
    let anchor = anchor.expect("the reserved capital claims a forward Foundry");
    assert!(
        anchor.chebyshev(TilePos::new(20, 9)) <= 7,
        "the expansion belongs to the frontier, not the home ring: {anchor:?}"
    );
}

#[test]
fn a_complete_tree_uses_the_crucible_and_airworks_for_its_heaviest_roster() {
    for (faction, bomber) in [
        (Faction::Ferrous, UnitKind::Condor),
        (Faction::Cupric, UnitKind::Moth),
    ] {
        let mut obs = obs_with_home();
        obs.faction = faction;
        obs.scrap = 2_000;
        obs.my_buildings.extend([
            building_obs(1, 0, BuildingKind::Fabricator, 5, 2),
            building_obs(2, 0, BuildingKind::Airworks, 8, 2),
            building_obs(3, 0, BuildingKind::Crucible, 11, 2),
        ]);
        obs.my_queues = vec![
            vec![UnitKind::Sentinel, UnitKind::Sentinel],
            vec![UnitKind::Lancer, UnitKind::Lancer],
            Vec::new(),
            Vec::new(),
        ];
        obs.my_units = (0..4)
            .map(|id| unit_obs(id, 0, UnitKind::Warden, 3 + id as i32, 5))
            .collect();

        let intents = UtilityPolicy::new().think(&Dials::overseer(), &obs, &[], &[]);
        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt { building, kind: UnitKind::Breaker }
                if *building == BuildingId(3)
        )));
        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt { building, kind }
                if *building == BuildingId(2) && *kind == bomber
        )));
    }
}
