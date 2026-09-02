//! Strategic-channel tests for the rules-based controller: artillery
//! standoff, air defense, repair audits, wreck salvage, raid discipline,
//! and the orientation involution that keeps observations seat-fair.

use chassis::grid::TilePos;
use oxide_sim::bot::observation::OBSERVATION_VERSION;
use oxide_sim::bot::{BuildingObs, Intent, Observation, Orientation, PublicMapBriefing, UnitObs};
use oxide_sim::scenario::PlayerSpec;
use oxide_sim::stats::BuildingKind;
use oxide_sim::{BuildingId, Command, Faction, PlayerId, Scenario, Target, UnitId, UnitKind};

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
        visible: vec![true; 24 * 13],
        explored: vec![true; 24 * 13],
        known_scrap: Vec::new(),
        known_rock: Vec::new(),
        known_frames: Vec::new(),
        known_peaks: Vec::new(),
        known_wrecks: Vec::new(),
        salvage_incidents: Vec::new(),
        blips: Vec::new(),
        faction: Faction::Ferrous,
        my_shells: 0,
        incoming_shells: Vec::new(),
    }
}

fn public_map(obs: &Observation) -> PublicMapBriefing {
    let width = usize::try_from(obs.map_width).expect("the test map has a positive width");
    let height = usize::try_from(obs.map_height).expect("the test map has a positive height");
    assert!(width >= 2 && height >= 2);
    let mut map = vec![".".repeat(width); height];
    map[0].replace_range(..1, "1");
    PublicMapBriefing::from_scenario(&Scenario {
        name: "doctrine test map".into(),
        seed: 0,
        map,
        players: vec![PlayerSpec {
            name: "test seat".into(),
            faction: obs.faction,
            team: None,
            scrap: 0,
            bot: false,
            bot_config: None,
        }],
        units: Vec::new(),
        buildings: Vec::new(),
        meta: None,
    })
    .expect("the focused observation has a matching public map")
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
        harvesting: None,
        cargo: 0,
        site: None,
        salvaging: None,
        founding: None,
        repairing: false,
        grounded: false,
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
    obs.visible.fill(false);
    let visible = TilePos::new(7, 2);
    let visible_index = usize::try_from(visible.y * obs.map_width + visible.x).unwrap();
    obs.visible[visible_index] = true;
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
    obs.salvage_incidents = vec![TilePos::new(17, 3), TilePos::new(2, 10)];
    obs.known_frames = vec![TilePos::new(13, 7), TilePos::new(3, 8)];
    obs.incoming_shells = vec![TilePos::new(7, 7)];
    obs.my_shells = 2;
    obs
}

#[test]
fn salvage_incidents_flip_as_canonical_warning_tiles() {
    let obs = full_obs();
    let orientation = Orientation::for_home(&obs, TilePos::new(20, 10));
    let flipped = orientation.observe(&obs);
    let mut expected: Vec<TilePos> = obs
        .salvage_incidents
        .iter()
        .map(|incident| orientation.tile(*incident))
        .collect();
    expected.sort_by_key(|tile| (tile.y, tile.x));

    assert_eq!(flipped.salvage_incidents, expected);
    assert!(
        flipped
            .salvage_incidents
            .windows(2)
            .all(|pair| (pair[0].y, pair[0].x) <= (pair[1].y, pair[1].x))
    );
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
    assert!(flipped.visible(orientation.tile(TilePos::new(7, 2))));
    assert!(!flipped.visible(TilePos::new(7, 2)));
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
    back.salvage_incidents.sort_by_key(|p| (p.y, p.x));
    let mut want = obs;
    want.known_scrap.sort_by_key(|(p, _)| (p.y, p.x));
    want.known_wrecks.sort_by_key(|(p, _)| (p.y, p.x));
    want.known_rock.sort_by_key(|p| (p.y, p.x));
    want.known_peaks.sort_by_key(|p| (p.y, p.x));
    want.blips.sort_by_key(|p| (p.y, p.x));
    want.salvage_incidents.sort_by_key(|p| (p.y, p.x));
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
        Intent::MoveUnits {
            units: vec![UnitId(1), UnitId(2)],
            goal: TilePos::new(5, 4),
        },
        Intent::AttackMoveUnits {
            units: vec![UnitId(3)],
            goal: TilePos::new(6, 4),
        },
        Intent::AttackUnits {
            units: vec![UnitId(4)],
            target: Target::Building(BuildingId(9)),
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
        Intent::MoveUnits {
            units: vec![UnitId(1), UnitId(2)],
            goal: orientation.tile(TilePos::new(5, 4)),
        }
    );
    assert_eq!(
        emitted[2],
        Intent::AttackMoveUnits {
            units: vec![UnitId(3)],
            goal: orientation.tile(TilePos::new(6, 4)),
        }
    );
    assert_eq!(
        emitted[3],
        Intent::AttackUnits {
            units: vec![UnitId(4)],
            target: Target::Building(BuildingId(9)),
        },
        "entity targets are stable ids rather than positions"
    );
    assert_eq!(
        emitted[4],
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

fn player_think(policy: &mut UtilityPolicy, dials: &Dials, obs: &Observation) -> Vec<Intent> {
    policy.think_player_facing(dials, obs, &[], &[], &[], &public_map(obs))
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
fn forward_enemy_guns_do_not_redefine_the_enemy_home_half() {
    let mut obs = obs_with_home();
    obs.my_units = vec![unit_obs(0, 0, UnitKind::Harvester, 3, 2)];
    let central = TilePos::new(12, 2);
    obs.known_scrap = vec![(central, 200)];
    obs.enemy_buildings = vec![building_obs(9, 1, BuildingKind::FlakTurret, 10, 2)];

    let legacy = UtilityPolicy::new().think(&Dials::full(), &obs, &[], &[]);
    assert!(
        legacy
            .iter()
            .all(|intent| !matches!(intent, Intent::AssignHarvest { .. })),
        "the profile-free Overseer retains its historical nearest-building boundary: {legacy:?}"
    );
    let intents = player_think(&mut UtilityPolicy::new(), &Dials::full(), &obs);
    assert!(
        intents.iter().any(|intent| matches!(
            intent,
            Intent::AssignHarvest {
                unit: UnitId(0),
                node,
            } if *node == central
        )),
        "a forward gun is a threat, not evidence of where the enemy home half begins: {intents:?}"
    );

    obs.known_scrap = vec![(TilePos::new(18, 2), 200)];
    obs.enemy_buildings
        .push(building_obs(10, 1, BuildingKind::Foundry, 20, 2));
    let intents = player_think(&mut UtilityPolicy::new(), &Dials::full(), &obs);
    assert!(
        intents
            .iter()
            .all(|intent| !matches!(intent, Intent::AssignHarvest { .. })),
        "a known enemy Foundry still bounds routine harvesting to our half: {intents:?}"
    );
}

#[test]
fn air_raids_ignore_unfinished_flak_but_scrub_against_completed_flak() {
    let mut obs = obs_with_home();
    let dials = Dials::full();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Buzzard, 4, 4),
        unit_obs(1, 0, UnitKind::Buzzard, 5, 4),
        unit_obs(2, 0, UnitKind::Buzzard, 4, 5),
    ];
    obs.enemy_units = vec![unit_obs(9, 1, UnitKind::Harvester, 18, 9)];
    let mut policy = UtilityPolicy::new();
    let intents = player_think(&mut policy, &dials, &obs);
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, Intent::RaidAir { target } if *target == TilePos::new(18, 9))),
        "three idle wings and a bare harvest line is a raid: {intents:?}"
    );

    let mut flak = building_obs(5, 1, BuildingKind::FlakTurret, 17, 8);
    flak.built = false;
    obs.enemy_buildings = vec![flak.clone()];
    let mut policy = UtilityPolicy::new();
    let intents = player_think(&mut policy, &dials, &obs);
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, Intent::RaidAir { target } if *target == TilePos::new(18, 9))),
        "an unfinished Flak site cannot fire and must not scrub the raid: {intents:?}"
    );

    flak.built = true;
    obs.enemy_buildings = vec![flak.clone()];
    let mut policy = UtilityPolicy::new();
    let intents = player_think(&mut policy, &dials, &obs);
    assert!(
        !intents.iter().any(|i| matches!(i, Intent::RaidAir { .. })),
        "no wing flies into completed visible Flak: {intents:?}"
    );

    flak.seen = false;
    obs.enemy_buildings = vec![flak];
    let mut policy = UtilityPolicy::new();
    let intents = player_think(&mut policy, &dials, &obs);
    assert!(
        !intents.iter().any(|i| matches!(i, Intent::RaidAir { .. })),
        "completed remembered Flak remains actionable risk: {intents:?}"
    );

    obs.enemy_buildings[0].built = false;
    let mut overseer_policy = UtilityPolicy::new();
    let overseer = think(&mut overseer_policy, &obs);
    assert!(
        !overseer
            .iter()
            .any(|intent| matches!(intent, Intent::RaidAir { .. })),
        "the frozen profile-free controller retains its legacy Flak assessment: {overseer:?}"
    );
}

#[test]
fn strategic_air_reservations_do_not_complete_a_utility_raid_wing() {
    let mut obs = obs_with_home();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Buzzard, 4, 4),
        unit_obs(1, 0, UnitKind::Buzzard, 5, 4),
    ];
    obs.enemy_units = vec![unit_obs(9, 1, UnitKind::Harvester, 18, 9)];
    let mut dials = Dials::full();
    dials.air_wing = 2;

    let reserved = UtilityPolicy::new().think_player_facing(
        &dials,
        &obs,
        &[],
        &[],
        &[UnitId(0)],
        &public_map(&obs),
    );
    assert!(
        reserved
            .iter()
            .all(|intent| !matches!(intent, Intent::RaidAir { .. })),
        "one reserved bomber plus one free bomber is not a utility wing: {reserved:?}"
    );

    let free =
        UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert!(
        free.iter().any(|intent| matches!(
            intent,
            Intent::RaidAir { target } if *target == TilePos::new(18, 9)
        )),
        "two free bombers complete the utility wing: {free:?}"
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

fn staged_ground_push_intents(obs: &Observation) -> Vec<Intent> {
    use oxide_sim::bot::{Army, ArmyId, ArmyState};

    let members: Vec<UnitId> = obs.my_units.iter().map(|unit| unit.id).collect();
    let army = Army {
        id: ArmyId(7),
        members,
        state: ArmyState::Staging,
        staging: TilePos::new(7, 6),
        target: None,
        focus: None,
        progress: None,
        issued: None,
        bounces: 0,
    };
    let mut dials = Dials::overseer();
    dials.own_strength_scale = u16::MAX;
    dials.enemy_strength_scale = 0;
    UtilityPolicy::new().think_player_facing(&dials, obs, &[army], &[], &[], &public_map(obs))
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
fn ground_armies_only_push_enemy_sites_in_their_own_known_component() {
    use oxide_sim::bot::{Army, ArmyId, ArmyState};

    let army = |members: Vec<UnitId>, staging: TilePos| Army {
        id: ArmyId(7),
        members,
        state: ArmyState::Staging,
        staging,
        target: None,
        focus: None,
        progress: None,
        issued: None,
        bounces: 0,
    };
    let ids: Vec<UnitId> = (1..=6).map(UnitId).collect();
    let mut dials = Dials::overseer();
    dials.own_strength_scale = u16::MAX;
    dials.enemy_strength_scale = 0;

    let mut home_side = island_obs();
    home_side.my_units = (1..=6)
        .map(|id| unit_obs(id, 0, UnitKind::Sentinel, 5 + id as i32, 6))
        .collect();
    let legacy = UtilityPolicy::new().think(
        &dials,
        &home_side,
        &[army(ids.clone(), TilePos::new(7, 6))],
        &[],
    );
    assert!(
        legacy
            .iter()
            .any(|intent| matches!(intent, Intent::PushArmy { .. })),
        "the profile-free Overseer keeps its frozen preflight behavior: {legacy:?}"
    );
    let blocked = UtilityPolicy::new().think_player_facing(
        &dials,
        &home_side,
        &[army(ids.clone(), TilePos::new(7, 6))],
        &[],
        &[],
        &public_map(&home_side),
    );
    assert!(
        blocked
            .iter()
            .all(|intent| !matches!(intent, Intent::PushArmy { .. })),
        "the home army must wait for the ferry rather than order across known rock: {blocked:?}"
    );

    let mut landed = island_obs();
    landed.my_units = (1..=6)
        .map(|id| {
            unit_obs(
                id,
                0,
                UnitKind::Sentinel,
                15 + (id % 2) as i32,
                5 + id as i32,
            )
        })
        .collect();
    let local = UtilityPolicy::new().think_player_facing(
        &dials,
        &landed,
        &[army(ids, TilePos::new(16, 7))],
        &[],
        &[],
        &public_map(&landed),
    );
    assert!(
        local.iter().any(|intent| matches!(
            intent,
            Intent::PushArmy {
                army: ArmyId(7),
                target
            } if *target == TilePos::new(18, 8)
        )),
        "a landed army keeps its local offensive verb: {local:?}"
    );
}

#[test]
fn ground_armies_do_not_invent_a_road_through_an_unexplored_gulf() {
    let mut obs = obs_with_home();
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::Foundry, 18, 8)];
    obs.my_units = (1..=6)
        .map(|id| unit_obs(id, 0, UnitKind::Sentinel, 5 + id as i32, 6))
        .collect();
    obs.explored.fill(false);
    for y in 0..obs.map_height {
        for x in 0..=10 {
            obs.explored[(y * obs.map_width + x) as usize] = true;
        }
        for x in 15..obs.map_width {
            obs.explored[(y * obs.map_width + x) as usize] = true;
        }
    }

    let blocked = staged_ground_push_intents(&obs);
    assert!(
        blocked
            .iter()
            .all(|intent| !matches!(intent, Intent::PushArmy { .. })),
        "unknown ground must not bridge two separately explored islands: {blocked:?}"
    );

    for x in 11..15 {
        obs.explored[(6 * obs.map_width + x) as usize] = true;
    }
    let connected = staged_ground_push_intents(&obs);
    assert!(
        connected
            .iter()
            .any(|intent| matches!(intent, Intent::PushArmy { .. })),
        "an explored corridor restores the ordinary ground push: {connected:?}"
    );
}

#[test]
fn only_the_player_facing_controller_route_checks_defensive_retargets() {
    use oxide_sim::bot::{Army, ArmyId, ArmyState};

    let mut obs = island_obs();
    obs.my_units = (1..=3)
        .map(|id| unit_obs(id, 0, UnitKind::Sentinel, 15 + id as i32, 6))
        .collect();
    obs.enemy_units = vec![unit_obs(20, 1, UnitKind::Sentinel, 5, 4)];
    let army = Army {
        id: ArmyId(7),
        members: vec![UnitId(1), UnitId(2), UnitId(3)],
        state: ArmyState::Staging,
        staging: TilePos::new(16, 6),
        target: None,
        focus: None,
        progress: None,
        issued: None,
        bounces: 0,
    };
    let dials = Dials::overseer();

    let legacy = UtilityPolicy::new().think(&dials, &obs, std::slice::from_ref(&army), &[]);
    assert!(legacy.iter().any(|intent| matches!(
        intent,
        Intent::PushArmy {
            army: ArmyId(7),
            target
        } if *target == TilePos::new(5, 4)
    )));

    let player_facing = UtilityPolicy::new().think_player_facing(
        &dials,
        &obs,
        &[army],
        &[],
        &[],
        &public_map(&obs),
    );
    assert!(
        player_facing
            .iter()
            .all(|intent| !matches!(intent, Intent::PushArmy { .. })),
        "a player-facing army must not retarget across a known wall: {player_facing:?}"
    );
}

#[test]
fn player_facing_defense_does_not_overwrite_an_army_withdrawal() {
    use oxide_sim::bot::{ArmyState, Dials, Executive};

    let mut obs = obs_with_home();
    obs.my_units = vec![unit_obs(1, 0, UnitKind::Flakhound, 8, 4)];
    obs.enemy_units = vec![unit_obs(20, 1, UnitKind::Sentinel, 9, 4)];
    let staging = TilePos::new(4, 4);
    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 1 }],
        &[],
    );
    let _ = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    let mut legacy_retarget_exec = exec.clone();
    let retreat = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(exec.armies()[0].state, ArmyState::Withdrawing);
    assert!(matches!(
        retreat.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(1)] && *goal == staging
    ));

    let enlisted: Vec<_> = exec.enlisted().collect();
    let intents = UtilityPolicy::new().think_player_facing(
        &Dials::overseer(),
        &obs,
        exec.armies(),
        &enlisted,
        &[],
        &public_map(&obs),
    );
    assert!(
        intents.iter().all(|intent| !matches!(
            intent,
            Intent::PushArmy { army, .. } if *army == exec.armies()[0].id
        )),
        "the retreat must remain the body's only queue-replacing order: {intents:?}"
    );

    let legacy_retreat = legacy_retarget_exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(matches!(
        legacy_retreat.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(1)] && *goal == staging
    ));
    let legacy_enlisted: Vec<_> = legacy_retarget_exec.enlisted().collect();
    let legacy = UtilityPolicy::new().think(
        &Dials::overseer(),
        &obs,
        legacy_retarget_exec.armies(),
        &legacy_enlisted,
    );
    assert!(
        legacy.iter().any(|intent| matches!(
            intent,
            Intent::PushArmy { army, .. } if *army == exec.armies()[0].id
        )),
        "the profile-free Overseer retains its historical retargeting path"
    );

    obs.my_units[0].tile = staging;
    obs.my_units[0].idle = true;
    obs.enemy_units[0].tile = TilePos::new(6, 4);
    obs.tick = Dials::overseer().cadence;
    let settled = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(settled.is_empty());
    assert_eq!(exec.armies()[0].state, ArmyState::Withdrawing);
    let enlisted: Vec<_> = exec.enlisted().collect();
    let player_facing = UtilityPolicy::new().think_player_facing(
        &Dials::overseer(),
        &obs,
        exec.armies(),
        &enlisted,
        &[],
        &public_map(&obs),
    );
    assert!(
        player_facing.iter().all(|intent| !matches!(
            intent,
            Intent::PushArmy { army, .. } if *army == exec.armies()[0].id
        )),
        "an army holding its retreat line must not recommit to the same losing contact: {player_facing:?}"
    );

    for _ in 0..4 {
        obs.tick += Dials::overseer().cadence;
        assert!(
            exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2))
                .is_empty()
        );
        assert_eq!(
            exec.armies()[0].state,
            ArmyState::Withdrawing,
            "a faster think cadence must not turn fallback contact into focus/retreat churn"
        );
    }
    let mut settled_legacy = exec.clone();
    assert!(
        settled_legacy
            .maintain(PlayerId(0), &obs, TilePos::new(2, 2))
            .is_empty()
    );
    assert_eq!(settled_legacy.armies()[0].state, ArmyState::Staging);

    obs.enemy_units[0].tile = TilePos::new(12, 4);
    obs.tick += Dials::overseer().cadence;
    assert!(
        exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2))
            .is_empty()
    );
    assert_eq!(
        exec.armies()[0].state,
        ArmyState::Withdrawing,
        "the continuously visible threat remains inside the routed fight area even after direct contact breaks"
    );

    obs.enemy_units[0].tile = TilePos::new(18, 4);
    obs.tick += Dials::overseer().cadence;
    assert!(
        exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2))
            .is_empty()
    );
    assert_eq!(exec.armies()[0].state, ArmyState::Staging);

    obs.enemy_units[0].tile = TilePos::new(6, 4);
    obs.tick += Dials::overseer().cadence;
    let contact = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(contact.is_empty());
    assert_eq!(exec.armies()[0].state, ArmyState::Engaging);
    let enlisted: Vec<_> = exec.enlisted().collect();
    let engaged = UtilityPolicy::new().think_player_facing(
        &Dials::overseer(),
        &obs,
        exec.armies(),
        &enlisted,
        &[],
        &public_map(&obs),
    );
    assert!(
        engaged.iter().all(|intent| !matches!(
            intent,
            Intent::PushArmy { army, .. } if *army == exec.armies()[0].id
        )),
        "defense must not replace the order of a body already handling local contact: {engaged:?}"
    );

    obs.tick += Dials::overseer().cadence;
    let mut legacy_exec = exec.clone();
    let holding = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(holding.is_empty());
    assert_eq!(exec.armies()[0].state, ArmyState::Engaging);

    let legacy_retreat = legacy_exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(matches!(
        legacy_retreat.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(1)] && *goal == staging
    ));
}

#[test]
fn a_routed_defender_musters_before_retrying_the_same_remote_fight() {
    use oxide_sim::bot::{ArmyState, Dials, Executive};

    let home = TilePos::new(2, 2);
    let staging = TilePos::new(4, 4);
    let expansion = TilePos::new(18, 8);
    let mut obs = obs_with_home();
    obs.faction = Faction::Cupric;
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Foundry, 18, 8));
    obs.my_queues.push(Vec::new());
    obs.my_units = vec![unit_obs(1, 0, UnitKind::Stinger, 17, 8)];
    obs.enemy_units = (20..=22)
        .map(|id| unit_obs(id, 1, UnitKind::Sentinel, 18 + (id as i32 - 21), 8))
        .collect();

    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 1 }],
        &[],
    );
    let army_id = exec.armies()[0].id;
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army: army_id,
            target: expansion,
        }],
        &[],
    );

    let first_contact = exec.maintain_player_facing(PlayerId(0), &obs, home);
    assert!(first_contact.is_empty());
    assert_eq!(exec.armies()[0].state, ArmyState::Engaging);
    obs.tick += 1;
    let retreat = exec.maintain_player_facing(PlayerId(0), &obs, home);
    assert!(matches!(
        retreat.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(1)] && *goal == staging
    ));
    assert_eq!(exec.armies()[0].state, ArmyState::Withdrawing);

    obs.my_units[0].tile = staging;
    obs.my_units[0].idle = true;
    obs.tick += 1;
    assert!(
        exec.maintain_player_facing(PlayerId(0), &obs, home)
            .is_empty()
    );
    assert_eq!(
        exec.armies()[0].state,
        ArmyState::Withdrawing,
        "the same visible force still contests the fight that routed this body"
    );

    let enlisted: Vec<_> = exec.enlisted().collect();
    let outmatched = UtilityPolicy::new().think_player_facing(
        &Dials::overseer(),
        &obs,
        exec.armies(),
        &enlisted,
        &[],
        &public_map(&obs),
    );
    assert!(
        outmatched.iter().all(|intent| !matches!(
            intent,
            Intent::PushArmy { army, .. } if *army == army_id
        )),
        "a body must not bounce between home and the same catastrophic defense: {outmatched:?}"
    );
    assert!(
        outmatched
            .iter()
            .any(|intent| matches!(intent, Intent::FormArmy { .. })),
        "the bot must keep mustering while the visible matchup is too weak: {outmatched:?}"
    );

    obs.my_units.extend([
        unit_obs(2, 0, UnitKind::Scuttler, 4, 4),
        unit_obs(3, 0, UnitKind::Scuttler, 5, 4),
        unit_obs(4, 0, UnitKind::Scuttler, 4, 5),
        unit_obs(5, 0, UnitKind::Scuttler, 5, 5),
        unit_obs(6, 0, UnitKind::Scuttler, 4, 6),
        unit_obs(7, 0, UnitKind::Scuttler, 5, 6),
    ]);
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 6 }],
        &[],
    );
    let fresh_army = exec
        .armies()
        .iter()
        .find(|army| army.id != army_id)
        .expect("fresh production musters separately from the routed body")
        .id;
    let enlisted: Vec<_> = exec.enlisted().collect();
    let reinforced = UtilityPolicy::new().think_player_facing(
        &Dials::overseer(),
        &obs,
        exec.armies(),
        &enlisted,
        &[],
        &public_map(&obs),
    );
    assert!(
        reinforced.iter().any(|intent| matches!(
            intent,
            Intent::PushArmy { army, target }
                if *army == fresh_army && *target == expansion
        )),
        "a fresh ground-capable body can reopen defense without recommitting the routed force: {reinforced:?}"
    );
    assert!(reinforced.iter().all(|intent| !matches!(
        intent,
        Intent::PushArmy { army, .. } if *army == army_id
    )));
}

#[test]
fn player_facing_army_at_a_live_objective_is_not_reissued_every_think() {
    use oxide_sim::bot::{ArmyState, Dials, Executive};

    let target = TilePos::new(18, 8);
    let staging = TilePos::new(5, 4);
    let mut obs = obs_with_home();
    obs.my_units = (1..=4)
        .map(|id| unit_obs(id, 0, UnitKind::Sentinel, 4 + id as i32, 4))
        .collect();
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::Foundry, 18, 8)];
    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 4 }],
        &[],
    );
    let army_id = exec.armies()[0].id;
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army: army_id,
            target,
        }],
        &[],
    );

    for (unit, (x, y)) in obs
        .my_units
        .iter_mut()
        .zip([(16, 8), (17, 8), (18, 7), (18, 9)])
    {
        unit.tile = TilePos::new(x, y);
        unit.idle = false;
    }
    obs.tick = 6;
    let mut legacy_exec = exec.clone();
    let maintenance = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(maintenance.is_empty());
    assert_eq!(exec.armies()[0].state, ArmyState::Staging);
    assert_eq!(exec.armies()[0].staging, target);

    let mut dials = Dials::overseer();
    dials.army_size = 4;
    dials.own_strength_scale = u16::MAX;
    dials.enemy_strength_scale = 0;
    let enlisted: Vec<_> = exec.enlisted().collect();
    let player_facing = UtilityPolicy::new().think_player_facing(
        &dials,
        &obs,
        exec.armies(),
        &enlisted,
        &[],
        &public_map(&obs),
    );
    assert!(
        player_facing.iter().all(|intent| !matches!(
            intent,
            Intent::PushArmy { army, target: next }
                if *army == army_id && *next == target
        )),
        "the standing attack-move must keep owning the live objective: {player_facing:?}"
    );

    let legacy_maintenance = legacy_exec.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(legacy_maintenance.is_empty());
    let legacy_enlisted: Vec<_> = legacy_exec.enlisted().collect();
    let legacy = UtilityPolicy::new().think(&dials, &obs, legacy_exec.armies(), &legacy_enlisted);
    assert!(
        legacy.iter().any(|intent| matches!(
            intent,
            Intent::PushArmy { army, target: next }
                if *army == army_id && *next == target
        )),
        "the frozen Overseer retains its historical arrival behavior"
    );
}

#[test]
fn player_facing_completed_forward_army_rejoins_the_safe_muster() {
    use oxide_sim::bot::{ArmyState, Executive};

    let target = TilePos::new(18, 8);
    let forward_staging = TilePos::new(6, 4);
    let home_staging = TilePos::new(2, 2);
    let mut obs = obs_with_home();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 5, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 6, 4),
        unit_obs(3, 0, UnitKind::Sentinel, 2, 2),
        unit_obs(4, 0, UnitKind::Sentinel, 3, 2),
    ];
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::Foundry, 18, 8)];

    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: forward_staging,
            size: 2,
        }],
        &[],
    );
    let forward = exec.armies()[0].id;
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army: forward,
            target,
        }],
        &[],
    );
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: home_staging,
            size: 2,
        }],
        &[],
    );
    assert_eq!(exec.armies().len(), 2, "test premise");

    for (unit, tile) in obs
        .my_units
        .iter_mut()
        .take(2)
        .zip([TilePos::new(17, 8), target])
    {
        unit.tile = tile;
        unit.idle = false;
    }
    obs.tick = 6;
    assert!(
        exec.maintain_player_facing(PlayerId(0), &obs, home_staging)
            .is_empty()
    );
    assert_eq!(exec.armies().len(), 2);
    assert_eq!(exec.armies()[0].state, ArmyState::Staging);
    assert_eq!(exec.armies()[0].target, Some(target));

    let mut legacy = exec.clone();
    obs.enemy_buildings.clear();
    for unit in obs.my_units.iter_mut().take(2) {
        unit.idle = true;
    }
    obs.tick = 12;
    assert!(
        exec.maintain_player_facing(PlayerId(0), &obs, home_staging)
            .is_empty()
    );
    assert_eq!(exec.armies().len(), 1);
    assert_eq!(exec.armies()[0].members, vec![UnitId(3), UnitId(4)]);

    let commands = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: home_staging,
            size: 4,
        }],
        &[],
    );
    assert_eq!(
        exec.armies()[0].members,
        vec![UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
    );
    assert!(matches!(
        commands.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(1), UnitId(2)] && *goal == home_staging
    ));

    assert!(legacy.maintain(PlayerId(0), &obs, home_staging).is_empty());
    assert_eq!(
        legacy.armies().len(),
        2,
        "the frozen Overseer retains a completed forward staging body"
    );
}

#[test]
fn player_facing_refused_forward_army_cannot_mask_a_reachable_muster() {
    use oxide_sim::bot::{ArmyState, Executive};

    let left = TilePos::new(5, 4);
    let right = TilePos::new(7, 4);
    let mut obs = obs_with_home();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 8, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 4, 4),
    ];
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();

    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: right,
            size: 1,
        }],
        &[],
    );
    let forward = exec.armies()[0].id;
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army: forward,
            target: left,
        }],
        &[],
    );
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: left,
            size: 1,
        }],
        &[],
    );
    assert_eq!(exec.armies().len(), 2, "test premise");

    obs.tick = 6;
    let _ = exec.maintain_player_facing(PlayerId(0), &obs, left);
    assert_eq!(exec.armies()[0].state, ArmyState::Pushing);
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army: forward,
            target: TilePos::new(4, 4),
        }],
        &[],
    );
    let mut legacy = exec.clone();

    obs.tick = 12;
    let _ = exec.maintain_player_facing(PlayerId(0), &obs, left);
    assert_eq!(exec.armies().len(), 1);
    assert_eq!(exec.armies()[0].members, vec![UnitId(2)]);
    let commands = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: left,
            size: 2,
        }],
        &[],
    );
    assert!(commands.is_empty());
    assert_eq!(exec.armies()[0].members, vec![UnitId(2)]);

    let _ = legacy.maintain(PlayerId(0), &obs, left);
    assert_eq!(legacy.armies().len(), 2);
    assert_eq!(legacy.armies()[0].state, ArmyState::Staging);
}

#[test]
fn player_facing_army_finishes_a_harmless_target_without_restarting_its_march() {
    use oxide_sim::bot::{ArmyState, Executive};

    let mut obs = obs_with_home();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 6, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 6, 5),
    ];
    obs.enemy_units = vec![unit_obs(20, 1, UnitKind::Flakhound, 7, 4)];
    assert!(
        obs.my_units
            .iter()
            .all(|unit| unit.tile.chebyshev(obs.enemy_units[0].tile) <= 1),
        "ordinary acquisition must be able to finish the harmless contact without a chase order"
    );
    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: TilePos::new(4, 4),
            size: 2,
        }],
        &[],
    );
    let army = exec.armies()[0].id;
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::PushArmy {
            army,
            target: TilePos::new(18, 8),
        }],
        &[],
    );
    let _ = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(exec.armies()[0].state, ArmyState::Engaging);

    obs.tick = 6;
    let mut legacy = exec.clone();
    let commands = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(exec.armies()[0].state, ArmyState::Engaging);
    assert!(
        commands.is_empty(),
        "an in-range harmless contact should not replace the objective march with an explicit chase"
    );

    let legacy_commands = legacy.maintain(PlayerId(0), &obs, TilePos::new(2, 2));
    assert_eq!(legacy.armies()[0].state, ArmyState::Pushing);
    assert!(
        legacy_commands
            .iter()
            .any(|command| matches!(command.command, Command::AttackMove { .. }))
    );
}

#[test]
fn player_facing_ground_armies_do_not_pursue_aircraft_over_unstandable_ground() {
    use oxide_sim::bot::{ArmyState, Executive};

    let mut obs = obs_with_home();
    obs.my_units = (1..=7)
        .map(|id| {
            unit_obs(
                id,
                0,
                UnitKind::Sentinel,
                5 + id as i32 % 3,
                4 + id as i32 / 3,
            )
        })
        .collect();
    let aircraft = UnitId(20);
    obs.enemy_units = vec![unit_obs(aircraft.0, 1, UnitKind::Moth, 9, 5)];
    obs.known_rock.push(TilePos::new(9, 5));

    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: TilePos::new(6, 5),
            size: 7,
        }],
        &[],
    );
    let first_contact = exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
    assert!(first_contact.is_empty());
    assert_eq!(exec.armies()[0].state, ArmyState::Engaging);

    obs.tick = 6;
    let mut legacy = exec.clone();
    assert!(
        exec.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2))
            .is_empty(),
        "ground defenders answer nearby aircraft through their standing order and auto-acquisition instead of pathing onto the aircraft's unstandable tile"
    );
    assert_eq!(exec.armies()[0].focus, None);

    // Salvo-priced fight strength reads the Moth's full bombing stick as
    // a real threat, so the legacy arm no longer commits a direct pursuit
    // Attack: it withdraws to its own staging ground through an
    // attack-move that answers fire on the way, and never paths onto the
    // aircraft's unstandable tile.
    assert!(matches!(
        legacy
            .maintain(PlayerId(0), &obs, TilePos::new(2, 2))
            .as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &(1..=7).map(UnitId).collect::<Vec<_>>()
            && *goal == TilePos::new(6, 5)
    ));
    assert_eq!(
        legacy.armies()[0].state,
        oxide_sim::bot::ArmyState::Withdrawing
    );
}

#[test]
fn a_known_scrap_wall_suppresses_an_impossible_ground_push() {
    let mut obs = obs_with_home();
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::Foundry, 18, 8)];
    obs.my_units = (1..=6)
        .map(|id| unit_obs(id, 0, UnitKind::Sentinel, 5 + id as i32, 6))
        .collect();
    obs.known_scrap = (0..obs.map_height)
        .map(|y| (TilePos::new(12, y), 100))
        .collect();

    let blocked = staged_ground_push_intents(&obs);
    assert!(
        blocked
            .iter()
            .all(|intent| !matches!(intent, Intent::PushArmy { .. })),
        "known impassable salvage must stop an AttackMove storm: {blocked:?}"
    );

    obs.known_scrap.retain(|(tile, _)| tile.y != 6);
    let open = staged_ground_push_intents(&obs);
    assert!(
        open.iter()
            .any(|intent| matches!(intent, Intent::PushArmy { .. })),
        "one known-open gap restores the legal push: {open:?}"
    );
}

#[test]
fn a_known_building_wall_suppresses_an_impossible_ground_push() {
    let mut obs = obs_with_home();
    obs.enemy_buildings = vec![building_obs(5, 1, BuildingKind::Foundry, 18, 8)];
    obs.my_units = (1..=6)
        .map(|id| unit_obs(id, 0, UnitKind::Sentinel, 5 + id as i32, 6))
        .collect();
    obs.ally_buildings = (0..obs.map_height)
        .map(|y| building_obs(100 + y as u32, 2, BuildingKind::Turret, 12, y))
        .collect();

    let blocked = staged_ground_push_intents(&obs);
    assert!(
        blocked
            .iter()
            .all(|intent| !matches!(intent, Intent::PushArmy { .. })),
        "known structures must stop an AttackMove storm: {blocked:?}"
    );

    obs.ally_buildings.retain(|building| building.anchor.y != 6);
    let open = staged_ground_push_intents(&obs);
    assert!(
        open.iter()
            .any(|intent| matches!(intent, Intent::PushArmy { .. })),
        "one known-open gap restores the legal push: {open:?}"
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
fn player_facing_muster_drafts_only_fighters_with_a_known_ground_route() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 3, 4),
        unit_obs(1, 0, UnitKind::Flakhound, 4, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 8, 4),
        unit_obs(3, 0, UnitKind::Flakhound, 9, 4),
    ];
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();
    let staging = TilePos::new(5, 4);
    let form = [Intent::FormArmy { staging, size: 4 }];

    let mut player_facing = Executive::new();
    let commands = player_facing.apply_with_reservations(PlayerId(0), &obs, &form, &[]);
    assert_eq!(
        player_facing.armies()[0].members,
        vec![UnitId(0), UnitId(1)],
        "the nearby reachable component forms a useful partial army"
    );
    assert!(matches!(
        commands.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(0), UnitId(1)] && *goal == staging
    ));

    let mut overseer = Executive::new();
    let _ = overseer.apply(PlayerId(0), &obs, &form);
    assert_eq!(
        overseer.armies()[0].members.len(),
        4,
        "profile-free lowering preserves the Overseer's optimistic draft"
    );
}

#[test]
fn player_facing_reinforcement_skips_an_unknown_gulf_until_a_route_is_mapped() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.explored.fill(false);
    for y in 0..obs.map_height {
        for x in 0..=5 {
            obs.explored[(y * obs.map_width + x) as usize] = true;
        }
        for x in 7..obs.map_width {
            obs.explored[(y * obs.map_width + x) as usize] = true;
        }
    }
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 4, 4),
        unit_obs(1, 0, UnitKind::Flakhound, 7, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 1, 4),
    ];
    let staging = TilePos::new(5, 4);
    let mut exec = Executive::new();
    let _ = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 1 }],
        &[],
    );

    let commands = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 3 }],
        &[],
    );
    assert_eq!(exec.armies()[0].members, vec![UnitId(0), UnitId(2)]);
    assert!(matches!(
        commands.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(2)] && *goal == staging
    ));

    obs.explored[(4 * obs.map_width + 6) as usize] = true;
    let commands = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 3 }],
        &[],
    );
    assert_eq!(
        exec.armies()[0].members,
        vec![UnitId(0), UnitId(1), UnitId(2)]
    );
    assert!(matches!(
        commands.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::AttackMove { units, goal, queue: false },
            ..
        }] if units == &vec![UnitId(1)] && *goal == staging
    ));
}

#[test]
fn player_facing_muster_consolidates_nearby_staged_bodies() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Sentinel, 3 + id as i32, 4))
        .collect();
    let first = TilePos::new(6, 4);
    let nearby = TilePos::new(7, 5);
    let intents = |staging, size| [Intent::FormArmy { staging, size }];

    let mut player_facing = Executive::new();
    let _ = player_facing.apply(PlayerId(0), &obs, &intents(first, 2));
    let _ = player_facing.apply(PlayerId(0), &obs, &intents(nearby, 2));
    assert_eq!(player_facing.armies().len(), 2, "test premise");

    let commands =
        player_facing.apply_with_reservations(PlayerId(0), &obs, &intents(first, 4), &[]);
    assert!(
        commands.is_empty(),
        "consolidating already enlisted members needs no sim command"
    );
    assert_eq!(player_facing.armies().len(), 1);
    assert_eq!(
        player_facing.armies()[0].members,
        vec![UnitId(0), UnitId(1), UnitId(2), UnitId(3)]
    );
    assert_eq!(player_facing.armies()[0].staging, first);

    let mut overseer = Executive::new();
    let _ = overseer.apply(PlayerId(0), &obs, &intents(first, 2));
    let _ = overseer.apply(PlayerId(0), &obs, &intents(nearby, 2));
    let _ = overseer.apply(PlayerId(0), &obs, &intents(first, 4));
    assert_eq!(
        overseer.armies().len(),
        2,
        "profile-free lowering retains exact-rally army bookkeeping"
    );
}

#[test]
fn player_facing_muster_does_not_consolidate_bodies_across_a_known_wall() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 3, 4),
        unit_obs(1, 0, UnitKind::Sentinel, 4, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 8, 4),
        unit_obs(3, 0, UnitKind::Sentinel, 9, 4),
    ];
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(6, y)).collect();
    let left = TilePos::new(5, 4);
    let right = TilePos::new(7, 4);
    let mut exec = Executive::new();
    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: left,
            size: 2,
        }],
    );
    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: right,
            size: 2,
        }],
    );
    assert_eq!(exec.armies().len(), 2, "test premise");

    let commands = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: left,
            size: 4,
        }],
        &[],
    );

    assert!(commands.is_empty());
    assert_eq!(exec.armies().len(), 2);
    let mut left_members = exec.armies()[0].members.clone();
    left_members.sort_unstable();
    let mut right_members = exec.armies()[1].members.clone();
    right_members.sort_unstable();
    assert_eq!(left_members, vec![UnitId(0), UnitId(1)]);
    assert_eq!(right_members, vec![UnitId(2), UnitId(3)]);
}

#[test]
fn player_facing_rear_wait_keeps_unrepaired_units_out_of_voluntary_musters() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 4, 4),
        unit_obs(1, 0, UnitKind::Sentinel, 5, 4),
    ];
    let staging = TilePos::new(6, 4);
    let rear = TilePos::new(2, 2);
    let form = [Intent::FormArmy { staging, size: 2 }];
    let mut player_facing = Executive::new();
    let _ = player_facing.apply_with_reservations(PlayerId(0), &obs, &form, &[]);

    obs.my_units[0].hp = 1;
    let retreat = player_facing.maintain_player_facing(PlayerId(0), &obs, rear);
    assert!(retreat.iter().any(|command| matches!(
        &command.command,
        Command::Move { units, goal, queue: false }
            if units == &vec![UnitId(0)] && *goal == rear
    )));

    obs.my_units[0].tile = rear;
    obs.tick = 1_199;
    let _ = player_facing.maintain_player_facing(PlayerId(0), &obs, rear);
    let _ = player_facing.apply_with_reservations(PlayerId(0), &obs, &form, &[]);
    assert!(
        player_facing
            .armies()
            .iter()
            .all(|army| !army.members.contains(&UnitId(0))),
        "the repair window remains a real opportunity to recover"
    );

    obs.tick = 1_200;
    let _ = player_facing.maintain_player_facing(PlayerId(0), &obs, rear);
    let _ = player_facing.apply_with_reservations(PlayerId(0), &obs, &form, &[]);
    assert!(
        player_facing
            .armies()
            .iter()
            .all(|army| !army.members.contains(&UnitId(0))),
        "rear timeout must not turn a nearly destroyed machine into voluntary assault strength"
    );

    obs.my_units[0].hp = UnitKind::Sentinel.stats().max_hp;
    obs.tick += 1;
    let _ = player_facing.maintain_player_facing(PlayerId(0), &obs, rear);
    let commands = player_facing.apply_with_reservations(PlayerId(0), &obs, &form, &[]);
    assert!(
        player_facing.armies()[0].members.contains(&UnitId(0))
            && commands.iter().any(|command| matches!(
                &command.command,
                Command::AttackMove { units, goal, queue: false }
                    if units.contains(&UnitId(0)) && *goal == staging
            )),
        "a genuinely repaired machine must become eligible for the next voluntary muster"
    );

    let mut overseer = Executive::new();
    obs.tick = 0;
    obs.my_units[0].tile = TilePos::new(4, 4);
    obs.my_units[0].hp = UnitKind::Sentinel.stats().max_hp;
    let _ = overseer.apply(PlayerId(0), &obs, &form);
    obs.my_units[0].hp = 1;
    let _ = overseer.maintain(PlayerId(0), &obs, rear);
    obs.my_units[0].tile = rear;
    obs.tick = 10_000;
    let _ = overseer.maintain(PlayerId(0), &obs, rear);
    let _ = overseer.apply(PlayerId(0), &obs, &form);
    assert!(
        overseer
            .armies()
            .iter()
            .all(|army| !army.members.contains(&UnitId(0))),
        "profile-free maintenance keeps its historical permanent rear line"
    );
}

#[test]
fn exact_group_intents_lower_canonical_live_owned_members_only() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(3, 0, UnitKind::Bombard, 4, 3),
        unit_obs(5, 0, UnitKind::Warden, 5, 3),
        unit_obs(7, 0, UnitKind::Lancer, 6, 3),
        unit_obs(9, 1, UnitKind::Sentinel, 7, 3),
        unit_obs(11, 0, UnitKind::Sentinel, 8, 3),
        unit_obs(13, 0, UnitKind::Tender, 9, 3),
    ];
    obs.my_units[1].hp = 0;
    let move_goal = TilePos::new(10, 5);
    let march_goal = TilePos::new(12, 6);
    let target = Target::Building(BuildingId(42));

    let commands = Executive::new().apply(
        PlayerId(0),
        &obs,
        &[
            Intent::MoveUnits {
                units: vec![
                    UnitId(11),
                    UnitId(5),
                    UnitId(3),
                    UnitId(5),
                    UnitId(99),
                    UnitId(9),
                ],
                goal: move_goal,
            },
            Intent::AttackMoveUnits {
                units: vec![UnitId(1)],
                goal: march_goal,
            },
            Intent::AttackUnits {
                units: vec![UnitId(7)],
                target,
            },
            Intent::RepairUnits {
                welders: vec![UnitId(13), UnitId(1), UnitId(13)],
                target: UnitId(5),
            },
        ],
    );

    assert_eq!(
        commands,
        vec![
            oxide_sim::PlayerCommand {
                player: PlayerId(0),
                command: Command::Move {
                    units: vec![UnitId(5), UnitId(11)],
                    goal: move_goal,
                    queue: false,
                },
            },
            oxide_sim::PlayerCommand {
                player: PlayerId(0),
                command: Command::AttackMove {
                    units: vec![UnitId(1)],
                    goal: march_goal,
                    queue: false,
                },
            },
            oxide_sim::PlayerCommand {
                player: PlayerId(0),
                command: Command::Attack {
                    units: vec![UnitId(7)],
                    target,
                    queue: false,
                },
            },
            oxide_sim::PlayerCommand {
                player: PlayerId(0),
                command: Command::RepairUnit {
                    units: vec![UnitId(13)],
                    target: UnitId(5),
                    queue: false,
                },
            },
        ],
        "dead, non-owned, missing, and duplicate ids are removed before lowering"
    );

    let empty = Executive::new().apply(
        PlayerId(0),
        &obs,
        &[Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(9), UnitId(99)],
            target,
        }],
    );
    assert!(empty.is_empty(), "an empty filtered group emits no command");
}

#[test]
fn exact_reservations_survive_an_earlier_army_draft_and_transfer_ownership() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(0, 0, UnitKind::Sentinel, 3, 3),
        unit_obs(1, 0, UnitKind::Sentinel, 4, 3),
        unit_obs(2, 0, UnitKind::Sentinel, 5, 3),
    ];
    let staging = TilePos::new(8, 5);
    let operation_goal = TilePos::new(16, 8);
    let mut exec = Executive::new();

    let commands = exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[
            Intent::FormArmy { staging, size: 3 },
            Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: operation_goal,
            },
        ],
        &[UnitId(2), UnitId(99)],
    );

    assert_eq!(exec.armies()[0].members.len(), 2);
    assert!(exec.armies()[0].members.contains(&UnitId(0)));
    assert!(exec.armies()[0].members.contains(&UnitId(1)));
    assert!(!exec.armies()[0].members.contains(&UnitId(2)));
    assert!(commands.iter().any(|command| matches!(
        &command.command,
        Command::AttackMove { units, goal, queue: false }
            if units.len() == 2
                && units.contains(&UnitId(0))
                && units.contains(&UnitId(1))
                && *goal == staging
    )));
    assert!(commands.iter().any(|command| matches!(
        &command.command,
        Command::Move { units, goal, queue: false }
            if units == &vec![UnitId(2)] && *goal == operation_goal
    )));

    let _ = exec.apply(
        PlayerId(0),
        &obs,
        &[Intent::AttackMoveUnits {
            units: vec![UnitId(1)],
            goal: operation_goal,
        }],
    );
    assert_eq!(
        exec.armies()[0].members,
        vec![UnitId(0)],
        "an explicit operation transfers its members out of army lifecycle bookkeeping"
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
fn profile_free_lowering_preserves_the_ferrys_distance_ranked_rider_order() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 3, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 4, 4),
        unit_obs(3, 0, UnitKind::Sentinel, 5, 4),
        unit_obs(10, 0, UnitKind::Skyhook, 4, 5),
    ];

    let commands = Executive::new().apply(
        PlayerId(0),
        &obs,
        &[Intent::Load {
            transport: UnitId(10),
            riders: vec![UnitId(3), UnitId(1), UnitId(2)],
        }],
    );

    assert_eq!(
        commands,
        vec![oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: Command::Load {
                units: vec![UnitId(3), UnitId(1), UnitId(2)],
                transport: UnitId(10),
                queue: false,
            },
        }],
        "the frozen Overseer's command stream keeps utility preference order"
    );
}

#[test]
fn exact_loading_uses_its_reservations_without_stealing_same_think_claims() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Sentinel, 3, 4),
        unit_obs(2, 0, UnitKind::Sentinel, 4, 4),
        unit_obs(3, 0, UnitKind::Sentinel, 5, 4),
        unit_obs(10, 0, UnitKind::Skyhook, 4, 5),
    ];
    let strategic_goal = TilePos::new(18, 8);
    let commands = Executive::new().apply_with_reservations(
        PlayerId(0),
        &obs,
        &[
            Intent::MoveUnits {
                units: vec![UnitId(1)],
                goal: strategic_goal,
            },
            Intent::Load {
                transport: UnitId(10),
                riders: vec![UnitId(1), UnitId(2), UnitId(3)],
            },
        ],
        &[UnitId(3)],
    );

    assert!(matches!(
        commands.first().map(|command| &command.command),
        Some(Command::Move { units, goal, queue: false })
            if units == &vec![UnitId(1)] && *goal == strategic_goal
    ));
    assert!(matches!(
        commands.get(1).map(|command| &command.command),
        Some(Command::Load { units, transport, queue: false })
            if units == &vec![UnitId(2), UnitId(3)] && *transport == UnitId(10)
    ));
    assert_eq!(commands.len(), 2);

    let transport_reserved = Executive::new().apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::Load {
            transport: UnitId(10),
            riders: vec![UnitId(2)],
        }],
        &[UnitId(10)],
    );
    assert!(matches!(
        transport_reserved.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::Load {
                units,
                transport: UnitId(10),
                queue: false,
            },
            ..
        }] if units == &vec![UnitId(2)]
    ));

    let already_moving = Executive::new().apply_with_reservations(
        PlayerId(0),
        &obs,
        &[
            Intent::MoveUnits {
                units: vec![UnitId(10)],
                goal: strategic_goal,
            },
            Intent::Load {
                transport: UnitId(10),
                riders: vec![UnitId(2)],
            },
        ],
        &[UnitId(10), UnitId(2)],
    );
    assert_eq!(
        already_moving.len(),
        1,
        "an earlier exact command owns the transport for this think"
    );
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
fn an_unreachable_paid_site_does_not_starve_reachable_construction() {
    let mut obs = obs_with_home();
    obs.scrap = 2_000;
    obs.my_queues[0] = vec![UnitKind::Sentinel, UnitKind::Sentinel];
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 6))
        .collect();
    let mut orphan = building_obs(1, 0, BuildingKind::Turret, 19, 1);
    orphan.built = false;
    orphan.hp = 1;
    obs.my_buildings.push(orphan);
    obs.my_queues.push(Vec::new());
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(12, y)).collect();

    let intents = player_think(&mut UtilityPolicy::new(), &Dials::full(), &obs);

    assert!(
        !intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Turret,
                anchor,
            } if *anchor == TilePos::new(19, 1)
        )),
        "no free player-facing builder can reach the paid site: {intents:?}"
    );
    assert!(
        intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Fabricator,
                anchor,
            } if anchor.x < 12
        )),
        "the disconnected orphan must yield to reachable construction: {intents:?}"
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
fn executive_defers_a_build_claim_outside_current_sight() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![unit_obs(1, 0, UnitKind::Harvester, 3, 3)];
    let anchor = TilePos::new(14, 7);
    let (width, height) = BuildingKind::Fabricator.base_stats().size;
    for dy in 0..height {
        for dx in 0..width {
            let tile = anchor.offset(dx, dy);
            let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
            obs.visible[index] = false;
        }
    }

    let commands = Executive::new().apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::Build {
            kind: BuildingKind::Fabricator,
            anchor,
        }],
        &[],
    );

    assert_eq!(commands.len(), 1);
    assert!(matches!(
        commands[0].command,
        Command::Build {
            kind: BuildingKind::Fabricator,
            anchor: actual,
            defer: true,
            ..
        } if actual == anchor
    ));

    let legacy = Executive::new().apply(
        PlayerId(0),
        &obs,
        &[Intent::Build {
            kind: BuildingKind::Fabricator,
            anchor,
        }],
    );
    assert!(matches!(
        legacy[0].command,
        Command::Build { defer: false, .. }
    ));

    obs.visible.fill(true);
    let commands = Executive::new().apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::Build {
            kind: BuildingKind::Fabricator,
            anchor,
        }],
        &[],
    );
    assert!(matches!(
        commands[0].command,
        Command::Build { defer: false, .. }
    ));
}

#[test]
fn player_facing_builds_choose_a_reachable_worker() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_base();
    obs.my_units = vec![
        unit_obs(1, 0, UnitKind::Harvester, 13, 5),
        unit_obs(2, 0, UnitKind::Harvester, 3, 5),
    ];
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(12, y)).collect();
    let intent = Intent::Build {
        kind: BuildingKind::Fabricator,
        anchor: TilePos::new(10, 5),
    };

    let commands = Executive::new().apply_with_reservations(
        PlayerId(0),
        &obs,
        std::slice::from_ref(&intent),
        &[],
    );
    assert!(matches!(
        commands.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::Build { units, .. },
            ..
        }] if units == &[UnitId(2)]
    ));

    obs.my_units.remove(1);
    let commands = Executive::new().apply_with_reservations(
        PlayerId(0),
        &obs,
        std::slice::from_ref(&intent),
        &[],
    );
    assert!(
        commands.is_empty(),
        "a known-severed worker must not be sent"
    );

    let legacy = Executive::new().apply(PlayerId(0), &obs, &[intent]);
    assert!(matches!(
        legacy.as_slice(),
        [oxide_sim::PlayerCommand {
            command: Command::Build { units, .. },
            ..
        }] if units == &[UnitId(1)]
    ));
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
fn a_walking_foundry_founder_blocks_another_capital_commitment() {
    let mut obs = obs_with_home();
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Fabricator, 6, 2));
    obs.my_queues.push(Vec::new());
    obs.my_buildings
        .push(building_obs(2, 0, BuildingKind::Extractor, 20, 9));
    obs.my_queues.push(Vec::new());
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .collect();
    obs.known_scrap.clear();
    for y in 0..obs.map_height {
        for x in obs.map_width / 2..obs.map_width {
            let index = usize::try_from(y * obs.map_width + x).unwrap();
            obs.visible[index] = false;
        }
    }
    obs.scrap = 2_000;
    let mut dials = Dials::full();
    dials.deep_tech = false;
    dials.expansion = true;
    dials.scouting = false;
    let mut policy = UtilityPolicy::new();

    let first = player_think(&mut policy, &dials, &obs);
    let (founder, promised) = first
        .iter()
        .find_map(|intent| match intent {
            Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor,
            } => Some((*builder, *anchor)),
            _ => None,
        })
        .expect("a rich seat with an unserved frontier commits one expansion");
    let commands =
        oxide_sim::bot::Executive::new().apply_with_reservations(PlayerId(0), &obs, &first, &[]);
    assert!(
        commands.iter().any(|command| matches!(
            &command.command,
            Command::Build {
                units,
                kind: BuildingKind::Foundry,
                anchor,
                defer: true,
                ..
            } if units == &[founder] && *anchor == promised
        )),
        "the unpaid claim must remain a deferred Foundry command: {commands:?}"
    );

    obs.tick = oxide_sim::bot::difficulty::STRATEGIC_ADMISSION_CADENCE;
    obs.visible.fill(true);
    let founder = obs
        .my_units
        .iter_mut()
        .find(|unit| unit.id == founder)
        .expect("the selected founder remains alive");
    founder.idle = false;
    founder.founding = Some((BuildingKind::Foundry, promised));
    obs.my_units
        .extend((10..13).map(|id| unit_obs(id, 0, UnitKind::Sentinel, 5 + id as i32 - 10, 7)));
    obs.enemy_buildings
        .push(building_obs(90, 1, BuildingKind::Foundry, 18, 8));
    obs.scrap = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries have a price")
        .cost
        + UnitKind::Sentinel.stats().cost;
    let next = player_think(&mut policy, &dials, &obs);
    assert!(
        next.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )),
        "an unpaid Foundry founder already owns this cadence's capital commitment: {next:?}"
    );
    assert!(
        next.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Sentinel,
                ..
            }
        )),
        "the separately reserved promise must not bank a second unusable Foundry fund: {next:?}"
    );
}

#[test]
fn a_walking_fabricator_is_reserved_and_counts_as_the_tech_rung() {
    let mut obs = obs_with_home();
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .chain((10..13).map(|id| unit_obs(id, 0, UnitKind::Sentinel, 5 + id as i32 - 10, 7)))
        .collect();
    let promised = TilePos::new(9, 5);
    obs.my_units[0].idle = false;
    obs.my_units[0].founding = Some((BuildingKind::Fabricator, promised));
    obs.enemy_buildings
        .push(building_obs(90, 1, BuildingKind::Foundry, 18, 8));
    obs.scrap = BuildingKind::Fabricator
        .base_stats()
        .construction
        .expect("Fabricators have a price")
        .cost
        + UnitKind::Sentinel.stats().cost;
    let mut dials = Dials::full();
    dials.scouting = false;

    let player = player_think(&mut UtilityPolicy::new(), &dials, &obs);
    assert!(
        player.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Fabricator,
                ..
            }
        )),
        "a walking Fabricator already fills the one-off tech rung: {player:?}"
    );
    assert!(
        player.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Sentinel,
                ..
            }
        )),
        "only the unpaid Fabricator price is reserved; its residual bank remains usable: {player:?}"
    );

    let legacy = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(
        legacy.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Fabricator,
                ..
            }
        )),
        "the profile-free Overseer retains standing-building-only projection: {legacy:?}"
    );
}

#[test]
fn a_walking_extractor_claims_its_fixed_frame_once() {
    let mut obs = obs_with_home();
    obs.my_buildings
        .push(building_obs(1, 0, BuildingKind::Fabricator, 5, 2));
    obs.my_queues.push(Vec::new());
    let frame = TilePos::new(10, 7);
    obs.known_frames = vec![frame];
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .collect();
    obs.my_units[0].idle = false;
    obs.my_units[0].founding = Some((BuildingKind::Extractor, frame));
    obs.scrap = 1_000;
    let mut dials = Dials::full();
    dials.extractors = true;
    dials.scouting = false;

    let player = player_think(&mut UtilityPolicy::new(), &dials, &obs);
    assert!(
        player.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Extractor,
                anchor,
            } if *anchor == frame
        )),
        "a fixed frame already has one unpaid restoration claim: {player:?}"
    );

    let legacy = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(
        legacy.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Extractor,
                anchor,
            } if *anchor == frame
        )),
        "the profile-free Overseer retains its historical standing-site check: {legacy:?}"
    );
}

#[test]
fn deferred_build_stops_repairs_before_reusing_a_repairing_builder() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_with_home();
    obs.scrap = 1_000;
    obs.visible.fill(false);
    obs.my_units = (0..4)
        .map(|id| {
            let mut worker = unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5);
            worker.idle = false;
            worker.repairing = true;
            worker
        })
        .collect();
    let mut dials = Dials::full();
    dials.scouting = false;

    let intents = player_think(&mut UtilityPolicy::new(), &dials, &obs);
    let stop = intents
        .iter()
        .position(|intent| matches!(intent, Intent::StopUnits { .. }))
        .expect("the unpaid remote build cancels voluntary repair");
    let build = intents
        .iter()
        .position(|intent| matches!(intent, Intent::Build { .. }))
        .expect("the remote Fabricator is still planned");
    assert!(
        stop < build,
        "repair cancellation must lower before Build: {intents:?}"
    );

    let commands = Executive::new().apply_with_reservations(PlayerId(0), &obs, &intents, &[]);
    let stopped = commands.iter().find_map(|command| match &command.command {
        Command::Stop { units } => Some(units),
        _ => None,
    });
    let builder = commands.iter().find_map(|command| match &command.command {
        Command::Build {
            units, defer: true, ..
        } => units.first(),
        _ => None,
    });
    let builder = builder.expect("the unseen build lowers as a deferred claim");
    assert!(
        stopped.is_some_and(|units| units.contains(builder)),
        "the same repaired worker is stopped before receiving its new Found order: {commands:?}"
    );
}

#[test]
fn visible_paid_construction_leaves_existing_repair_work_alone() {
    let mut obs = obs_with_home();
    obs.scrap = 1_000;
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .collect();
    obs.my_units[0].idle = false;
    obs.my_units[0].repairing = true;
    let mut wounded = building_obs(7, 0, BuildingKind::Turret, 10, 5);
    wounded.hp = 1;
    obs.my_buildings.push(wounded);
    obs.my_queues.push(Vec::new());
    let mut dials = Dials::full();
    dials.scouting = false;

    let intents = player_think(&mut UtilityPolicy::new(), &dials, &obs);
    assert!(
        intents
            .iter()
            .all(|intent| !matches!(intent, Intent::StopUnits { .. })),
        "an immediately paid visible site creates no deferred obligation: {intents:?}"
    );
    assert!(
        intents
            .iter()
            .all(|intent| !matches!(intent, Intent::Repair { .. })),
        "the existing persistent repair continues without a replacement intent: {intents:?}"
    );
}

#[test]
fn an_underfunded_foundry_promise_escrows_every_player_facing_spend() {
    let mut obs = obs_with_home();
    for (id, kind, x) in [
        (1, BuildingKind::Fabricator, 5),
        (2, BuildingKind::Airworks, 8),
        (3, BuildingKind::Crucible, 11),
        (4, BuildingKind::Turret, 14),
    ] {
        obs.my_buildings.push(building_obs(id, 0, kind, x, 2));
        obs.my_queues.push(Vec::new());
    }
    let promised = TilePos::new(17, 8);
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 6))
        .collect();
    obs.my_units[0].idle = false;
    obs.my_units[0].founding = Some((BuildingKind::Foundry, promised));
    obs.my_units.push(unit_obs(10, 0, UnitKind::Tender, 7, 6));
    obs.my_units[4].idle = false;
    obs.my_units[4].repairing = true;
    let mut patient = unit_obs(11, 0, UnitKind::Sentinel, 8, 6);
    patient.hp = 1;
    obs.my_units.push(patient);
    obs.my_buildings[4].hp = 1;
    obs.known_scrap = vec![(TilePos::new(8, 9), 500)];
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("expansion Foundries have a price")
        .cost;
    obs.scrap = foundry_cost - 1;

    let mut dials = Dials::full();
    dials.adaptive_composition = true;
    dials.discretionary_slots = 6;
    dials.expansion = true;
    dials.scouting = false;
    let mut policy = UtilityPolicy::new();
    let _ = player_think(&mut policy, &dials, &obs);
    obs.tick = 2_016;
    let intents = player_think(&mut policy, &dials, &obs);

    assert!(
        intents.iter().any(|intent| matches!(
            intent,
            Intent::StopUnits { units } if units == &[UnitId(10)]
        )),
        "an active voluntary repair is cancelled before it can drain the claim: {intents:?}"
    );
    assert!(
        intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt { .. }
                | Intent::Build { .. }
                | Intent::Repair { .. }
                | Intent::RepairUnits { .. }
                | Intent::Upgrade { .. }
        )),
        "even low-screen desperation cannot consume an unpaid Foundry claim: {intents:?}"
    );

    let legacy = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(
        legacy.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt { .. } | Intent::Repair { .. } | Intent::RepairUnits { .. }
        )),
        "the control must expose spending that only player-facing escrow suppresses: {legacy:?}"
    );
}

#[test]
fn a_fresh_scout_owns_its_harvester_before_construction_lowers() {
    use oxide_sim::bot::Executive;

    let mut obs = obs_with_home();
    obs.scrap = 1_000;
    obs.my_units = (0..4)
        .map(|id| unit_obs(id, 0, UnitKind::Harvester, 3 + id as i32, 5))
        .collect();
    let mut dials = Dials::full();
    dials.deep_tech = false;
    let mut policy = UtilityPolicy::new();

    let legacy = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    let legacy_build = legacy
        .iter()
        .position(|intent| matches!(intent, Intent::Build { .. }))
        .expect("the legacy policy advances its tech tree");
    let legacy_scout = legacy
        .iter()
        .position(|intent| matches!(intent, Intent::Scout { .. }))
        .expect("the legacy policy also scouts");
    assert!(
        legacy_build < legacy_scout,
        "profile-free intent ordering remains frozen for the Overseer: {legacy:?}"
    );

    let intents = player_think(&mut policy, &dials, &obs);
    let player_scout = intents
        .iter()
        .position(|intent| matches!(intent, Intent::Scout { .. }))
        .expect("the player-facing policy scouts");
    let player_build = intents
        .iter()
        .position(|intent| matches!(intent, Intent::Build { .. }))
        .expect("the player-facing policy advances its tech tree");
    assert!(player_scout < player_build);
    let scout = intents
        .iter()
        .find_map(|intent| match intent {
            Intent::Scout { unit, .. } => Some(*unit),
            _ => None,
        })
        .expect("a full harvest line supplies a scout");
    assert!(
        intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Fabricator,
                ..
            }
        )),
        "the same think also advances the tech tree: {intents:?}"
    );

    let commands = Executive::new().apply_with_reservations(PlayerId(0), &obs, &intents, &[]);
    assert!(
        commands.iter().any(|command| matches!(
            &command.command,
            Command::Move {
                units,
                queue: false,
                ..
            } if units == &[scout]
        )),
        "the scout intent must survive lowering: {commands:?}"
    );
    let builder = commands.iter().find_map(|command| match &command.command {
        Command::Build {
            units,
            kind: BuildingKind::Fabricator,
            ..
        } => units.first().copied(),
        _ => None,
    });
    assert_ne!(
        builder,
        Some(scout),
        "construction must deterministically choose a different harvester"
    );
    assert!(
        builder.is_some(),
        "the Fabricator build must also survive lowering"
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
