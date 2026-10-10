use super::*;
use crate::scenario::ScenarioMode;

#[test]
fn command_phase_inspection_is_pure_and_stops_before_the_tick() {
    let state = crate::Scenario::skirmish()
        .build()
        .expect("embedded skirmish builds");
    let before = state.clone();
    let worker = state
        .units()
        .iter()
        .find(|unit| unit.player == crate::PlayerId(0) && unit.kind == crate::UnitKind::Harvester)
        .expect("skirmish authors a worker")
        .id;
    let kind = crate::BuildingKind::Turret;
    let anchor = TilePos::new(10, 4);
    let cost = kind
        .base_stats()
        .construction
        .expect("turret is constructible")
        .cost;
    let command = PlayerCommand {
        player: crate::PlayerId(0),
        command: crate::Command::Build {
            units: vec![worker],
            kind,
            anchor,
            queue: false,
            defer: false,
        },
    };

    state.inspect_command_phase(&[command], |projected| {
        assert_eq!(projected.current_tick(), state.current_tick());
        assert_eq!(
            projected.scrap(crate::PlayerId(0)),
            Some(state.player(crate::PlayerId(0)).scrap - cost)
        );
        assert_eq!(
            projected.place_intent_refusal_replacing(crate::PlayerId(0), kind, anchor, &[]),
            Some(crate::PlaceRefusal::Building),
            "the projected site owns its footprint"
        );
        assert!(matches!(
            projected.unit(worker).expect("worker remains").order,
            crate::Order::Build { .. }
        ));
        let site = projected
            .buildings()
            .iter()
            .find(|b| b.kind == kind && b.anchor == anchor)
            .expect("projected site is inspectable");
        assert!(!site.built());
        assert!(site.queue.is_empty());
        assert!(state.buildings().iter().all(|b| b.id != site.id));
    });
    assert_eq!(state, before, "inspection never mutates its source");
}

#[test]
fn command_phase_inspection_honors_a_frozen_result() {
    let mut state = crate::Scenario::skirmish()
        .build()
        .expect("embedded skirmish builds");
    state.result = Some(GameResult::Draw);
    let before = state.clone();

    state.inspect_command_phase(
        &[PlayerCommand {
            player: crate::PlayerId(0),
            command: crate::Command::Surrender,
        }],
        |projected| {
            assert!(!projected.accepts_commands(crate::PlayerId(0)));
            assert_eq!(
                projected.scrap(crate::PlayerId(0)),
                Some(state.player(crate::PlayerId(0)).scrap)
            );
        },
    );
    assert_eq!(state, before);
}

#[test]
fn rect_ring_has_expected_size_and_order() {
    // 2x2 rect → 12-tile ring.
    let ring: Vec<TilePos> = rect_adjacent_tiles(TilePos::new(5, 5), (2, 2)).collect();
    assert_eq!(ring.len(), 12);
    assert_eq!(
        ring[0],
        TilePos::new(4, 4),
        "row-major: top-left corner first"
    );
    assert!(
        ring.iter()
            .all(|t| tile_adjacent_to_rect(*t, TilePos::new(5, 5), (2, 2)))
    );
}

#[test]
fn footprint_incident_tiles_are_half_turn_equivariant() {
    use chassis::fx::{Fx, Vec2Fx};

    let (map_width, map_height) = (48, 30);
    let rotate_tile = |tile: TilePos| TilePos::new(map_width - 1 - tile.x, map_height - 1 - tile.y);
    let rotate_point = |point: Vec2Fx| {
        Vec2Fx::new(
            Fx::from_num(map_width) - point.x,
            Fx::from_num(map_height) - point.y,
        )
    };
    let cases = [
        (
            "northeast impact",
            TilePos::new(8, 5),
            (2, 2),
            Vec2Fx::new(Fx::from_num(10), Fx::from_num(5)),
            Vec2Fx::new(Fx::ONE, -Fx::ONE),
        ),
        (
            "southwest impact",
            TilePos::new(31, 20),
            (2, 2),
            Vec2Fx::new(Fx::from_num(31), Fx::from_num(22)),
            Vec2Fx::new(-Fx::ONE, Fx::ONE),
        ),
        (
            "wide even footprint",
            TilePos::new(12, 11),
            (4, 2),
            Vec2Fx::new(Fx::from_num(14), Fx::from_num(11)),
            Vec2Fx::new(Fx::ZERO, Fx::ONE),
        ),
    ];

    for (name, anchor, size, impact, approach) in cases {
        let mirrored_anchor = TilePos::new(
            map_width - size.0 - anchor.x,
            map_height - size.1 - anchor.y,
        );
        let tile = footprint_incident_tile(anchor, size, impact, approach);
        let mirrored =
            footprint_incident_tile(mirrored_anchor, size, rotate_point(impact), -approach);
        let inside = |tile: TilePos, anchor: TilePos| {
            tile.x >= anchor.x
                && tile.x < anchor.x + size.0
                && tile.y >= anchor.y
                && tile.y < anchor.y + size.1
        };
        assert!(inside(tile, anchor), "{name}: warning left its footprint");
        assert!(
            inside(mirrored, mirrored_anchor),
            "{name}: mirrored warning left its footprint"
        );
        assert_eq!(rotate_tile(tile), mirrored, "{name}");
    }

    let axis_anchor = TilePos::new(23, 5);
    let axis_impact = Vec2Fx::new(Fx::from_num(24), Fx::from_num(5));
    let axis_tile = footprint_incident_tile(
        axis_anchor,
        (2, 2),
        axis_impact,
        Vec2Fx::new(Fx::ZERO, Fx::ONE),
    );
    assert_eq!(
        axis_tile,
        TilePos::new(24, 5),
        "cross-product tie selects the same attack-local side on the map axis"
    );
    let mirrored_axis_anchor = TilePos::new(23, 23);
    let mirrored_axis_tile = footprint_incident_tile(
        mirrored_axis_anchor,
        (2, 2),
        rotate_point(axis_impact),
        Vec2Fx::new(Fx::ZERO, -Fx::ONE),
    );
    assert_eq!(rotate_tile(axis_tile), mirrored_axis_tile);
}

#[test]
fn a_building_upgraded_on_the_tick_it_falls_reports_the_new_tier() {
    use crate::scenario::{BuildingSpec, UnitSpec};
    use crate::{BuildingKind, Command, Order, PlayerCommand, PlayerId, Target, UnitKind};

    let kind = BuildingKind::ALL
        .into_iter()
        .find(|kind| {
            kind.upgrade_from(0)
                .is_some_and(|up| up.requires == [BuildingKind::Fabricator])
        })
        .expect("some building upgrades once a Fabricator stands");
    let mut scenario = calibration_open_cupric();
    scenario.units = vec![UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 18,
        y: 16,
    }];
    let anchor = TilePos::new(21, 15);
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 25,
            y: 13,
        },
        BuildingSpec {
            player: 1,
            kind,
            x: anchor.x,
            y: anchor.y,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::Fabricator,
            x: 29,
            y: 13,
        },
    ];
    let mut state = scenario.build().expect("the duel builds");
    let victim = state
        .buildings
        .iter()
        .find(|building| building.player == PlayerId(1) && building.anchor == anchor)
        .expect("the victim exists")
        .id;
    state.building_mut(victim).expect("victim").hp = 1;
    state.players[1].scrap = kind.upgrade_from(0).expect("upgradable").cost;
    let shooter = state.units[0].id;
    let direction = state
        .building(victim)
        .unwrap()
        .closest_point_to(state.units[0].pos)
        - state.units[0].pos;
    let unit = state.unit_mut(shooter).unwrap();
    unit.turret_heading = Some(chassis::compass::heading_of(direction));
    unit.order = Order::Attack {
        pursue: false,
        target: Target::Building(victim).into(),
        resume: None,
    };

    // Upgrade on exactly the tick the shot would land.
    let falls = |report: &crate::TickReport| {
        report.events.iter().any(|event| {
            matches!(event, crate::Event::BuildingDestroyed { building, .. } if *building == victim)
        })
    };
    while !falls(&state.clone().tick(&[])) {
        state.tick(&[]);
        assert!(state.current_tick() < 100, "the shot lands");
    }
    let report = state.tick(&[PlayerCommand {
        player: PlayerId(1),
        command: Command::UpgradeBuilding { building: victim },
    }]);
    assert!(
        report.events.iter().any(|event| matches!(
            event,
            crate::Event::BuildingDestroyed { building, tier: 1, .. } if *building == victim
        )),
        "the same-tick upgrade counts: {:?}",
        report.events
    );
}

#[test]
fn mirrored_lethal_hits_record_mirrored_footprint_incidents() {
    use crate::scenario::{BuildingSpec, UnitSpec};
    use crate::{BuildingKind, Order, PlayerId, Target, UnitKind};

    let mut scenario = calibration_open_cupric();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 18,
            y: 16,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 29,
            y: 13,
        },
    ];
    let left_anchor = TilePos::new(25, 13);
    let right_anchor = TilePos::new(21, 15);
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: left_anchor.x,
            y: left_anchor.y,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::Fabricator,
            x: right_anchor.x,
            y: right_anchor.y,
        },
    ];
    let mut state = scenario.build().expect("the mirrored volley builds");
    let victim = |state: &State, player, anchor| {
        state
            .buildings
            .iter()
            .find(|building| building.player == player && building.anchor == anchor)
            .expect("the victim exists")
            .id
    };
    let left_victim = victim(&state, PlayerId(0), left_anchor);
    let right_victim = victim(&state, PlayerId(1), right_anchor);
    let damage = UnitKind::Sentinel.stats().weapons[0].damage;
    state.building_mut(left_victim).expect("left victim").hp = damage;
    state.building_mut(right_victim).expect("right victim").hp = damage;
    for (player, target) in [
        (PlayerId(0), Target::Building(right_victim)),
        (PlayerId(1), Target::Building(left_victim)),
    ] {
        state
            .units
            .iter_mut()
            .find(|unit| unit.player == player)
            .expect("the mirrored shooter exists")
            .order = Order::Attack {
            pursue: false,
            target: target.into(),
            resume: None,
        };
    }

    for (unit_id, victim) in [
        (state.units[0].id, right_victim),
        (state.units[1].id, left_victim),
    ] {
        let unit = state.unit(unit_id).unwrap();
        let direction = state.building(victim).unwrap().closest_point_to(unit.pos) - unit.pos;
        state.unit_mut(unit_id).unwrap().turret_heading =
            Some(chassis::compass::heading_of(direction));
    }
    // Only unseen fire is remembered, and a Sentinel cannot outrange a
    // Fabricator's sight, so each victim's team loses sight of its shooter.
    for (viewer, shooter) in [(0, state.units[1].id), (1, state.units[0].id)] {
        let tile = state.unit(shooter).unwrap().tile();
        state.vision[viewer].conceal(tile);
    }
    let report = state.tick(&[]);
    assert!(
        report.events.iter().any(|event| matches!(
            event,
            crate::Event::BuildingDestroyed { building, .. } if *building == left_victim
        )),
        "the west victim dies in the mirrored volley"
    );
    assert!(
        report.events.iter().any(|event| matches!(
            event,
            crate::Event::BuildingDestroyed { building, .. } if *building == right_victim
        )),
        "the east victim dies in the mirrored volley"
    );
    let left = state.vision(PlayerId(0)).salvage_incidents();
    let right = state.vision(PlayerId(1)).salvage_incidents();
    assert_eq!(left.len(), 1);
    assert_eq!(right.len(), 1);
    let inside = |tile: TilePos, anchor: TilePos| {
        tile.x >= anchor.x && tile.x < anchor.x + 2 && tile.y >= anchor.y && tile.y < anchor.y + 2
    };
    assert!(inside(left[0].tile, left_anchor));
    assert!(inside(right[0].tile, right_anchor));
    assert_eq!(mirror_tile(&state, left[0].tile), right[0].tile);
    assert_eq!(left[0].expires_at, right[0].expires_at);
}

#[test]
fn adjacency_excludes_inside_and_far() {
    let anchor = TilePos::new(3, 3);
    assert!(!tile_adjacent_to_rect(TilePos::new(3, 3), anchor, (2, 2)));
    assert!(!tile_adjacent_to_rect(TilePos::new(4, 4), anchor, (2, 2)));
    assert!(tile_adjacent_to_rect(TilePos::new(2, 2), anchor, (2, 2)));
    assert!(tile_adjacent_to_rect(TilePos::new(5, 4), anchor, (2, 2)));
    assert!(!tile_adjacent_to_rect(TilePos::new(6, 4), anchor, (2, 2)));
}

fn calibration_open_cupric() -> crate::Scenario {
    use crate::scenario::{PlayerSpec, UnitSpec};
    use crate::{Faction, UnitKind};

    crate::Scenario {
        mode: ScenarioMode::Match,
        name: "Calibration Open - Cupric".into(),
        map: [
            "################################################",
            "#..............................................#",
            "#..............................................#",
            "#.......ss.....................................#",
            "#..............................................#",
            "#....1....E....##..............................#",
            "#..............................................#",
            "#..............................................#",
            "#...................#..........................#",
            "#...........s.......#..........................#",
            "#.............s................................#",
            "#................E.............................#",
            "#..............................................#",
            "#..................S...........................#",
            "#..............................................#",
            "#..............................................#",
            "#...........................S..................#",
            "#............................E.................#",
            "#..............................................#",
            "#................................s.............#",
            "#..........................#.......s...........#",
            "#..........................#...................#",
            "#..............................................#",
            "#...................................E....2.....#",
            "#..............................##..............#",
            "#..............................................#",
            "#.....................................ss.......#",
            "#..............................................#",
            "#..............................................#",
            "################################################",
        ]
        .map(str::to_owned)
        .into(),
        players: ["West Cupric", "East Cupric"]
            .map(|name| PlayerSpec {
                name: name.into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 150,
                bot: false,
                bot_config: None,
            })
            .into(),
        units: [
            (0, UnitKind::Harvester, 6, 8),
            (0, UnitKind::Harvester, 7, 8),
            (0, UnitKind::Harvester, 8, 7),
            (0, UnitKind::Sentinel, 10, 8),
            (1, UnitKind::Harvester, 41, 21),
            (1, UnitKind::Harvester, 40, 21),
            (1, UnitKind::Harvester, 39, 22),
            (1, UnitKind::Sentinel, 37, 21),
        ]
        .map(|(player, kind, x, y)| UnitSpec { player, kind, x, y })
        .into(),
        buildings: Vec::new(),
        meta: None,
    }
}

fn calibration_open_tick_zero_commands() -> Vec<PlayerCommand> {
    use crate::{BuildingId, Command, PlayerId, UnitId, UnitKind};

    let mut commands = Vec::new();
    for (player, units, node) in [
        (0, [0, 1, 2], TilePos::new(8, 3)),
        (1, [4, 5, 6], TilePos::new(39, 26)),
    ] {
        commands.extend(units.map(|unit| PlayerCommand {
            player: PlayerId(player),
            command: Command::Harvest {
                units: vec![UnitId(unit)],
                node,
                queue: false,
            },
        }));
        commands.push(PlayerCommand {
            player: PlayerId(player),
            command: Command::Train {
                building: BuildingId(u32::from(player)),
                kind: UnitKind::Harvester,
            },
        });
        commands.push(PlayerCommand {
            player: PlayerId(player),
            command: Command::Hunt {
                units: vec![UnitId(if player == 0 { 3 } else { 7 })],
                goal: if player == 0 {
                    TilePos::new(8, 8)
                } else {
                    TilePos::new(39, 21)
                },
                queue: false,
            },
        });
    }
    commands
}

fn mirror_tile(state: &State, tile: TilePos) -> TilePos {
    TilePos::new(
        state.map.width() - 1 - tile.x,
        state.map.height() - 1 - tile.y,
    )
}

/// Mirrored goals name mirrored clicked tiles, slots and endpoints. A
/// pending pair shares its rank and scans in opposite frames, which is
/// what makes the slots it later takes mirror.
fn assert_goals_mirror(stage: &str, state: &State, left: crate::Goal, right: crate::Goal) {
    use crate::Aim;

    assert_eq!(
        mirror_tile(state, left.tile()),
        right.tile(),
        "{stage}: clicked tile"
    );
    match (left.aim, right.aim) {
        (Aim::Tile, Aim::Tile) => {}
        (
            Aim::Pending {
                rank: left_rank,
                reverse: left_reverse,
            },
            Aim::Pending {
                rank: right_rank,
                reverse: right_reverse,
            },
        ) => {
            assert_eq!(left_rank, right_rank, "{stage}: pending rank");
            assert_ne!(left_reverse, right_reverse, "{stage}: pending frame");
        }
        (Aim::Slot(left_slot), Aim::Slot(right_slot)) => {
            assert_eq!(mirror_tile(state, left_slot), right_slot, "{stage}: slot");
        }
        aims => panic!("{stage}: aims are not paired: {aims:?}"),
    }
    assert_eq!(
        left.endpoint.map(|tile| mirror_tile(state, tile)),
        right.endpoint,
        "{stage}: endpoint"
    );
}

fn assert_calibration_open_symmetry(
    stage: &str,
    state: &State,
    unit_pairs: &[(crate::UnitId, crate::UnitId)],
) {
    use crate::Order;
    use chassis::fx::{Fx, Vec2Fx};

    for y in 0..state.map.height() {
        for x in 0..state.map.width() {
            let tile = TilePos::new(x, y);
            assert_eq!(
                state.map.tile(tile),
                state.map.tile(mirror_tile(state, tile)),
                "{stage}: map tile {tile:?}"
            );
        }
    }
    assert_eq!(
        state.players[0].scrap, state.players[1].scrap,
        "{stage}: scrap"
    );
    for &(left_id, right_id) in unit_pairs {
        let left = state.unit(left_id).expect("left unit exists");
        let right = state.unit(right_id).expect("right unit exists");
        assert_eq!(left.player, crate::PlayerId(0), "{stage}: left owner");
        assert_eq!(right.player, crate::PlayerId(1), "{stage}: right owner");
        assert_eq!(left.kind, right.kind, "{stage}: unit kind {left_id}");
        assert_eq!(left.hp, right.hp, "{stage}: unit hp {left_id}");
        assert_eq!(left.carrying, right.carrying, "{stage}: cargo {left_id}");
        assert_eq!(left.progress, right.progress, "{stage}: progress {left_id}");
        let mirrored_pos = Vec2Fx::new(
            Fx::from_num(state.map.width()) - left.pos.x,
            Fx::from_num(state.map.height()) - left.pos.y,
        );
        assert_eq!(mirrored_pos, right.pos, "{stage}: unit position {left_id}");

        match (left.order, right.order) {
            (
                Order::Harvest {
                    node: left_node,
                    anchor: left_anchor,
                    retiring: left_retiring,
                },
                Order::Harvest {
                    node: right_node,
                    anchor: right_anchor,
                    retiring: right_retiring,
                },
            ) => {
                assert_eq!(mirror_tile(state, left_node), right_node, "{stage}: node");
                assert_eq!(
                    mirror_tile(state, left_anchor),
                    right_anchor,
                    "{stage}: anchor"
                );
                assert_eq!(left_retiring, right_retiring, "{stage}: retirement");
            }
            (Order::Hunt { goal: left_goal }, Order::Hunt { goal: right_goal })
            | (Order::Run { goal: left_goal }, Order::Run { goal: right_goal }) => {
                assert_goals_mirror(stage, state, left_goal, right_goal);
            }
            (
                Order::Attack {
                    target: left_target,
                    pursue: left_pursue,
                    resume: left_resume,
                },
                Order::Attack {
                    target: right_target,
                    pursue: right_pursue,
                    resume: right_resume,
                },
            ) => {
                assert_eq!(
                    std::mem::discriminant(&left_target),
                    std::mem::discriminant(&right_target),
                    "{stage}: attack target kind"
                );
                assert_eq!(left_pursue, right_pursue, "{stage}: pursuit");
                match (left_resume, right_resume) {
                    (None, None) => {}
                    (Some(left_goal), Some(right_goal)) => {
                        assert_goals_mirror(stage, state, left_goal, right_goal);
                    }
                    resumes => panic!("{stage}: resumes are not paired: {resumes:?}"),
                }
            }
            (Order::Idle, Order::Idle) => {}
            orders => panic!("{stage}: orders are not paired: {orders:?}"),
        }

        match (&left.path, &right.path) {
            (None, None) => {}
            (Some(left_path), Some(right_path)) => {
                assert_eq!(
                    mirror_tile(state, left_path.goal),
                    right_path.goal,
                    "{stage}: path goal {left_id}"
                );
                let mirrored: Vec<_> = left_path
                    .waypoints
                    .iter()
                    .map(|tile| mirror_tile(state, *tile))
                    .collect();
                assert_eq!(mirrored, right_path.waypoints, "{stage}: path {left_id}");
                assert_eq!(
                    left_path.next, right_path.next,
                    "{stage}: path cursor {left_id}"
                );
            }
            paths => panic!("{stage}: only one paired unit has a path: {paths:?}"),
        }
    }
    {
        let (left, right) = (0, 1);
        let left = &state.buildings[left];
        let right = &state.buildings[right];
        let (width, height) = left.kind.size();
        assert_eq!(left.kind, right.kind, "{stage}: building kind");
        assert_eq!(left.hp, right.hp, "{stage}: building hp");
        assert_eq!(left.queue, right.queue, "{stage}: production queue");
        assert_eq!(left.phase, right.phase, "{stage}: production progress");
        assert_eq!(
            TilePos::new(
                state.map.width() - width - left.anchor.x,
                state.map.height() - height - left.anchor.y,
            ),
            right.anchor,
            "{stage}: building anchor"
        );
    }
}

fn pair_new_calibration_units(state: &State, unit_pairs: &mut Vec<(crate::UnitId, crate::UnitId)>) {
    let already_paired = |id| {
        unit_pairs
            .iter()
            .any(|&(left, right)| left == id || right == id)
    };
    let unmatched = |player| {
        state
            .units
            .iter()
            .filter(|unit| unit.player == player && !already_paired(unit.id))
            .map(|unit| unit.id)
            .collect::<Vec<_>>()
    };
    let left = unmatched(crate::PlayerId(0));
    let right = unmatched(crate::PlayerId(1));
    assert_eq!(
        left.len(),
        right.len(),
        "a production phase spawned for only one mirrored seat: {left:?} vs {right:?}"
    );
    for pair in left.into_iter().zip(right) {
        unit_pairs.push(pair);
    }
}

fn run_calibration_open_tick(
    state: &mut State,
    commands_for_tick: &[PlayerCommand],
    unit_pairs: &mut Vec<(crate::UnitId, crate::UnitId)>,
) {
    let tick = state.tick;
    let mut events = Vec::new();
    let mut index = spatial::UnitIndex::new();
    let stage = |phase| format!("tick {tick} {phase}");

    production::capture_recovery_entitlements(state);
    commands::apply(state, commands_for_tick, &mut events);
    assert_calibration_open_symmetry(&stage("commands"), state, unit_pairs);
    production::run(state, &mut events);
    pair_new_calibration_units(state, unit_pairs);
    assert_calibration_open_symmetry(&stage("production"), state, unit_pairs);
    charges::cancel_discovered(state, &mut events);
    production::decay_abandoned_sites(state);
    let (pending, salvaged) = brain::run(state, &mut index, &mut events);
    assert_calibration_open_symmetry(&stage("brains"), state, unit_pairs);
    brain::logistics::resolve(state, pending, &mut events);
    assert_calibration_open_symmetry(&stage("logistics"), state, unit_pairs);
    movement::evict_claimed_ground(state);
    let air_positions = aircraft_crashes::capture_positions(state);
    let (travel, _) = movement::run(state);
    assert_calibration_open_symmetry(&stage("movement"), state, unit_pairs);
    movement::resolve_collisions(state, &travel, &mut index);
    assert_calibration_open_symmetry(&stage("collisions"), state, unit_pairs);
    aircraft_crashes::remember_motion(state, &air_positions);
    aircraft_crashes::land(state, &mut events);
    charges::detonate_under_units(state, &mut events);
    cleanup(state, &salvaged, &mut events);
    if state.tick.is_multiple_of(crate::stats::WRECK_DECAY_TICKS) {
        state.map.decay_wrecks();
    }
    state.refresh_vision();
    if charges::cancel_discovered(state, &mut events) {
        state.reconcile_attack_knowledge();
    }
    goals::expose(state);
    assert_calibration_open_symmetry(&stage("exposure"), state, unit_pairs);
    victory(state, &mut events);
    assert_calibration_open_symmetry(&stage("cleanup"), state, unit_pairs);
    state.tick += 1;
}

#[test]
fn fixed_facing_opening_replays_identically_through_harvest_cycles() {
    use crate::{Command, PlayerId, UnitId};

    let mut state = calibration_open_cupric()
        .build()
        .expect("the calibration scenario builds");
    let commands = calibration_open_tick_zero_commands();
    let mut replay = state.clone();
    assert_eq!(state.tick(&commands), replay.tick(&commands));
    for _ in 1..=600 {
        let commands = if state.tick == 102 {
            vec![
                PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Harvest {
                        units: vec![UnitId(8)],
                        node: TilePos::new(8, 3),
                        queue: false,
                    },
                },
                PlayerCommand {
                    player: PlayerId(1),
                    command: Command::Harvest {
                        units: vec![UnitId(9)],
                        node: TilePos::new(39, 26),
                        queue: false,
                    },
                },
            ]
        } else {
            Vec::new()
        };
        assert_eq!(state.tick(&commands), replay.tick(&commands));
        assert_eq!(state.hash(), replay.hash());
        state.validate_invariants().unwrap();
    }
}

#[test]
fn mirrored_haulers_replan_together_when_construction_closes_their_routes() {
    use crate::scenario::UnitSpec;
    use crate::state::PathFollow;
    use crate::{BuildingKind, Command, Order, PlayerId, UnitId, UnitKind};
    use chassis::fx::{Fx, Vec2Fx};

    let mut scenario = calibration_open_cupric();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 14,
            y: 7,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 33,
            y: 22,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 8,
            y: 8,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 39,
            y: 21,
        },
    ];
    let mut state = scenario.build().expect("the mirrored scenario builds");
    let left_waypoints = [(13, 7), (12, 7), (11, 7), (10, 7), (9, 7), (8, 6), (7, 6)]
        .map(|(x, y)| TilePos::new(x, y));
    let right_waypoints = left_waypoints.map(|tile| mirror_tile(&state, tile));
    let left_goal = *left_waypoints.last().expect("the route has a goal");
    let right_goal = *right_waypoints.last().expect("the route has a goal");
    for (player, goal) in [(PlayerId(0), left_goal), (PlayerId(1), right_goal)] {
        let foundry = state
            .buildings
            .iter()
            .find(|building| building.player == player && building.kind == BuildingKind::Foundry)
            .expect("each side has a foundry");
        assert!(
            tile_adjacent_to_rect(goal, foundry.anchor, foundry.kind.size()),
            "{goal:?} must be a doorstep around {:?}",
            foundry.anchor
        );
    }
    for (id, node, anchor, goal, waypoints) in [
        (
            UnitId(0),
            TilePos::new(12, 9),
            TilePos::new(12, 9),
            left_goal,
            left_waypoints.to_vec(),
        ),
        (
            UnitId(1),
            TilePos::new(35, 20),
            TilePos::new(35, 20),
            right_goal,
            right_waypoints.to_vec(),
        ),
    ] {
        let unit = state.unit_mut(id).expect("the hauler exists");
        unit.carrying = 10;
        unit.order = Order::Harvest {
            node,
            anchor,
            retiring: false,
        };
        unit.path = Some(PathFollow {
            final_point: None,
            goal,
            waypoints,
            next: 0,
        });
    }

    let report = state.tick(&[
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Build {
                units: vec![UnitId(2)],
                kind: BuildingKind::Turret,
                anchor: TilePos::new(9, 7),
                queue: false,
                defer: false,
            },
        },
        PlayerCommand {
            player: PlayerId(1),
            command: Command::Build {
                units: vec![UnitId(3)],
                kind: BuildingKind::Turret,
                anchor: TilePos::new(38, 22),
                queue: false,
                defer: false,
            },
        },
    ]);
    assert!(
        report
            .events
            .iter()
            .all(|event| !matches!(event, crate::Event::CommandRejected { .. })),
        "the mirrored build commands must both land: {:?}",
        report.events
    );

    let left = state.unit(UnitId(0)).expect("the left hauler remains");
    let right = state.unit(UnitId(1)).expect("the right hauler remains");
    assert_eq!(
        Vec2Fx::new(
            Fx::from_num(state.map.width()) - left.pos.x,
            Fx::from_num(state.map.height()) - left.pos.y,
        ),
        right.pos,
        "equivalent route closures must not stagger by global unit id"
    );
    let left_path = left.path.as_ref().expect("the left hauler replans");
    let right_path = right.path.as_ref().expect("the right hauler replans");
    assert!(
        !left_path.waypoints.contains(&TilePos::new(9, 7)),
        "the left route must clear the new footprint: {left_path:?}"
    );
    assert!(
        !right_path.waypoints.contains(&TilePos::new(38, 22)),
        "the right route must clear the new footprint: {right_path:?}"
    );
    assert_eq!(left_path.next, right_path.next);
    assert_eq!(mirror_tile(&state, left_path.goal), right_path.goal);
    assert_eq!(
        left_path
            .waypoints
            .iter()
            .map(|tile| mirror_tile(&state, *tile))
            .collect::<Vec<_>>(),
        right_path.waypoints
    );
}

#[test]
fn centered_builders_leave_new_footprints_through_legal_doorsteps() {
    use crate::scenario::{BuildingSpec, UnitSpec};
    use crate::{BuildingKind, Command, PlayerId, UnitId, UnitKind};
    use chassis::fx::{Fx, Vec2Fx};

    let left_anchor = TilePos::new(8, 7);
    let right_anchor = TilePos::new(39, 22);
    let mut scenario = calibration_open_cupric();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: left_anchor.x,
            y: left_anchor.y,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: right_anchor.x,
            y: right_anchor.y,
        },
    ];
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 10,
            y: 12,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::Fabricator,
            x: 36,
            y: 16,
        },
    ];
    let mut state = scenario
        .build()
        .expect("the centered mirrored builder scenario builds");

    let report = state.tick(&[
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Build {
                units: vec![UnitId(0)],
                kind: BuildingKind::ScuttleCharge,
                anchor: left_anchor,
                queue: false,
                defer: false,
            },
        },
        PlayerCommand {
            player: PlayerId(1),
            command: Command::Build {
                units: vec![UnitId(1)],
                kind: BuildingKind::ScuttleCharge,
                anchor: right_anchor,
                queue: false,
                defer: false,
            },
        },
    ]);

    assert!(
        report
            .events
            .iter()
            .all(|event| !matches!(event, crate::Event::CommandRejected { .. })),
        "the mirrored build commands must both land: {:?}",
        report.events
    );
    let left = state.unit(UnitId(0)).expect("the west builder remains");
    let right = state.unit(UnitId(1)).expect("the east builder remains");
    assert_eq!(
        right.pos,
        Vec2Fx::new(
            Fx::from_num(state.map.width()) - left.pos.x,
            Fx::from_num(state.map.height()) - left.pos.y,
        ),
        "builders centered on new sites must take exact half-turn steps"
    );
    let left_path = left.path.as_ref().expect("the west builder routes out");
    let right_path = right.path.as_ref().expect("the east builder routes out");
    assert!(tile_adjacent_to_rect(
        left_path.goal,
        left_anchor,
        BuildingKind::ScuttleCharge.size(),
    ));
    assert!(tile_adjacent_to_rect(
        right_path.goal,
        right_anchor,
        BuildingKind::ScuttleCharge.size(),
    ));
    for path in [left_path, right_path] {
        assert!(path.final_point.is_some());
        assert!(!path.waypoints.is_empty());
    }
    state.validate_invariants().unwrap();
}

#[test]
fn mirrored_six_unit_hunt_spreads_in_each_armys_local_frame() {
    use crate::scenario::{BuildingSpec, UnitSpec};
    use crate::{BuildingKind, Command, PlayerId, UnitId, UnitKind};

    let mut scenario = calibration_open_cupric();
    scenario.units.extend(
        [
            (0, 9, 9),
            (1, 38, 20),
            (0, 8, 9),
            (1, 39, 20),
            (0, 9, 7),
            (1, 38, 22),
            (0, 7, 9),
            (1, 40, 20),
            (0, 7, 7),
            (1, 40, 22),
        ]
        .map(|(player, x, y)| UnitSpec {
            player,
            kind: UnitKind::Sentinel,
            x,
            y,
        }),
    );
    scenario.buildings.extend([
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Barricade,
            x: 21,
            y: 15,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::Barricade,
            x: 26,
            y: 14,
        },
    ]);
    let mut state = scenario.build().expect("the mirrored scenario builds");
    let mut unit_pairs = Vec::from(
        [
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
            (8, 9),
            (10, 11),
            (12, 13),
            (14, 15),
            (16, 17),
        ]
        .map(|(left, right)| (UnitId(left), UnitId(right))),
    );
    let commands = vec![
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Hunt {
                units: [3, 8, 10, 12, 14, 16].map(UnitId).into(),
                goal: TilePos::new(21, 15),
                queue: false,
            },
        },
        PlayerCommand {
            player: PlayerId(1),
            command: Command::Hunt {
                units: [7, 9, 11, 13, 15, 17].map(UnitId).into(),
                goal: TilePos::new(26, 14),
                queue: false,
            },
        },
    ];

    assert_calibration_open_symmetry("before group order", &state, &unit_pairs);
    run_calibration_open_tick(&mut state, &commands, &mut unit_pairs);
}

#[test]
fn mirrored_groups_sent_beyond_sight_take_mirrored_slots_on_exposure() {
    use crate::scenario::UnitSpec;
    use crate::{Aim, Command, Order, PlayerId, UnitId, UnitKind};

    let mut scenario = calibration_open_cupric();
    scenario.units.extend(
        [
            (0, 9, 9),
            (1, 38, 20),
            (0, 8, 9),
            (1, 39, 20),
            (0, 9, 7),
            (1, 38, 22),
            (0, 7, 9),
            (1, 40, 20),
            (0, 7, 7),
            (1, 40, 22),
        ]
        .map(|(player, x, y)| UnitSpec {
            player,
            kind: UnitKind::Sentinel,
            x,
            y,
        }),
    );
    let mut state = scenario.build().expect("the mirrored scenario builds");
    let mut unit_pairs = Vec::from(
        [
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
            (8, 9),
            (10, 11),
            (12, 13),
            (14, 15),
            (16, 17),
        ]
        .map(|(left, right)| (UnitId(left), UnitId(right))),
    );
    // A rock tile just beyond the western group's sight, far from every
    // enemy, and its mirror: the click snaps and spreads only once seen.
    let clicked = TilePos::new(20, 8);
    assert!(!state.passable(clicked), "premise: the click lands on rock");
    assert!(!state.vision(PlayerId(0)).explored(clicked));
    assert!(
        !state
            .vision(PlayerId(1))
            .explored(mirror_tile(&state, clicked))
    );
    let groups = [
        [3, 8, 10, 12, 14, 16].map(UnitId),
        [7, 9, 11, 13, 15, 17].map(UnitId),
    ];
    let commands = vec![
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Hunt {
                units: groups[0].into(),
                goal: clicked,
                queue: false,
            },
        },
        PlayerCommand {
            player: PlayerId(1),
            command: Command::Hunt {
                units: groups[1].into(),
                goal: mirror_tile(&state, clicked),
                queue: false,
            },
        },
    ];
    let goal = |state: &State, id| match state.unit(id).expect("sentinel lives").order {
        Order::Hunt { goal } => Some(goal),
        Order::Idle => None,
        other => panic!("unit {id} left its march: {other:?}"),
    };

    run_calibration_open_tick(&mut state, &commands, &mut unit_pairs);
    for (rank, id) in groups[0].into_iter().enumerate() {
        let goal = goal(&state, id).expect("marching");
        assert_eq!(goal.target(), clicked, "every member heads for the click");
        assert!(matches!(goal.aim, Aim::Pending { rank: r, .. } if usize::from(r) == rank));
    }

    let mut exposed = None;
    for _ in 0..400 {
        run_calibration_open_tick(&mut state, &[], &mut unit_pairs);
        let goals: Vec<_> = groups[0].iter().map(|&id| goal(&state, id)).collect();
        if exposed.is_none() && goals.iter().flatten().all(|goal| !goal.is_pending()) {
            exposed = Some(goals);
        }
        if groups
            .iter()
            .flatten()
            .all(|&id| goal(&state, id).is_none())
        {
            break;
        }
    }
    let exposed = exposed.expect("the click was explored on the way");
    let mut targets: Vec<_> = exposed
        .iter()
        .map(|goal| goal.expect("still marching when exposed").target())
        .collect();
    assert!(
        targets.iter().all(|&target| target != clicked),
        "rock is no slot"
    );
    targets.sort_unstable_by_key(|tile| (tile.y, tile.x));
    targets.dedup();
    assert_eq!(targets.len(), 6, "the members spread over their own slots");
    assert!(
        groups
            .iter()
            .flatten()
            .all(|&id| goal(&state, id).is_none()),
        "both groups arrived"
    );
}

/// An open map whose half-turn maps each seat's Foundry onto the other's.
fn mirrored_field(width: i32, height: i32) -> Vec<String> {
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| match (x, y) {
                    (1, 1) => '1',
                    _ if (x, y) == (width - 3, height - 3) => '2',
                    _ if x == 0 || y == 0 || x == width - 1 || y == height - 1 => '#',
                    _ => '.',
                })
                .collect()
        })
        .collect()
}

#[test]
fn mirrored_avalanches_back_out_to_mirrored_stands() {
    use crate::scenario::UnitSpec;
    use crate::{Command, PlayerId, Target, UnitKind};

    let mut scenario = calibration_open_cupric();
    scenario.map = mirrored_field(24, 12);
    // Rock on the straight line back leaves two equally near stands.
    for (x, y) in [(3, 5), (20, 6)] {
        scenario.map[y].replace_range(x..=x, "#");
    }
    scenario.buildings.clear();
    scenario.units = [
        (0, UnitKind::Avalanche, 6, 5),
        (1, UnitKind::Harvester, 7, 5),
        (1, UnitKind::Avalanche, 17, 6),
        (0, UnitKind::Harvester, 16, 6),
    ]
    .map(|(player, kind, x, y)| UnitSpec { player, kind, x, y })
    .into();
    let mut state = scenario.build().expect("the mirrored scenario builds");
    let ids: Vec<_> = state.units().iter().map(|unit| unit.id).collect();
    let attack = |player: u8, unit: usize, victim: usize| PlayerCommand {
        player: PlayerId(player),
        command: Command::Attack {
            units: vec![ids[unit]],
            target: Target::Unit(ids[victim]).into(),
            queue: false,
        },
    };
    state.tick(&[attack(0, 0, 1), attack(1, 2, 3)]);
    let stand = |unit: usize| {
        state
            .unit(ids[unit])
            .and_then(|unit| unit.path.as_ref())
            .expect("the Avalanche routes out of its dead zone")
            .goal
    };
    let west = stand(0);
    assert!(
        west.center().dist(TilePos::new(7, 5).center())
            >= crate::stats::UnitKind::Avalanche.stats().weapons[0].minimum_range
    );
    assert_eq!(stand(2), mirror_tile(&state, west));
}

#[test]
fn mirrored_sappers_press_mirrored_doorsteps() {
    use crate::scenario::{BuildingSpec, UnitSpec};
    use crate::{BuildingKind, Command, PlayerId, Target, UnitKind};

    let mut scenario = calibration_open_cupric();
    scenario.map = mirrored_field(24, 12);
    // Rock on the nearest doorstep leaves two equally near ones.
    for (x, y) in [(8, 5), (15, 6)] {
        scenario.map[y].replace_range(x..=x, "#");
    }
    scenario.units = [(0, 5, 5), (1, 18, 6)]
        .map(|(player, x, y)| UnitSpec {
            player,
            kind: UnitKind::Sapper,
            x,
            y,
        })
        .into();
    scenario.buildings = [(1, 9, 5), (0, 14, 6)]
        .map(|(player, x, y)| BuildingSpec {
            player,
            kind: BuildingKind::Barricade,
            x,
            y,
        })
        .into();
    let mut state = scenario.build().expect("the mirrored scenario builds");
    let sappers: Vec<_> = state.units().iter().map(|unit| unit.id).collect();
    let walls: Vec<_> = state
        .buildings()
        .iter()
        .filter(|building| building.kind == BuildingKind::Barricade)
        .map(|building| building.id)
        .collect();
    let attack = |player: u8| PlayerCommand {
        player: PlayerId(player),
        command: Command::Attack {
            units: vec![sappers[usize::from(player)]],
            target: Target::Building(walls[usize::from(player)]).into(),
            queue: false,
        },
    };
    let doorstep = |state: &State, player: usize| {
        state
            .unit(sappers[player])
            .and_then(|unit| unit.path.as_ref())
            .expect("the Sapper routes to a doorstep")
            .goal
    };
    state.tick(&[attack(0), attack(1)]);
    let west = doorstep(&state, 0);
    assert!(tile_adjacent_to_rect(west, TilePos::new(9, 5), (1, 1)));
    assert_eq!(doorstep(&state, 1), mirror_tile(&state, west));
}
