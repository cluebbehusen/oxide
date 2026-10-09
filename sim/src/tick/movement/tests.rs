use super::super::spatial::UnitIndex;
use super::*;
use crate::scenario::{PlayerSpec, Scenario, ScenarioMode, UnitSpec};
use crate::state::Faction;
use crate::stats::UnitKind;
use chassis::grid::as_index;

/// A resting row of bodies has no pair to correct and is dropped; a
/// touching neighbor of a pressing pair stays, since the pair's push can
/// reach it.
#[test]
fn only_groups_with_a_pressing_pair_are_kept() {
    let mut state = Scenario::skirmish().build().expect("skirmish builds");
    state.units.clear();
    let spacing = UnitKind::Harvester.stats().radius * 2;
    let step = |from: Vec2Fx, gap: &str| from + Vec2Fx::new(spacing + Fx::lit(gap), Fx::ZERO);
    let mut resting = TilePos::new(6, 5).center();
    for _ in 0..4 {
        state.spawn_unit(crate::PlayerId(0), UnitKind::Harvester, resting);
        resting = step(resting, "0.1");
    }
    let first = TilePos::new(6, 12).center();
    let second = step(first, "-0.1");
    let third = step(second, "0.1");
    let chain = [first, second, third]
        .map(|pos| state.spawn_unit(crate::PlayerId(0), UnitKind::Harvester, pos));
    let slot = |id| state.units.iter().position(|u| u.id == id).unwrap();
    let ranks = owner_local_ranks(&state);
    let pairs =
        collision_pairs(&state, false, &mut UnitIndex::new(), &ranks).expect("the chain presses");
    let mut kept: Vec<_> = pairs.into_iter().collect();
    kept.sort_unstable();
    let mut expected = vec![
        (slot(chain[0]), slot(chain[1])),
        (slot(chain[1]), slot(chain[2])),
    ];
    expected.sort_unstable();
    assert_eq!(kept, expected);
}

#[test]
fn contact_that_cancels_progress_drops_the_route_after_the_stall_bound() {
    for precise in [false, true] {
        let mut state = Scenario::skirmish().build().unwrap();
        let waypoint = TilePos::new(20, 12);
        let slot = 0;
        state.units[slot].path = Some(PathFollow {
            final_point: precise.then(|| Vec2Fx::new(Fx::lit("20.8"), Fx::lit("12.5"))),
            goal: waypoint,
            waypoints: vec![waypoint],
            next: 0,
        });
        state.units[slot].pos = if precise {
            Vec2Fx::new(Fx::lit("20.7"), Fx::lit("12.5"))
        } else {
            TilePos::new(10, 12).center()
        };
        let travel: Vec<Vec2Fx> = (0..state.units.len())
            .map(|i| {
                if i == slot {
                    Vec2Fx::new(Fx::lit("0.1"), Fx::ZERO)
                } else {
                    Vec2Fx::ZERO
                }
            })
            .collect();
        // Propulsion carried the body east; contact shoved it back to where
        // it started, cancelling the whole step.
        let refused = vec![false; state.units.len()];
        let driven: Vec<Vec2Fx> = state
            .units
            .iter()
            .zip(&travel)
            .map(|(unit, &step)| unit.pos + step)
            .collect();
        for tick in 1..crate::stats::STALL_REPLAN_TICKS {
            note_stalls(&mut state, &travel, &refused, &driven);
            assert_eq!(state.units[slot].stall_ticks, tick);
            assert!(state.units[slot].path.is_some());
        }
        // One tick of real progress clears the count.
        state.units[slot].pos += travel[slot];
        note_stalls(&mut state, &travel, &refused, &driven);
        assert_eq!(state.units[slot].stall_ticks, 0);
        assert!(state.units[slot].path.is_some());
        state.units[slot].pos -= travel[slot];
        for _ in 1..crate::stats::STALL_REPLAN_TICKS {
            note_stalls(&mut state, &travel, &refused, &driven);
        }
        assert!(state.units[slot].path.is_some());
        note_stalls(&mut state, &travel, &refused, &driven);
        assert!(
            state.units[slot].path.is_none(),
            "the stalled route was kept"
        );
        assert_eq!(state.units[slot].stall_ticks, 0);
    }
}

/// A refused step moves the body nowhere, so no deflection measures it;
/// it still counts toward the bound, while a tick spent pivoting in
/// place does not.
#[test]
fn refused_steps_drop_the_route_after_the_stall_bound() {
    let mut state = Scenario::skirmish().build().unwrap();
    let waypoint = TilePos::new(20, 12);
    let slot = 0;
    state.units[slot].path = Some(PathFollow {
        final_point: None,
        goal: waypoint,
        waypoints: vec![waypoint],
        next: 0,
    });
    let travel = vec![Vec2Fx::ZERO; state.units.len()];
    let driven: Vec<Vec2Fx> = state.units.iter().map(|unit| unit.pos).collect();
    let pivoting = vec![false; state.units.len()];
    let mut refused = pivoting.clone();
    refused[slot] = true;
    note_stalls(&mut state, &travel, &pivoting, &driven);
    assert_eq!(state.units[slot].stall_ticks, 0);
    for tick in 1..crate::stats::STALL_REPLAN_TICKS {
        note_stalls(&mut state, &travel, &refused, &driven);
        assert_eq!(state.units[slot].stall_ticks, tick);
    }
    note_stalls(&mut state, &travel, &pivoting, &driven);
    assert_eq!(state.units[slot].stall_ticks, 0);
    for _ in 1..crate::stats::STALL_REPLAN_TICKS {
        note_stalls(&mut state, &travel, &refused, &driven);
    }
    assert!(state.units[slot].path.is_some());
    note_stalls(&mut state, &travel, &refused, &driven);
    assert!(
        state.units[slot].path.is_none(),
        "the refused route was kept"
    );
    assert_eq!(state.units[slot].stall_ticks, 0);
}

#[test]
fn a_route_dropped_after_stalls_were_noted_takes_its_count_with_it() {
    let mut state = Scenario::skirmish().build().unwrap();
    let waypoint = TilePos::new(20, 12);
    let slot = 0;
    state.units[slot].path = Some(PathFollow {
        final_point: None,
        goal: waypoint,
        waypoints: vec![waypoint],
        next: 0,
    });
    let mut travel = vec![Vec2Fx::ZERO; state.units.len()];
    travel[slot] = Vec2Fx::new(Fx::lit("0.1"), Fx::ZERO);
    let refused = vec![false; state.units.len()];
    let driven: Vec<Vec2Fx> = state
        .units
        .iter()
        .zip(&travel)
        .map(|(unit, &step)| unit.pos + step)
        .collect();
    note_stalls(&mut state, &travel, &refused, &driven);
    assert_eq!(state.units[slot].stall_ticks, 1);
    state
        .validate_invariants()
        .expect("a stalled walker is valid");

    // A late phase, such as revealing the site a builder is walking to,
    // retargets the unit after the stalls were noted.
    state.units[slot].path = None;
    assert!(matches!(
        state.validate_invariants(),
        Err(crate::state::StateIntegrityError::InvalidStallTicks(_))
    ));
    forget_stalls_without_routes(&mut state);
    assert_eq!(state.units[slot].stall_ticks, 0);
    state
        .validate_invariants()
        .expect("the count left with the route");
}

#[test]
fn footprint_escape_routes_rotate_with_the_body() {
    let state = Scenario::skirmish().build().unwrap();
    let mirror = |tile: TilePos| {
        TilePos::new(
            state.map.width() - 1 - tile.x,
            state.map.height() - 1 - tile.y,
        )
    };
    for from in [
        TilePos::new(4, 4),
        TilePos::new(5, 4),
        TilePos::new(4, 5),
        TilePos::new(5, 5),
    ] {
        assert!(!state.passable(from));
        assert!(!state.passable(mirror(from)));
        for heading in 0..128 {
            let a = escape_route(&state, UnitKind::Harvester, from, heading).unwrap();
            let b = escape_route(&state, UnitKind::Harvester, mirror(from), heading + 128).unwrap();
            assert_eq!(b.goal, mirror(a.goal));
            assert_eq!(
                b.waypoints,
                a.waypoints.into_iter().map(mirror).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn mirrored_collision_pushes_agree_at_rock_faces_and_corners() {
    let radius = UnitKind::Sentinel.stats().radius;
    let separation = Fx::lit("0.5");
    let step = ((radius + radius - separation) * chassis::fx::HALF).min(COLLISION_MAX_STEP);
    let extent = Vec2Fx::new(Fx::from_num(40), Fx::from_num(24));
    for (boundary, inward) in [
        (
            Vec2Fx::new(Fx::lit("12.5"), Fx::from_num(8)),
            Vec2Fx::new(Fx::ZERO, Fx::ONE),
        ),
        (
            Vec2Fx::new(Fx::from_num(13), Fx::lit("7.5")),
            Vec2Fx::new(Fx::ONE, Fx::ZERO),
        ),
        (
            Vec2Fx::new(Fx::from_num(13), Fx::from_num(8)),
            Vec2Fx::new(Fx::ZERO, Fx::ONE),
        ),
    ] {
        for offset in [-Fx::DELTA, Fx::ZERO, Fx::DELTA] {
            let mut scenario = Scenario::skirmish();
            let mut rows = vec![vec!['.'; 40]; 24];
            for (x, y, tile) in [(4, 4, '1'), (34, 18, '2'), (12, 7, '#'), (27, 16, '#')] {
                rows[y][x] = tile;
            }
            scenario.map = rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect();
            scenario.units = (0..4)
                .map(|i| UnitSpec {
                    player: i / 2,
                    kind: UnitKind::Sentinel,
                    x: 15 + i32::from(i),
                    y: 10,
                })
                .collect();
            let mut state = scenario.build().unwrap();
            let first = boundary + inward * (step + offset);
            let second = first + inward * separation;
            for (unit, position) in
                state
                    .units
                    .iter_mut()
                    .zip([first, second, extent - first, extent - second])
            {
                unit.pos = position;
            }
            let ranks = owner_local_ranks(&state);
            let mut index = UnitIndex::new();
            let pairs = collision_pairs(&state, false, &mut index, &ranks).unwrap_or_default();
            relaxation_pass(&mut state, &[Vec2Fx::ZERO; 4], &ranks, &mut vec![], &pairs);
            for (a, b) in [(0, 2), (1, 3)] {
                assert_eq!(
                    state.units[b].pos,
                    extent - state.units[a].pos,
                    "boundary={boundary:?}, offset={offset}"
                );
            }
        }
    }
}

#[test]
fn collision_boundaries_respect_domain_and_all_touching_obstacles() {
    use crate::stats::Domain;
    for terrain in ['#', '^', 's'] {
        let mut scenario = Scenario::skirmish();
        let mut rows = vec![vec!['.'; 40]; 24];
        rows[4][4] = '1';
        rows[18][34] = '2';
        rows[7][12] = terrain;
        rows[16][27] = terrain;
        scenario.map = rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect();
        scenario.units.clear();
        let state = scenario.build().unwrap();
        let extent = Vec2Fx::new(Fx::from_num(40), Fx::from_num(24));
        for (edge, direction) in [
            (
                Vec2Fx::new(Fx::lit("12.5"), Fx::from_num(8)),
                Vec2Fx::new(Fx::ZERO, Fx::ONE),
            ),
            (
                Vec2Fx::new(Fx::from_num(13), Fx::lit("7.5")),
                Vec2Fx::new(Fx::ONE, Fx::ZERO),
            ),
            (
                Vec2Fx::new(Fx::from_num(13), Fx::from_num(8)),
                Vec2Fx::new(Fx::ONE, Fx::ONE),
            ),
        ] {
            for delta in [-Fx::DELTA, Fx::ZERO, Fx::DELTA] {
                let pos = edge + direction * delta;
                for domain in [Domain::Ground, Domain::Air] {
                    let expected = delta > Fx::ZERO || (domain == Domain::Air && terrain != '^');
                    assert_eq!(collision_position_open(&state, domain, pos), expected);
                    assert_eq!(
                        collision_position_open(&state, domain, extent - pos),
                        expected
                    );
                }
            }
        }
        for pos in [
            Vec2Fx::new(Fx::from_num(6), Fx::from_num(5)),
            Vec2Fx::new(Fx::from_num(6), Fx::from_num(6)),
        ] {
            assert!(!collision_position_open(&state, Domain::Ground, pos));
            assert!(!collision_position_open(
                &state,
                Domain::Ground,
                extent - pos
            ));
            assert!(collision_position_open(&state, Domain::Air, pos));
            assert!(collision_position_open(&state, Domain::Air, extent - pos));
        }
        assert!(collision_position_open(
            &state,
            Domain::Ground,
            TilePos::new(20, 12).center()
        ));
        assert!(collision_position_open(
            &state,
            Domain::Ground,
            Vec2Fx::new(Fx::from_num(20), Fx::from_num(12))
        ));
    }
}

#[test]
fn shallow_air_bearing_does_not_alternate_across_the_goal_ray() {
    let map = Map::parse(&vec![".".repeat(200); 200]).unwrap().0;
    for heading in [0u8, 64, 128, 192] {
        let mut unit = boundary_pair().units[0].clone();
        unit.kind = UnitKind::Condor;
        unit.heading = heading;
        unit.pos = Vec2Fx::new(Fx::from_num(100), Fx::from_num(100));
        let forward = chassis::compass::dir(heading);
        let sideways = chassis::compass::dir(heading.wrapping_add(64));
        let target = unit.pos + forward * Fx::from_num(80) + sideways;
        for _ in 0..200 {
            steer_toward(&mut unit, &map, UnitKind::Condor.stats(), target);
            assert_eq!(
                unit.heading, heading,
                "straight approach must not wag its nose"
            );
            unit.pos += chassis::compass::dir(unit.heading) * unit.kind.stats().speed;
        }
        let target = unit.pos + sideways * Fx::from_num(20);
        steer_toward(&mut unit, &map, UnitKind::Condor.stats(), target);
        assert_eq!(
            unit.heading,
            heading.wrapping_add(unit.kind.stats().turn_rate)
        );
    }
}

fn seat(name: &str, faction: Faction) -> PlayerSpec {
    PlayerSpec {
        name: name.into(),
        faction,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    }
}

fn boundary_pair() -> State {
    Scenario {
        mode: ScenarioMode::Match,
        name: "boundary-pair".into(),
        map: vec![
            "............".into(),
            "............".into(),
            "............".into(),
            "1.........2.".into(),
            "............".into(),
            "............".into(),
            "............".into(),
            "............".into(),
        ],
        players: vec![
            seat("North", Faction::Ferrous),
            seat("South", Faction::Cupric),
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 5,
                y: 1,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 6,
                y: 1,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("boundary pair builds")
}

#[test]
fn coasting_worker_is_not_anchored_until_its_motor_stops() {
    let state = boundary_pair();
    let terrain = state.ground_terrain();
    let parked = ParkedBodies::default();
    let mut unit = state.units[0].clone();
    let unit = &mut unit;
    unit.kind = UnitKind::Harvester;
    unit.heading = 0;
    unit.order = Order::Harvest {
        node: TilePos::new(7, 1),
        anchor: TilePos::new(7, 1),
        retiring: false,
    };
    unit.drive_speed = unit.kind.stats().speed;
    let before = unit.pos;
    assert!(!is_anchored(unit));
    ground::advance(unit, &terrain, &parked);
    assert!(unit.pos.x > before.x);
    assert!(!is_anchored(unit));
    for _ in 0..2 {
        ground::advance(unit, &terrain, &parked);
    }
    assert_eq!(unit.drive_speed, Fx::ZERO);
    assert!(is_anchored(unit));
}

fn corner_shortcut_pair(
    name: &str,
    kind: UnitKind,
    pos: Vec2Fx,
    waypoint: TilePos,
    next: TilePos,
    blocked: TilePos,
) -> State {
    let width = 20;
    let height = 14;
    let mirror_tile = |tile: TilePos| TilePos::new(width - 1 - tile.x, height - 1 - tile.y);
    let mut map = vec![".".repeat(as_index(width)); as_index(height)];
    map[1].replace_range(1..2, "1");
    map[as_index(height) - 2].replace_range(as_index(width) - 2..as_index(width) - 1, "2");
    map[as_index(blocked.y)].replace_range(as_index(blocked.x)..=as_index(blocked.x), "#");
    let mirrored_blocked = mirror_tile(blocked);
    map[as_index(mirrored_blocked.y)].replace_range(
        as_index(mirrored_blocked.x)..=as_index(mirrored_blocked.x),
        "#",
    );

    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: name.into(),
        map,
        players: vec![
            seat("West", Faction::Ferrous),
            seat("East", Faction::Cupric),
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind,
                x: pos.x.floor().to_num(),
                y: pos.y.floor().to_num(),
            },
            UnitSpec {
                player: 1,
                kind,
                x: width - 1 - pos.x.floor().to_num::<i32>(),
                y: height - 1 - pos.y.floor().to_num::<i32>(),
            },
        ],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("corner-shortcut pair builds");

    let mirrored_pos = Vec2Fx::new(Fx::from_num(width) - pos.x, Fx::from_num(height) - pos.y);
    let paths = [
        PathFollow {
            final_point: None,
            goal: next,
            waypoints: vec![waypoint, next],
            next: 0,
        },
        PathFollow {
            final_point: None,
            goal: mirror_tile(next),
            waypoints: vec![mirror_tile(waypoint), mirror_tile(next)],
            next: 0,
        },
    ];
    for ((unit, position), path) in state.units.iter_mut().zip([pos, mirrored_pos]).zip(paths) {
        unit.heading =
            chassis::compass::heading_of(path.waypoints[path.next as usize].center() - position);
        unit.pos = position;
        unit.order = Order::Run {
            goal: path.goal.into(),
        };
        unit.path = Some(path);
    }
    state
}

fn assert_corner_shortcut_pair_reaches_goal(mut state: State) {
    let width = Fx::from_num(state.map.width());
    let height = Fx::from_num(state.map.height());
    let starts = [state.units[0].pos, state.units[1].pos];

    for _ in 0..64 {
        run(&mut state);
        assert_eq!(
            state.units[1].pos,
            Vec2Fx::new(width - state.units[0].pos.x, height - state.units[0].pos.y),
            "corner recovery lost half-turn symmetry"
        );
        if state.units.iter().all(|unit| unit.path.is_none()) {
            break;
        }
    }

    for (unit, start) in state.units.iter().zip(starts) {
        assert_ne!(
            unit.pos, start,
            "the worker never escaped the blocked corner"
        );
        let Order::Run { goal } = unit.order else {
            panic!("test worker lost its move order")
        };
        assert_eq!(unit.pos, goal.tile().center());
        assert!(
            unit.path.is_none(),
            "the worker did not finish within the bound"
        );
    }
}

/// A Harvester south of its own Fabricator's south-east corner with a
/// route to (10, 3) that first rounds that corner, and the half-turned
/// copy for the other seat.
fn corner_hugging_pair(offset: Vec2Fx, heading: u8, next: u32) -> State {
    let width = 32;
    let height = 14;
    let mirror_tile = |tile: TilePos| TilePos::new(width - 1 - tile.x, height - 1 - tile.y);
    let mut map = vec![".".repeat(as_index(width)); as_index(height)];
    map[1].replace_range(1..2, "1");
    map[as_index(height) - 3].replace_range(as_index(width) - 3..as_index(width) - 2, "2");
    let anchor = TilePos::new(6, 6);
    let mirrored_anchor = TilePos::new(width - 2 - anchor.x, height - 2 - anchor.y);
    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: "corner-hugging-pair".into(),
        map,
        players: vec![
            seat("West", Faction::Ferrous),
            seat("East", Faction::Cupric),
        ],
        units: [(0, anchor), (1, mirrored_anchor)]
            .into_iter()
            .map(|(player, anchor)| UnitSpec {
                player,
                kind: UnitKind::Harvester,
                x: anchor.x + 1,
                y: anchor.y + 2,
            })
            .collect(),
        buildings: [(0, anchor), (1, mirrored_anchor)]
            .into_iter()
            .map(|(player, anchor)| crate::scenario::BuildingSpec {
                player,
                kind: crate::stats::BuildingKind::Fabricator,
                x: anchor.x,
                y: anchor.y,
            })
            .collect(),
        meta: None,
    }
    .build()
    .expect("corner-hugging pair builds");

    let corner = Vec2Fx::new(Fx::from_num(anchor.x + 2), Fx::from_num(anchor.y + 2));
    let pos = corner + offset;
    let mirrored_pos = Vec2Fx::new(Fx::from_num(width) - pos.x, Fx::from_num(height) - pos.y);
    let waypoints = tiles(&[(8, 8), (9, 7), (10, 6), (10, 5), (10, 4), (10, 3)]);
    let paths = [
        PathFollow {
            final_point: None,
            goal: *waypoints.last().unwrap(),
            waypoints: waypoints.clone(),
            next,
        },
        PathFollow {
            final_point: None,
            goal: mirror_tile(*waypoints.last().unwrap()),
            waypoints: waypoints.iter().copied().map(mirror_tile).collect(),
            next,
        },
    ];
    let headings = [heading, heading.wrapping_add(128)];
    for (((unit, position), path), heading) in state
        .units
        .iter_mut()
        .zip([pos, mirrored_pos])
        .zip(paths)
        .zip(headings)
    {
        unit.pos = position;
        unit.heading = heading;
        unit.order = Order::Run {
            goal: path.goal.into(),
        };
        unit.path = Some(path);
    }
    state
}

fn tiles(points: &[(i32, i32)]) -> Vec<TilePos> {
    points.iter().map(|&(x, y)| TilePos::new(x, y)).collect()
}

fn assert_corner_hugging_pair_arrives(mut state: State) {
    let width = Fx::from_num(state.map.width());
    let height = Fx::from_num(state.map.height());
    let goal = TilePos::new(10, 3);
    let clearance = |state: &State| {
        let unit = &state.units[0];
        let fabricator = state
            .buildings
            .iter()
            .find(|b| b.player == unit.player && b.kind == crate::stats::BuildingKind::Fabricator)
            .expect("the pair has a Fabricator");
        unit.pos.dist(fabricator.closest_point_to(unit.pos))
    };
    let start = clearance(&state);
    for _ in 0..240 {
        let report = state.tick(&[]);
        assert!(
            clearance(&state) >= start,
            "the Harvester steered closer to the Fabricator: {:?}",
            state.units[0].pos
        );
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, crate::Event::OrderStalled { .. })),
            "an open route must not stall"
        );
        assert_eq!(
            state.units[1].pos,
            Vec2Fx::new(width - state.units[0].pos.x, height - state.units[0].pos.y),
            "corner recovery lost half-turn symmetry"
        );
        if state.units[0].order == Order::Idle {
            break;
        }
    }
    let unit = &state.units[0];
    assert_eq!(
        unit.tile(),
        goal,
        "the Harvester pinned itself on the corner at {:?}",
        unit.pos
    );
    assert_eq!(unit.order, Order::Idle);
    assert!(unit.path.is_none());
}

#[test]
fn a_body_pinned_on_a_building_corner_drives_off_it() {
    let state = corner_hugging_pair(
        Vec2Fx::new(Fx::lit("-0.005956769"), Fx::lit("0.0085311425")),
        217,
        1,
    );
    assert_corner_hugging_pair_arrives(state);
}

#[test]
fn a_hull_overlapping_a_corner_never_steers_a_leg_across_it() {
    let state = corner_hugging_pair(
        Vec2Fx::new(Fx::lit("-0.182642685"), Fx::lit("0.1853985682")),
        240,
        0,
    );
    assert_corner_hugging_pair_arrives(state);
}

fn collision_trio() -> State {
    Scenario {
        mode: ScenarioMode::Match,
        name: "collision-trio".into(),
        map: vec![
            "............".into(),
            "............".into(),
            "............".into(),
            "1.........2.".into(),
            "............".into(),
            "............".into(),
            "............".into(),
            "............".into(),
        ],
        players: vec![
            seat("North", Faction::Ferrous),
            seat("South", Faction::Cupric),
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 5,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 4,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 6,
                y: 3,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("collision trio builds")
}

fn replay_center_crossing() -> State {
    let mut map = vec![".".repeat(48); 30];
    map[5].replace_range(5..6, "1");
    map[24].replace_range(42..43, "2");
    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: "replay-center-crossing".into(),
        map,
        players: vec![
            seat("West", Faction::Ferrous),
            seat("East", Faction::Ferrous),
        ],
        units: [0, 1, 0, 1]
            .into_iter()
            .zip(10..)
            .map(|(player, x)| UnitSpec {
                player,
                kind: UnitKind::Flakhound,
                x,
                y: 10,
            })
            .collect(),
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("replay-shaped crossing builds");
    state.tick = 12_269;
    let positions = [
        Vec2Fx::new(
            Fx::from_bits(104_588_663_713),
            Fx::from_bits(66_428_111_470),
        ),
        Vec2Fx::new(
            Fx::from_bits(101_569_766_495),
            Fx::from_bits(62_420_907_410),
        ),
        Vec2Fx::new(
            Fx::from_bits(101_367_326_202),
            Fx::from_bits(65_708_255_908),
        ),
        Vec2Fx::new(
            Fx::from_bits(104_791_104_006),
            Fx::from_bits(63_140_762_972),
        ),
    ];
    let paths = [
        PathFollow {
            final_point: None,
            goal: TilePos::new(22, 13),
            waypoints: vec![
                TilePos::new(24, 15),
                TilePos::new(23, 14),
                TilePos::new(22, 13),
            ],
            next: 1,
        },
        PathFollow {
            final_point: None,
            goal: TilePos::new(25, 16),
            waypoints: vec![
                TilePos::new(23, 14),
                TilePos::new(24, 15),
                TilePos::new(25, 16),
            ],
            next: 1,
        },
        PathFollow {
            final_point: None,
            goal: TilePos::new(23, 14),
            waypoints: vec![TilePos::new(23, 15), TilePos::new(23, 14)],
            next: 1,
        },
        PathFollow {
            final_point: None,
            goal: TilePos::new(24, 15),
            waypoints: vec![TilePos::new(24, 14), TilePos::new(24, 15)],
            next: 1,
        },
    ];
    for ((unit, pos), path) in state.units.iter_mut().zip(positions).zip(paths) {
        unit.heading =
            chassis::compass::heading_of(path.waypoints[path.next as usize].center() - pos);
        unit.pos = pos;
        unit.order = Order::Hunt {
            goal: path.goal.into(),
        };
        unit.path = Some(path);
    }
    state
}

fn assert_replay_pairs_are_half_turns(state: &State) {
    let center_twice = Vec2Fx::new(
        Fx::from_num(state.map.width()),
        Fx::from_num(state.map.height()),
    );
    for (west, east) in [(0, 1), (2, 3)] {
        assert_eq!(
            state.units[east].pos,
            center_twice - state.units[west].pos,
            "owner-local rank {west:?}/{east:?} lost half-turn symmetry"
        );
    }
}

fn assert_collision_half_turn(mut original: State, travel: &[Vec2Fx]) {
    let width = Fx::from_num(original.map.width());
    let height = Fx::from_num(original.map.height());
    let mut rotated = original.clone();
    for unit in &mut rotated.units {
        unit.pos = Vec2Fx::new(width - unit.pos.x, height - unit.pos.y);
    }
    let rotated_travel = travel.iter().map(|step| -*step).collect::<Vec<_>>();

    let mut original_index = UnitIndex::new();
    let mut rotated_index = UnitIndex::new();
    resolve_collisions(&mut original, travel, &mut original_index);
    resolve_collisions(&mut rotated, &rotated_travel, &mut rotated_index);

    for (unit, rotated_unit) in original.units.iter().zip(&rotated.units) {
        assert_eq!(unit.id, rotated_unit.id);
        assert_eq!(
            rotated_unit.pos,
            Vec2Fx::new(width - unit.pos.x, height - unit.pos.y),
            "collision resolution favored an absolute direction for {:?}",
            unit.id
        );
    }
}

#[test]
fn ordered_multi_body_collision_is_equivariant_under_half_turns() {
    let mut state = collision_trio();
    state.units[0].pos = Vec2Fx::new(Fx::lit("5.5"), Fx::lit("3.5"));
    state.units[1].pos = Vec2Fx::new(Fx::lit("5.05"), Fx::lit("3.35"));
    state.units[2].pos = Vec2Fx::new(Fx::lit("5.95"), Fx::lit("3.65"));
    let travel = vec![
        Vec2Fx::new(Fx::lit("0.04"), Fx::lit("0.01")),
        Vec2Fx::new(Fx::lit("0.08"), Fx::lit("0.02")),
        Vec2Fx::new(Fx::lit("-0.05"), Fx::lit("-0.01")),
    ];

    assert_collision_half_turn(state, &travel);
}

#[test]
fn mirrored_crossing_armies_remain_exact_half_turns_through_collision() {
    let mut state = replay_center_crossing();
    assert_replay_pairs_are_half_turns(&state);
    let (travel, _) = run(&mut state);
    assert_replay_pairs_are_half_turns(&state);
    let mut index = UnitIndex::new();

    resolve_collisions(&mut state, &travel, &mut index);

    assert_replay_pairs_are_half_turns(&state);
}

#[test]
fn perfectly_stacked_collision_is_equivariant_under_half_turns() {
    let mut state = collision_trio();
    let stack = Vec2Fx::new(Fx::lit("5.5"), Fx::lit("3.5"));
    for unit in &mut state.units {
        unit.pos = stack;
    }

    assert_collision_half_turn(state, &[Vec2Fx::ZERO; 3]);
}

#[test]
fn mirrored_seat_stacks_ignore_global_id_blocks() {
    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: "mirrored-seat-stacks".into(),
        map: vec![
            "............".into(),
            "............".into(),
            "............".into(),
            "1.........2.".into(),
            "............".into(),
            "............".into(),
            "............".into(),
            "............".into(),
        ],
        players: vec![
            seat("West", Faction::Ferrous),
            seat("East", Faction::Ferrous),
        ],
        units: (0..6)
            .map(|slot| UnitSpec {
                player: u8::from(slot >= 3),
                kind: UnitKind::Sentinel,
                x: 2 + slot,
                y: 2,
            })
            .collect(),
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("mirrored stack scenario builds");
    let width = Fx::from_num(state.map.width());
    let height = Fx::from_num(state.map.height());
    let west_stack = Vec2Fx::new(Fx::lit("4.5"), Fx::lit("3.5"));
    let east_stack = Vec2Fx::new(width - west_stack.x, height - west_stack.y);
    state.units[0].pos = west_stack;
    state.units[1].pos = west_stack;
    state.units[2].pos = Vec2Fx::new(Fx::lit("2.5"), Fx::lit("1.5"));
    state.units[3].pos = east_stack;
    state.units[4].pos = east_stack;
    state.units[5].pos = Vec2Fx::new(width - state.units[2].pos.x, height - state.units[2].pos.y);
    let mut index = UnitIndex::new();

    resolve_collisions(&mut state, &[Vec2Fx::ZERO; 6], &mut index);

    for (west, east) in [(0, 3), (1, 4)] {
        assert_eq!(
            state.units[east].pos,
            Vec2Fx::new(
                width - state.units[west].pos.x,
                height - state.units[west].pos.y,
            ),
            "matching owner-local ranks must separate in mirrored directions"
        );
    }
}

#[test]
fn collision_finds_border_crossing_pairs_in_either_id_order() {
    let height = boundary_pair().map.height();
    let edges = [
        ("north", Fx::lit("0.1"), Fx::lit("-0.1")),
        (
            "south",
            Fx::from_num(height) - Fx::lit("0.1"),
            Fx::from_num(height) + Fx::lit("0.1"),
        ),
    ];

    for (edge, inside_y, outside_y) in edges {
        for outside_slot in 0..2 {
            let mut state = boundary_pair();
            let inside_slot = 1 - outside_slot;
            state.units[outside_slot].pos = Vec2Fx::new(Fx::lit("5.5"), outside_y);
            state.units[inside_slot].pos = Vec2Fx::new(Fx::lit("5.5"), inside_y);
            state.refresh_vision();
            state
                .validate_invariants()
                .expect("the accepted coordinate envelope includes border rows");
            let before = state.units[inside_slot].pos;
            let travel = vec![Vec2Fx::ZERO; state.units.len()];
            let mut index = UnitIndex::new();
            let owner_ranks = owner_local_ranks(&state);
            let mut spent = Vec::new();
            let pairs = collision_pairs(&state, false, &mut index, &owner_ranks)
                .expect("the boundary pair overlaps");

            assert!(
                relaxation_pass(&mut state, &travel, &owner_ranks, &mut spent, &pairs),
                "{edge} pair with outside slot {outside_slot} was not visited"
            );
            assert_ne!(
                state.units[inside_slot].pos, before,
                "{edge} pair with outside slot {outside_slot} was not separated"
            );
        }
    }
}

fn resting_overlap(kinds: [UnitKind; 2], separation: Fx) -> [Fx; 2] {
    let mut state = boundary_pair();
    let center = TilePos::new(5, 2).center();
    let half = separation * chassis::fx::HALF;
    for ((unit, kind), side) in state.units.iter_mut().zip(kinds).zip([-Fx::ONE, Fx::ONE]) {
        unit.kind = kind;
        unit.pos = Vec2Fx::new(center.x + half * side, center.y);
    }
    let before: Vec<Vec2Fx> = state.units.iter().map(|unit| unit.pos).collect();
    let travel = vec![Vec2Fx::ZERO; state.units.len()];
    resolve_collisions(&mut state, &travel, &mut UnitIndex::new());
    [0, 1].map(|slot| state.units[slot].pos.dist(before[slot]))
}

#[test]
fn a_contact_within_the_slop_rests_and_a_deeper_one_separates() {
    let spacing = UnitKind::Sentinel.stats().radius * 2;
    let resting = resting_overlap([UnitKind::Sentinel; 2], spacing - COLLISION_SLOP / 2);
    assert_eq!(
        resting,
        [Fx::ZERO; 2],
        "rounding-deep contact needs no push"
    );
    let [left, right] = resting_overlap([UnitKind::Sentinel; 2], spacing - COLLISION_SLOP * 2);
    assert!(left > Fx::ZERO && right > Fx::ZERO);
}

#[test]
fn equal_bodies_split_an_overlap_exactly_in_half() {
    let [left, right] = resting_overlap([UnitKind::Sentinel; 2], Fx::lit("0.6"));
    assert!(left > Fx::ZERO);
    assert_eq!(left, right);
}

#[test]
fn a_heavy_hull_yields_less_than_the_light_one_shoving_it() {
    let kinds = [UnitKind::Sentinel, UnitKind::Breaker];
    let radii = kinds.map(|kind| kind.stats().radius);
    let overlap = Fx::lit("0.1");
    let [light, heavy] = resting_overlap(kinds, radii[0] + radii[1] - overlap);
    let tolerance = Fx::lit("0.000001");
    assert!(heavy > Fx::ZERO && light > heavy + heavy);
    assert!((light + heavy - overlap).abs() < tolerance);
    let imbalance = light * radii[0] * radii[0] - heavy * radii[1] * radii[1];
    assert!(
        imbalance.abs() < tolerance,
        "shares are not inverse to area"
    );
}

#[test]
fn mixed_size_collision_is_equivariant_under_half_turns() {
    let mut state = collision_trio();
    state.units[0].kind = UnitKind::Breaker;
    state.units[2].kind = UnitKind::Scuttler;
    state.units[0].pos = Vec2Fx::new(Fx::lit("5.5"), Fx::lit("3.5"));
    state.units[1].pos = Vec2Fx::new(Fx::lit("4.85"), Fx::lit("3.3"));
    state.units[2].pos = Vec2Fx::new(Fx::lit("6.1"), Fx::lit("3.75"));
    let travel = vec![
        Vec2Fx::new(Fx::lit("0.02"), Fx::lit("0.01")),
        Vec2Fx::new(Fx::lit("0.08"), Fx::lit("0.02")),
        Vec2Fx::new(Fx::lit("-0.12"), Fx::lit("-0.03")),
    ];

    assert_collision_half_turn(state, &travel);
}

/// A seat's light body meeting the other seat's heavy one, and the
/// half-turn image with the kinds exchanged between the seats: which body
/// the pair loop reaches first differs, so the split must not depend on it.
#[test]
fn mixed_size_split_ignores_which_seat_owns_the_heavier_body() {
    let kinds = [UnitKind::Sentinel, UnitKind::Breaker];
    let mut original = boundary_pair();
    let width = Fx::from_num(original.map.width());
    let height = Fx::from_num(original.map.height());
    let rotate = |pos: Vec2Fx| Vec2Fx::new(width - pos.x, height - pos.y);
    for step in 0..64 {
        let gap = Fx::lit("0.5") + Fx::lit("0.00617") * Fx::from_num(step);
        let at = [
            Vec2Fx::new(Fx::lit("5.31"), Fx::lit("2.47")),
            Vec2Fx::new(
                Fx::lit("5.31") + gap,
                Fx::lit("2.47") + gap * Fx::lit("0.37"),
            ),
        ];
        let mut rotated = original.clone();
        for slot in 0..2 {
            original.units[slot].kind = kinds[slot];
            original.units[slot].pos = at[slot];
            rotated.units[slot].kind = kinds[1 - slot];
            rotated.units[slot].pos = rotate(at[1 - slot]);
        }
        let travel = vec![Vec2Fx::ZERO; 2];
        let mut a = original.clone();
        resolve_collisions(&mut a, &travel, &mut UnitIndex::new());
        resolve_collisions(&mut rotated, &travel, &mut UnitIndex::new());
        for slot in 0..2 {
            assert_eq!(
                rotated.units[slot].pos,
                rotate(a.units[1 - slot].pos),
                "gap {gap}: the split depended on pair order"
            );
        }
    }
}

#[test]
fn collision_budget_spans_every_relaxation_pass_in_a_tick() {
    let mut state = boundary_pair();
    let stacked = TilePos::new(5, 2).center();
    for unit in &mut state.units {
        unit.pos = stacked;
    }
    let before: Vec<Vec2Fx> = state.units.iter().map(|u| u.pos).collect();
    let travel = vec![Vec2Fx::ZERO; state.units.len()];
    let mut index = UnitIndex::new();

    resolve_collisions(&mut state, &travel, &mut index);

    for (unit, before) in state.units.iter().zip(before) {
        let correction = unit.pos.dist(before);
        assert!(
            correction <= COLLISION_MAX_STEP,
            "{:?} received {correction:?} of correction in one tick",
            unit.id
        );
    }
}

#[test]
fn coordinated_head_on_slide_is_rotation_equivariant() {
    let away = Vec2Fx::new(Fx::lit("0.6"), Fx::lit("0.8"));
    let travel = -away;
    let original = correction_dirs(away, travel, true);
    let rotated = correction_dirs(-away, -travel, true);

    for (original, rotated) in original.into_iter().zip(rotated) {
        let (Some(original), Some(rotated)) = (original, rotated) else {
            assert_eq!(original, rotated);
            continue;
        };
        let error = original + rotated;
        let tolerance = Fx::DELTA * 2;
        assert!(error.x.abs() <= tolerance);
        assert!(error.y.abs() <= tolerance);
    }
}

#[test]
fn oriented_collision_rows_reverse_x_groups_without_reversing_slot_order() {
    let row = [
        (TilePos::new(3, 4), 1),
        (TilePos::new(3, 4), 5),
        (TilePos::new(4, 4), 2),
        (TilePos::new(6, 4), 0),
        (TilePos::new(6, 4), 7),
    ];

    assert_eq!(
        OrientedRow::new(&row, false).collect::<Vec<_>>(),
        [1, 5, 2, 0, 7]
    );
    assert_eq!(
        OrientedRow::new(&row, true).collect::<Vec<_>>(),
        [0, 7, 2, 1, 5]
    );
    assert!(OrientedRow::new(&[], true).next().is_none());
}

#[test]
fn passed_waypoint_still_rejects_a_blocked_next_step() {
    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: "blocked-next-waypoint".into(),
        map: vec![
            "............".into(),
            "............".into(),
            "......#.....".into(),
            "1.........2.".into(),
            "............".into(),
            "............".into(),
            "............".into(),
            "............".into(),
        ],
        players: vec![
            seat("North", Faction::Ferrous),
            seat("South", Faction::Cupric),
        ],
        units: vec![UnitSpec {
            player: 0,
            kind: UnitKind::Avalanche,
            x: 5,
            y: 2,
        }],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("blocked waypoint state builds");
    let unit = &mut state.units[0];
    unit.pos = Vec2Fx::new(Fx::lit("5.9"), Fx::lit("2.5"));
    unit.order = Order::Run {
        goal: TilePos::new(6, 2).into(),
    };
    unit.path = Some(PathFollow {
        final_point: None,
        goal: TilePos::new(6, 2),
        waypoints: vec![TilePos::new(5, 2), TilePos::new(6, 2)],
        next: 0,
    });
    let before = unit.pos;

    run(&mut state);

    assert_eq!(state.units[0].pos, before);
    assert!(state.units[0].path.is_none());
    assert!(state.map.terrain_passable(state.units[0].tile()));
}

#[test]
fn harvester_deflected_past_a_waypoint_does_not_drop_its_route() {
    let state = corner_shortcut_pair(
        "harvester-corner-shortcut",
        UnitKind::Harvester,
        Vec2Fx::new(Fx::lit("7.0030496502"), Fx::lit("7.5430232098")),
        TilePos::new(6, 7),
        TilePos::new(6, 8),
        TilePos::new(7, 8),
    );

    assert_corner_shortcut_pair_reaches_goal(state);
}

#[test]
fn excavator_deflected_past_a_waypoint_does_not_drop_its_route() {
    let state = corner_shortcut_pair(
        "excavator-corner-shortcut",
        UnitKind::Excavator,
        Vec2Fx::new(Fx::lit("8.4923983465"), Fx::lit("6.9311533265")),
        TilePos::new(8, 7),
        TilePos::new(7, 7),
        TilePos::new(7, 6),
    );

    assert_corner_shortcut_pair_reaches_goal(state);
}

#[test]
fn diagonal_head_on_pair_passes_shared_waypoint() {
    let mut state = boundary_pair();
    state.tick = 21_549;
    let shared = TilePos::new(5, 3);
    let paths = [
        PathFollow {
            final_point: None,
            goal: TilePos::new(2, 1),
            waypoints: vec![
                shared,
                TilePos::new(4, 2),
                TilePos::new(3, 1),
                TilePos::new(2, 1),
            ],
            next: 0,
        },
        PathFollow {
            final_point: None,
            goal: TilePos::new(9, 6),
            waypoints: vec![
                shared,
                TilePos::new(6, 4),
                TilePos::new(7, 5),
                TilePos::new(8, 6),
                TilePos::new(9, 6),
            ],
            next: 0,
        },
    ];
    let positions = [
        Vec2Fx::new(Fx::lit("5.922209162"), Fx::lit("3.837034112")),
        Vec2Fx::new(Fx::lit("5.236790039"), Fx::lit("3.125446076")),
    ];
    for ((unit, pos), path) in state.units.iter_mut().zip(positions).zip(paths) {
        unit.kind = UnitKind::Avalanche;
        unit.heading =
            chassis::compass::heading_of(path.waypoints[path.next as usize].center() - pos);
        unit.hp = UnitKind::Avalanche.stats().max_hp;
        unit.pos = pos;
        unit.order = Order::Run {
            goal: path.goal.into(),
        };
        unit.path = Some(path);
    }

    let mut index = UnitIndex::new();
    for _ in 0..80 {
        let (travel, _) = run(&mut state);
        resolve_collisions(&mut state, &travel, &mut index);
        state.tick += 1;
    }

    assert!(
        state
            .units
            .iter()
            .all(|unit| { unit.path.as_ref().is_none_or(|path| path.next > 0) }),
        "both Avalanches must pass the shared waypoint instead of oscillating: {:?}",
        state
            .units
            .iter()
            .map(|unit| (unit.id, unit.pos, unit.path.as_ref().map(|path| path.next)))
            .collect::<Vec<_>>()
    );
}
