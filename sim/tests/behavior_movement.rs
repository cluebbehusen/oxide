//! Movement, routing, collision, and order programs — behavior suite, public API only.

mod common;

use chassis::grid::TilePos;
use oxide_sim::scenario::PlayerSpec;
use oxide_sim::{Command, Event, Faction, Order, PlayerId, Scenario, State, UnitId, UnitKind};

use common::*;

#[test]
fn move_command_walks_unit_to_goal_then_idles() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let goal = TilePos::new(13, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);
    run_until(&mut state, 200, |s, _| {
        let u = s.unit(mover).unwrap();
        u.tile() == goal && u.order == Order::Idle
    });
}

#[test]
fn move_routes_around_rock() {
    // Goal sits directly behind the 2x2 rock at (6,3)-(7,4).
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 4)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let goal = TilePos::new(9, 4);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.unit(mover).unwrap().tile() == goal
    });
}

#[test]
fn move_goal_on_rock_snaps_to_nearby_ground() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal: TilePos::new(6, 3), // rock
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        let u = s.unit(mover).unwrap();
        u.order == Order::Idle && u.tile().chebyshev(TilePos::new(6, 3)) <= 1
    });
}

#[test]
fn units_ordered_to_one_tile_do_not_stack() {
    // Four harvesters converge on the same goal; collision resolution must
    // keep every pair at least (r_a + r_b) apart once things settle.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 3, 2),
        unit(0, UnitKind::Harvester, 12, 2),
        unit(0, UnitKind::Harvester, 3, 6),
        unit(0, UnitKind::Harvester, 12, 6),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids,
            goal: TilePos::new(8, 4),
            queue: false,
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
    }
    let min_gap = UnitKind::Harvester.stats().radius * 2;
    // Allow a whisker of tolerance: the final relaxation pass may leave a
    // sub-ulp of residual overlap.
    let tolerance = min_gap * chassis::fx::Fx::lit("0.9");
    for (i, a) in state.units().iter().enumerate() {
        for b in state.units().iter().skip(i + 1) {
            assert!(
                a.pos.dist_sq(b.pos) >= tolerance * tolerance,
                "{} and {} overlap: {:?} vs {:?}",
                a.id,
                b.id,
                a.pos,
                b.pos
            );
        }
    }
}

#[test]
fn collision_never_pushes_through_rock() {
    // Two units squeezed against the rock at (6,3)-(7,4): however hard the
    // crowd pushes, nobody ends up inside it.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 3),
        unit(0, UnitKind::Harvester, 5, 4),
        unit(0, UnitKind::Harvester, 4, 3),
        unit(0, UnitKind::Harvester, 4, 4),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids,
            goal: TilePos::new(5, 3),
            queue: false,
        },
    )]);
    for _ in 0..200 {
        state.tick(&[]);
        for u in state.units() {
            assert!(
                state.map().terrain_passable(u.tile()),
                "{} was pushed into impassable {:?}",
                u.id,
                u.tile()
            );
        }
    }
}

#[test]
fn congested_harvesters_keep_depositing() {
    // Six harvesters on one node, one Foundry: the deadlock regression.
    // Symmetric collision cancellation once froze exactly this setup with
    // full loads at the doorstep. Progress must continue, not just start.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(0, UnitKind::Harvester, 5, 2),
        unit(0, UnitKind::Harvester, 6, 2),
        unit(0, UnitKind::Harvester, 4, 5),
        unit(0, UnitKind::Harvester, 5, 5),
        unit(0, UnitKind::Harvester, 5, 6),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: ids,
            node: TilePos::new(11, 4),
            queue: false,
        },
    )]);

    let mut deposited_first_half = 0u32;
    let mut deposited_second_half = 0u32;
    for tick in 0..3000u64 {
        let report = state.tick(&[]);
        for event in &report.events {
            if let Event::ScrapDeposited { amount, .. } = event {
                if tick < 1500 {
                    deposited_first_half += amount;
                } else {
                    deposited_second_half += amount;
                }
            }
        }
    }
    assert!(
        deposited_first_half >= 60,
        "economy never started: {deposited_first_half}"
    );
    assert!(
        deposited_second_half >= 60,
        "economy stalled after starting: {deposited_second_half}"
    );
}

#[test]
fn group_moves_fan_out_over_distinct_tiles() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 3, 2),
        unit(0, UnitKind::Harvester, 4, 2),
        unit(0, UnitKind::Harvester, 3, 6),
        unit(0, UnitKind::Harvester, 4, 6),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids.clone(),
            goal: TilePos::new(10, 4),
            queue: false,
        },
    )]);
    for _ in 0..400 {
        state.tick(&[]);
    }
    // Everyone settled…
    assert!(
        state.units().iter().all(|u| u.order == Order::Idle),
        "group should settle: {:?}",
        state.units().iter().map(|u| u.order).collect::<Vec<_>>()
    );
    // …near the click, on distinct goals (spread), without stacking.
    let mut tiles: Vec<TilePos> = state.units().iter().map(|u| u.tile()).collect();
    for t in &tiles {
        assert!(
            t.chebyshev(TilePos::new(10, 4)) <= 3,
            "unit parked too far from the group goal: {t}"
        );
    }
    tiles.sort_unstable();
    tiles.dedup();
    assert!(
        tiles.len() >= 3,
        "group order must fan out, not stack on one tile"
    );
}

#[test]
fn congestion_survives_nonconsecutive_unit_ids() {
    // Doorstep rotation and pair iteration both walk unit ids; this run
    // punches holes in the sequence first. Three mid-id harvesters spawn
    // beside an enemy sentinel and die to its auto-acquire, then the
    // survivors (ids 0, 1, 4, 6, 7) crowd one node — the economy must keep
    // flowing exactly as it does with dense ids.
    let scenario = Scenario {
        name: "id-gaps".into(),
        seed: 42,
        map: vec![
            "########################".into(),
            "#1..........s..........#".into(),
            "#...........s..........#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#..................2...#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            unit(0, UnitKind::Harvester, 4, 1),
            unit(0, UnitKind::Harvester, 5, 1),
            unit(0, UnitKind::Harvester, 19, 4), // victim
            unit(0, UnitKind::Harvester, 20, 4), // victim
            unit(0, UnitKind::Harvester, 4, 2),
            unit(0, UnitKind::Harvester, 19, 5), // victim
            unit(0, UnitKind::Harvester, 6, 1),
            unit(0, UnitKind::Harvester, 5, 2),
            unit(1, UnitKind::Sentinel, 20, 5),
        ],
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().unwrap();

    let mut dead = 0;
    run_until(&mut state, 1500, |_, events| {
        dead += events
            .iter()
            .filter(|e| matches!(e, Event::UnitDied { .. }))
            .count();
        dead == 3
    });
    let survivors: Vec<UnitId> = state
        .units()
        .iter()
        .filter(|u| u.player == PlayerId(0))
        .map(|u| u.id)
        .collect();
    assert_eq!(
        survivors,
        [0, 1, 4, 6, 7].map(UnitId),
        "the wrong harvesters died"
    );

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: survivors,
            node: TilePos::new(12, 1),
            queue: false,
        },
    )]);
    let mut deposited = [0u32; 2];
    for tick in 0..3000u64 {
        let report = state.tick(&[]);
        for event in &report.events {
            if let Event::ScrapDeposited { amount, .. } = event {
                deposited[(tick >= 1500) as usize] += amount;
            }
        }
    }
    assert!(deposited[0] >= 50, "economy never started: {deposited:?}");
    assert!(
        deposited[1] >= 50,
        "economy stalled with gapped ids: {deposited:?}"
    );
}

#[test]
fn dense_stacks_respect_the_per_tick_displacement_cap() {
    // 100 units spawned on one tile. Per-pair clamping once let a unit in
    // k overlaps drift k × COLLISION_MAX_STEP in a single tick (measured
    // 1.8+ tiles), and resetting the budget between relaxation passes still
    // allowed three caps. One budget now spans the whole tick.
    let units = (0..100)
        .map(|_| unit(0, UnitKind::Harvester, 8, 3))
        .collect();
    let mut state = arena(units).build().unwrap();
    let before: Vec<_> = state.units().iter().map(|u| (u.id, u.pos)).collect();
    state.tick(&[]);
    let cap = oxide_sim::stats::COLLISION_MAX_STEP;
    let tolerance = chassis::fx::Fx::lit("0.0001");
    for (id, start) in before {
        let now = state.unit(id).unwrap().pos;
        let moved = (now - start).length_sq();
        assert!(
            moved <= (cap + tolerance) * (cap + tolerance),
            "{id} moved {moved:?}² in one tick (cap {cap:?})"
        );
    }
}

#[test]
fn a_harvester_crossing_a_parked_line_never_gets_a_collision_speed_burst() {
    fn timed_walk(mut state: State, mover: UnitId) -> (u64, chassis::fx::Fx) {
        state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: TilePos::new(35, 10),
                queue: false,
            },
        )]);
        let mut previous = state.unit(mover).expect("mover exists").pos;
        let mut max_step = chassis::fx::Fx::ZERO;
        for tick in 1..4_000 {
            state.tick(&[]);
            let unit = state.unit(mover).expect("mover survives");
            max_step = max_step.max(unit.pos.dist(previous));
            previous = unit.pos;
            if unit.order == Order::Idle {
                return (tick, max_step);
            }
        }
        panic!("harvester never crossed the lane");
    }

    let mut crowded_units = vec![unit(0, UnitKind::Harvester, 5, 10)];
    for y in 8..=12 {
        crowded_units.push(unit(0, UnitKind::Harvester, 20, y));
    }
    let crowded = open_arena(41, 21, crowded_units).build().unwrap();
    let mover = crowded.units()[0].id;
    let (crowded_ticks, max_step) = timed_walk(crowded, mover);

    let solo = open_arena(41, 21, vec![unit(0, UnitKind::Harvester, 5, 10)])
        .build()
        .unwrap();
    let solo_mover = solo.units()[0].id;
    let (solo_ticks, _) = timed_walk(solo, solo_mover);

    assert!(
        crowded_ticks >= solo_ticks,
        "collision separation made the crowded route faster ({crowded_ticks} vs {solo_ticks})"
    );
    let visual_limit = UnitKind::Harvester.stats().speed * chassis::fx::Fx::lit("1.3");
    assert!(
        max_step <= visual_limit,
        "one collision frame moved {max_step:?}, above the 30% visual-speed allowance {visual_limit:?}"
    );
}

#[test]
fn queued_orders_execute_in_sequence() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let (a, b) = (TilePos::new(9, 6), TilePos::new(9, 2));
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: a,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: b,
                queue: true,
            },
        ),
    ]);
    // First leg: arrive at A while B still waits in the queue.
    run_until(&mut state, 300, |s, _| s.unit(mover).unwrap().tile() == a);
    assert_eq!(state.unit(mover).unwrap().queue.len(), 1);
    // Second leg: end idle at B with nothing queued.
    run_until(&mut state, 300, |s, _| {
        let u = s.unit(mover).unwrap();
        u.tile() == b && u.order == Order::Idle && u.queue.is_empty()
    });
}

#[test]
fn queued_advance_executes_after_the_current_leg() {
    let mut state = open_arena(24, 12, vec![unit(0, UnitKind::Sentinel, 3, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let (first, advance_goal) = (TilePos::new(8, 6), TilePos::new(18, 6));
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: first,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Advance {
                units: vec![mover],
                goal: advance_goal,
                queue: true,
            },
        ),
    ]);
    assert_eq!(
        state.unit(mover).unwrap().queue.front(),
        Some(&Order::Advance { goal: advance_goal })
    );

    run_until(&mut state, 300, |state, _| {
        matches!(
            state.unit(mover).unwrap().order,
            Order::Advance { goal } if goal == advance_goal
        )
    });
    assert!(state.unit(mover).unwrap().queue.is_empty());
    run_until(&mut state, 400, |state, _| {
        let unit = state.unit(mover).unwrap();
        unit.tile() == advance_goal && unit.order == Order::Idle
    });
}

#[test]
fn direct_order_replaces_the_whole_queue() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: TilePos::new(13, 6),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: TilePos::new(13, 2),
                queue: true,
            },
        ),
    ]);
    // A fresh unqueued order wipes the program mid-walk.
    for _ in 0..20 {
        state.tick(&[]);
    }
    let d = TilePos::new(4, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal: d,
            queue: false,
        },
    )]);
    assert!(state.unit(mover).unwrap().queue.is_empty());
    run_until(&mut state, 300, |s, _| {
        let u = s.unit(mover).unwrap();
        u.tile() == d && u.order == Order::Idle
    });
}

#[test]
fn patrol_cycles_waypoints_and_never_settles() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let (a, b) = (TilePos::new(4, 6), TilePos::new(11, 6));
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![mover],
            waypoints: vec![a, b],
        },
    )]);
    // Expect the circuit to visit a, b, then a again — proof of the loop.
    let mut expected = [a, b, a].into_iter();
    let mut next = expected.next();
    for _ in 0..2000u32 {
        let u = state.unit(mover).unwrap();
        assert_ne!(u.order, Order::Idle, "a patrol never settles");
        if Some(u.tile()) == next {
            next = expected.next();
            if next.is_none() {
                break;
            }
        }
        state.tick(&[]);
    }
    assert_eq!(next, None, "circuit did not complete a full loop");
    assert!(state.unit(mover).unwrap().looping);
}

#[test]
fn patrol_engages_on_the_way_and_resumes_the_circuit() {
    // A sentinel patrols the top lane; an enemy harvester sits just off
    // it, inside aggro. The patroller must break off, kill it, and pick
    // the circuit back up. Waypoints stay >5 tiles from the enemy Foundry
    // so the patrol never besieges it mid-test.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 3, 2),
        unit(1, UnitKind::Harvester, 5, 4),
    ])
    .build()
    .unwrap();
    let (patroller, victim) = (state.units()[0].id, state.units()[1].id);
    let (a, b) = (TilePos::new(3, 2), TilePos::new(8, 2));
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![patroller],
            waypoints: vec![a, b],
        },
    )]);
    run_until(&mut state, 600, |s, _| s.unit(victim).is_none());
    // Back on the beat: both waypoints get visited again after the kill.
    for goal in [b, a] {
        run_until(&mut state, 600, |s, _| {
            s.unit(patroller).unwrap().tile() == goal
        });
    }
    assert!(state.unit(patroller).unwrap().looping);
}

#[test]
fn stalled_leg_drops_the_whole_program() {
    // The queued second leg targets a sealed pocket: no route. The stall
    // must abandon the entire program, not limp to the next leg.
    let scenario = Scenario {
        name: "sealed-pocket".into(),
        seed: 42,
        map: vec![
            "##############".into(),
            "#1...........#".into(),
            "#....###.....#".into(),
            "#....#.#.....#".into(),
            "#....###.....#".into(),
            "#............#".into(),
            "#.........2..#".into(),
            "#............#".into(),
            "##############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![unit(0, UnitKind::Harvester, 3, 1)],
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let mover = state.units()[0].id;
    let reachable = TilePos::new(10, 1);
    let pocket = TilePos::new(6, 3);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: reachable,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: pocket,
                queue: true,
            },
        ),
    ]);
    let events = run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { unit, .. } if *unit == mover))
    });
    assert!(!events.is_empty());
    let u = state.unit(mover).unwrap();
    assert_eq!(u.order, Order::Idle);
    assert!(u.queue.is_empty() && !u.looping);
    assert_eq!(u.tile(), reachable, "stall happens after the first leg");
}

#[test]
fn stop_clears_a_patrol() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![mover],
            waypoints: vec![TilePos::new(4, 6), TilePos::new(11, 6)],
        },
    )]);
    for _ in 0..30 {
        state.tick(&[]);
    }
    state.tick(&[cmd(0, Command::Stop { units: vec![mover] })]);
    let u = state.unit(mover).unwrap();
    assert_eq!(u.order, Order::Idle);
    assert!(u.queue.is_empty() && !u.looping);
    let parked = u.tile();
    for _ in 0..60 {
        state.tick(&[]);
    }
    assert_eq!(state.unit(mover).unwrap().tile(), parked);
}
