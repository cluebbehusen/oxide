//! The 0.15 Skyhook: boarding, riding, landing, stranding, and dying.

use crate::common;
use common::wide_open_map as open_map;
use common::{cmd, players, unit};
use oxide_sim::scenario::ScenarioMode;

use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::UnitSpec;
use oxide_sim::state::Order;
use oxide_sim::{Command, Event, Faction, Scenario, State, Target, UnitId, UnitKind};

fn arena(map: Vec<String>, units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        mode: ScenarioMode::Match,
        name: "sling-arena".into(),
        seed: 13,
        map,
        players: players(500),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

#[test]
fn machines_board_ride_and_land() {
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 4, 4),
            unit(0, UnitKind::Sentinel, 3, 3),
            unit(0, UnitKind::Sentinel, 5, 3),
            unit(0, UnitKind::Lancer, 3, 5),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let riders: Vec<_> = state.units()[1..].iter().map(|u| u.id).collect();
    // Adjacent riders can embark on the command tick itself, so its
    // report must be counted too.
    let report = state.tick(&[cmd(
        0,
        Command::Load {
            units: riders,
            transport: sky,
            queue: false,
        },
    )]);
    let mut boarded = report
        .events
        .iter()
        .filter(|e| matches!(e, Event::UnitBoarded { .. }))
        .count();
    for _ in 0..200 {
        let report = state.tick(&[]);
        boarded += report
            .events
            .iter()
            .filter(|e| matches!(e, Event::UnitBoarded { .. }))
            .count();
        if boarded == 3 {
            break;
        }
    }
    assert_eq!(boarded, 3, "the whole squad boards");
    assert_eq!(state.units().len(), 1, "riders leave the world's unit list");

    // Ride across the arena and set down.
    state.tick(&[cmd(
        0,
        Command::Unload {
            transport: sky,
            at: TilePos::new(19, 4),
            queue: false,
        },
    )]);
    let mut landed = Vec::new();
    for _ in 0..400 {
        let report = state.tick(&[]);
        for event in &report.events {
            if let Event::UnitUnloaded { unit, at, .. } = event {
                landed.push((*unit, *at));
            }
        }
        if landed.len() == 3 {
            break;
        }
    }
    assert_eq!(landed.len(), 3, "the whole squad lands");
    let mut spots: Vec<TilePos> = landed.iter().map(|(_, at)| *at).collect();
    spots.sort_by_key(|t| (t.y, t.x));
    spots.dedup();
    assert_eq!(spots.len(), 3, "each rider gets its own tile");
    for (id, at) in &landed {
        let back = state.unit(*id).expect("rider stands in the world again");
        assert_eq!(back.order, Order::Idle);
        assert!(
            at.chebyshev(TilePos::new(19, 4)) <= 4,
            "landed inside the unload scan"
        );
    }
    assert!(
        state.unit(sky).unwrap().order == Order::Idle,
        "the sling is spent"
    );
}

#[test]
fn a_full_sling_stalls_the_straggler() {
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 4, 4),
            unit(0, UnitKind::Breaker, 3, 4),
            unit(0, UnitKind::Sentinel, 5, 4),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let (heavy, straggler) = (state.units()[1].id, state.units()[2].id);
    state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![heavy, straggler],
            transport: sky,
            queue: false,
        },
    )]);
    let mut stalled = false;
    for _ in 0..200 {
        let report = state.tick(&[]);
        stalled |= report.events.iter().any(|e| {
            matches!(
                e,
                Event::OrderStalled {
                    unit,
                    reason: oxide_sim::event::StallReason::TransportFull,
                    ..
                } if *unit == straggler
            )
        });
        if stalled {
            break;
        }
    }
    assert!(stalled, "the sentinel finds the sling full and stands down");
    assert!(
        state.unit(heavy).is_none(),
        "the breaker took the whole hold"
    );
    assert!(
        state.unit(straggler).is_some(),
        "the straggler stays in the world"
    );
}

#[test]
fn cargo_dies_with_the_airframe() {
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 4, 4),
            unit(0, UnitKind::Sentinel, 3, 4),
            unit(1, UnitKind::Stinger, 8, 4),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let rider = state.units()[1].id;
    let hunter = state.units()[2].id;
    state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![rider],
            transport: sky,
            queue: false,
        },
    )]);
    for _ in 0..100 {
        state.tick(&[]);
        if state.unit(rider).is_none() {
            break;
        }
    }
    assert!(state.unit(rider).is_none(), "premise: the rider is aboard");
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![hunter],
            target: Target::Unit(sky).into(),
            queue: false,
        },
    )]);
    let mut deaths = Vec::new();
    for _ in 0..2_000 {
        let report = state.tick(&[]);
        for event in &report.events {
            if let Event::UnitDied { unit, .. } = event {
                deaths.push(*unit);
            }
        }
        if deaths.contains(&sky) {
            break;
        }
    }
    assert!(deaths.contains(&sky), "the airframe falls");
    assert!(
        deaths.contains(&rider),
        "the rider dies with it: {deaths:?}"
    );
    let crash = state.map().wreck_at(TilePos::new(4, 4))
        + state.map().wreck_at(TilePos::new(5, 4))
        + state.map().wreck_at(TilePos::new(3, 4))
        + state.map().wreck_at(TilePos::new(6, 4))
        + state.map().wreck_at(TilePos::new(7, 4))
        + state.map().wreck_at(TilePos::new(8, 4));
    assert!(
        crash > 0,
        "both prices fall as wreck salvage near the crash"
    );
}

#[test]
fn the_sling_refuses_flyers_and_itself() {
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 4, 4),
            unit(0, UnitKind::Kestrel, 5, 4),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let scout = state.units()[1].id;
    let report = state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![scout],
            transport: sky,
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
        "a flyer cannot be carried"
    );
    let report = state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![sky],
            transport: sky,
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
        "a sling cannot carry itself"
    );
}

#[test]
fn unload_over_the_pit_strands_the_cargo_until_open_ground() {
    // An 11-wide pit expanse: the radius-4 unload scan from its center
    // finds no ground at all.
    let map = vec![
        "########################".into(),
        "#1.....................#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.....#".into(),
        "#......~~~~~~~~~~~.2...#".into(),
        "#......................#".into(),
        "########################".into(),
    ];
    let mut state = arena(
        map,
        vec![
            unit(0, UnitKind::Skyhook, 3, 4),
            unit(0, UnitKind::Sentinel, 3, 5),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let rider = state.units()[1].id;
    state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![rider],
            transport: sky,
            queue: false,
        },
    )]);
    for _ in 0..100 {
        state.tick(&[]);
        if state.unit(rider).is_none() {
            break;
        }
    }
    assert!(state.unit(rider).is_none(), "premise: the rider is aboard");

    // Drop point dead center over the pit: nothing can land.
    state.tick(&[cmd(
        0,
        Command::Unload {
            transport: sky,
            at: TilePos::new(12, 6),
            queue: false,
        },
    )]);
    let mut stranded = false;
    for _ in 0..400 {
        let report = state.tick(&[]);
        stranded |= report.events.iter().any(|e| {
            matches!(
                e,
                Event::OrderStalled {
                    reason: oxide_sim::event::StallReason::NoOpenGround,
                    ..
                }
            )
        });
        if stranded {
            break;
        }
    }
    assert!(stranded, "the pit refuses the drop");
    assert!(state.unit(rider).is_none(), "the rider stays aboard");

    // Fly back over ground and the drop completes.
    state.tick(&[cmd(
        0,
        Command::Unload {
            transport: sky,
            at: TilePos::new(3, 4),
            queue: false,
        },
    )]);
    for _ in 0..400 {
        state.tick(&[]);
        if state.unit(rider).is_some() {
            return;
        }
    }
    panic!("the rider never landed on open ground");
}

#[test]
fn a_boarder_stands_down_when_the_sling_has_no_ground_route() {
    let map = vec![
        "########################".into(),
        "#1.....................#".into(),
        "#......................#".into(),
        "#..........###.........#".into(),
        "#..........#.#.........#".into(),
        "#..........###.........#".into(),
        "#......................#".into(),
        "#...................2..#".into(),
        "#......................#".into(),
        "########################".into(),
    ];
    let mut state = arena(
        map,
        vec![
            unit(0, UnitKind::Skyhook, 12, 4),
            unit(0, UnitKind::Sentinel, 8, 4),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let rider = state.units()[1].id;

    let report = state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![rider],
            transport: sky,
            queue: false,
        },
    )]);

    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::OrderStalled {
            unit,
            reason: oxide_sim::event::StallReason::NoRoute,
            ..
        } if *unit == rider
    )));
    assert_eq!(state.unit(rider).unwrap().order, Order::Idle);
    assert!(state.unit(sky).unwrap().cargo.is_empty());
}

#[test]
fn a_boarder_walks_to_reachable_ground_beside_a_sling_over_a_building() {
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 2, 2),
            unit(0, UnitKind::Sentinel, 7, 2),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let rider = state.units()[1].id;
    assert!(
        !state.passable(state.unit(sky).unwrap().tile()),
        "the transport deliberately hovers over its Foundry"
    );

    let first = state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![rider],
            transport: sky,
            queue: false,
        },
    )]);
    assert!(first.events.iter().all(|event| !matches!(
        event,
        Event::OrderStalled {
            unit,
            reason: oxide_sim::event::StallReason::NoRoute,
            ..
        } if *unit == rider
    )));

    for _ in 0..200 {
        let report = state.tick(&[]);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            Event::OrderStalled {
                unit,
                reason: oxide_sim::event::StallReason::NoRoute,
                ..
            } if *unit == rider
        )));
        if state.unit(rider).is_none() {
            assert_eq!(state.unit(sky).unwrap().cargo.len(), 1);
            return;
        }
    }
    panic!("the rider never reached a valid boarding tile");
}

#[test]
fn a_sling_sealed_in_by_peaks_unloads_as_close_as_it_can_get() {
    let map = vec![
        "########################".into(),
        "#1.....................#".into(),
        "#.........^^^^^........#".into(),
        "#.........^...^........#".into(),
        "#.........^...^........#".into(),
        "#.........^...^........#".into(),
        "#.........^^^^^........#".into(),
        "#...................2..#".into(),
        "#......................#".into(),
        "########################".into(),
    ];
    let mut state = arena(
        map,
        vec![
            unit(0, UnitKind::Skyhook, 12, 4),
            unit(0, UnitKind::Sentinel, 11, 4),
        ],
    )
    .build()
    .unwrap();
    let sky = state.units()[0].id;
    let rider = state.units()[1].id;
    state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![rider],
            transport: sky,
            queue: false,
        },
    )]);
    assert!(state.unit(rider).is_none(), "premise: the rider is aboard");

    let mut events = state
        .tick(&[cmd(
            0,
            Command::Unload {
                transport: sky,
                at: TilePos::new(18, 4),
                queue: false,
            },
        )])
        .events;
    for _ in 0..100 {
        if state.unit(sky).unwrap().cargo.is_empty() {
            break;
        }
        events.extend(state.tick(&[]).events);
    }
    let no_routes = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                Event::OrderStalled {
                    unit,
                    reason: oxide_sim::event::StallReason::NoRoute,
                    ..
                } if *unit == sky
            )
        })
        .count();
    assert_eq!(no_routes, 1, "the shortfall is reported once");
    let transport = state.unit(sky).expect("the airframe survives");
    assert_eq!(transport.order, Order::Idle);
    assert!(transport.cargo.is_empty(), "the riders are set down");
    assert_eq!(
        transport.tile(),
        TilePos::new(13, 4),
        "the flight ends inside the ring, nearest the drop point"
    );
    let dropped = state.unit(rider).expect("the rider is back in the world");
    let tile = dropped.tile();
    assert!(
        (11..=13).contains(&tile.x) && (3..=5).contains(&tile.y),
        "the rider lands inside the ring: {tile:?}"
    );
}

#[test]
fn a_boarding_walk_that_cannot_be_routed_yields_to_the_queued_move() {
    let map = vec![
        "########################".into(),
        "#1.........#...........#".into(),
        "#..........#...........#".into(),
        "#..........#...........#".into(),
        "#..........#...........#".into(),
        "#..........#.......2...#".into(),
        "#..........#...........#".into(),
        "########################".into(),
    ];
    let mut scenario = arena(
        map,
        vec![
            unit(0, UnitKind::Skyhook, 16, 3),
            unit(0, UnitKind::Sentinel, 4, 3),
        ],
    );
    scenario.mode = oxide_sim::scenario::ScenarioMode::Sandbox;
    let mut state = scenario.build().unwrap();
    let sky = state.units()[0].id;
    let rider = state.units()[1].id;
    let home = TilePos::new(2, 5);
    let mut events = state
        .tick(&[
            cmd(
                0,
                Command::Load {
                    units: vec![rider],
                    transport: sky,
                    queue: false,
                },
            ),
            cmd(
                0,
                Command::Run {
                    units: vec![rider],
                    goal: home,
                    queue: true,
                },
            ),
        ])
        .events;
    for _ in 0..300 {
        let walker = state.unit(rider).expect("the rider never boards");
        if walker.order == Order::Idle && walker.tile() == home {
            break;
        }
        events.extend(state.tick(&[]).events);
    }
    let walker = state.unit(rider).unwrap();
    assert_eq!(walker.order, Order::Idle);
    assert_eq!(walker.tile(), home, "the queued move ran");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                Event::OrderStalled {
                    unit,
                    reason: oxide_sim::event::StallReason::NoRoute,
                    ..
                } if *unit == rider
            ))
            .count(),
        1
    );
}

#[test]
fn a_rider_survives_when_its_sling_is_destroyed_during_boarding() {
    let mut scenario = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 10, 4),
            unit(0, UnitKind::Sentinel, 9, 4),
        ],
    );
    scenario.players[1].faction = Faction::Ferrous;
    scenario.units.extend(
        [(8, 2), (10, 2), (12, 2), (8, 4), (12, 4), (9, 6), (11, 6)]
            .into_iter()
            .map(|(x, y)| unit(1, UnitKind::Shrike, x, y)),
    );
    let mut state = scenario.build().unwrap();
    let sky = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook)
        .unwrap()
        .id;
    let rider = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Sentinel)
        .unwrap()
        .id;
    let hunters: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.kind == UnitKind::Shrike)
        .map(|unit| unit.id)
        .collect();

    for &hunter in &hunters {
        common::face_target(&mut state, hunter, Target::Unit(sky));
    }

    let report = state.tick(&[
        cmd(
            0,
            Command::Load {
                units: vec![rider],
                transport: sky,
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Attack {
                units: hunters,
                target: Target::Unit(sky).into(),
                queue: false,
            },
        ),
    ]);
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::UnitDied { unit, .. } if *unit == sky))
    );
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::UnitBoarded { unit, .. } if *unit == rider)),
        "a lethal same-tick volley wins over the buffered embarkation"
    );
    assert!(
        state.unit(rider).is_some(),
        "the waiting rider stays in the world"
    );

    state.tick(&[]);
    assert_eq!(
        state.unit(rider).unwrap().order,
        Order::Idle,
        "the missing transport releases the rider on its next decision"
    );
}

#[test]
fn a_lethally_hit_rider_is_not_entombed_as_cargo() {
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 10, 4),
            unit(0, UnitKind::Sentinel, 9, 4),
            unit(1, UnitKind::Lancer, 8, 2),
            unit(1, UnitKind::Lancer, 10, 2),
        ],
    )
    .build()
    .unwrap();
    let sky = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook)
        .unwrap()
        .id;
    let rider = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Sentinel)
        .unwrap()
        .id;
    let attackers: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.kind == UnitKind::Lancer)
        .map(|unit| unit.id)
        .collect();

    for &attacker in &attackers {
        common::face_target(&mut state, attacker, Target::Unit(rider));
    }
    let report = state.tick(&[
        cmd(
            0,
            Command::Load {
                units: vec![rider],
                transport: sky,
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Attack {
                units: attackers,
                target: Target::Unit(rider).into(),
                queue: false,
            },
        ),
    ]);

    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::UnitDied { unit, .. } if *unit == rider))
    );
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::UnitBoarded { unit, .. } if *unit == rider))
    );
    assert!(state.unit(rider).is_none());
    assert!(
        state.unit(sky).unwrap().cargo.is_empty(),
        "the death pass must still own a rider killed during buffered boarding"
    );
}

#[test]
fn a_rider_that_stalled_on_its_way_boards_dormant() {
    // The sling clears every live field of a boarding rider; the stall
    // counter is one of them, or cargo would fail the dormancy invariant
    // and a save taken mid-flight could never load.
    let mut state = arena(
        open_map(),
        vec![
            unit(0, UnitKind::Skyhook, 4, 4),
            unit(0, UnitKind::Sentinel, 10, 4),
        ],
    )
    .build()
    .unwrap();
    let (sky, rider) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![rider],
            transport: sky,
            queue: false,
        },
    )]);
    let slot = state
        .units()
        .iter()
        .position(|unit| unit.id == rider)
        .unwrap();
    assert!(state.units()[slot].path.is_some(), "walking to the sling");
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["units"][slot]["stall_ticks"] = serde_json::json!(1);
    let mut state: oxide_sim::State = serde_json::from_value(doc).unwrap();
    let mut boarded = false;
    for _ in 0..300 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::UnitBoarded { .. }))
        {
            boarded = true;
            break;
        }
    }
    assert!(boarded, "the rider never boarded");
    state.validate_invariants().expect("cargo is dormant");
    let carrier = state.unit(sky).unwrap();
    assert_eq!(carrier.cargo.len(), 1);
    assert_eq!(carrier.cargo[0].stall_ticks, 0);
}

#[test]
fn mirrored_boarders_take_mirrored_tiles() {
    // Several boarding tiles tie on route length for each rider below; the
    // rock beside the southern sling leaves two that also tie on distance to
    // both the rider and the carrier. The east seat is the half-turn of the
    // west, so every choice must be the half-turn of its partner's.
    let (width, height) = (40, 14);
    let rocks = [TilePos::new(7, 9), TilePos::new(32, 4)];
    let map = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| {
                    let border = x == 0 || y == 0 || x == width - 1 || y == height - 1;
                    if border || rocks.contains(&TilePos::new(x, y)) {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect();
    let mirror = |tile: TilePos| TilePos::new(width - 1 - tile.x, height - 1 - tile.y);
    let mirror_pos =
        |pos: Vec2Fx| Vec2Fx::new(Fx::from_num(width) - pos.x, Fx::from_num(height) - pos.y);
    let west = [
        (UnitKind::Skyhook, TilePos::new(8, 3)),
        (UnitKind::Sentinel, TilePos::new(4, 2)),
        (UnitKind::Sentinel, TilePos::new(4, 3)),
        (UnitKind::Skyhook, TilePos::new(8, 9)),
        (UnitKind::Sentinel, TilePos::new(4, 9)),
    ];
    let units = west
        .iter()
        .flat_map(|&(kind, at)| {
            let away = mirror(at);
            [unit(0, kind, at.x, at.y), unit(1, kind, away.x, away.y)]
        })
        .collect();
    let mut scenario = arena(map, units);
    scenario.mode = oxide_sim::scenario::ScenarioMode::Sandbox;
    let mut state = scenario.build().unwrap();
    let pairs: Vec<(UnitId, UnitId)> = state
        .units()
        .chunks(2)
        .map(|pair| (pair[0].id, pair[1].id))
        .collect();
    let load = |side: fn(&(UnitId, UnitId)) -> UnitId, player: u8| {
        [(0, vec![1, 2]), (3, vec![4])].map(|(sling, riders)| {
            cmd(
                player,
                Command::Load {
                    units: riders.iter().map(|&i| side(&pairs[i])).collect(),
                    transport: side(&pairs[sling]),
                    queue: false,
                },
            )
        })
    };
    let commands: Vec<_> = load(|pair| pair.0, 0)
        .into_iter()
        .chain(load(|pair| pair.1, 1))
        .collect();
    let symmetric = |state: &State, events: &[Event], tick: usize| {
        let boarded: Vec<UnitId> = events
            .iter()
            .filter_map(|event| match event {
                Event::UnitBoarded { unit, .. } => Some(*unit),
                _ => None,
            })
            .collect();
        for &(w, e) in &pairs {
            assert_eq!(
                boarded.contains(&w),
                boarded.contains(&e),
                "tick {tick}: {w:?} and {e:?} board together"
            );
            match (state.unit(w), state.unit(e)) {
                (Some(a), Some(b)) => {
                    assert_eq!(
                        a.path.as_ref().map(|path| mirror(path.goal)),
                        b.path.as_ref().map(|path| path.goal),
                        "tick {tick}: {w:?} boarding tile"
                    );
                    assert_eq!(mirror_pos(a.pos), b.pos, "tick {tick}: {w:?} position");
                }
                (None, None) => {}
                _ => panic!("tick {tick}: only one of {w:?} and {e:?} boarded"),
            }
        }
    };

    let report = state.tick(&commands);
    symmetric(&state, &report.events, 0);
    let goals = [1, 2, 4].map(|i| state.unit(pairs[i].0).unwrap().path.as_ref().unwrap().goal);
    assert_eq!(
        goals,
        [TilePos::new(7, 2), TilePos::new(7, 3), TilePos::new(7, 8)],
        "nearest tile first; the tie beside the rock falls to one side of the approach"
    );
    for tick in 1..=200 {
        if state.units().len() == 4 {
            break;
        }
        let report = state.tick(&[]);
        symmetric(&state, &report.events, tick);
    }
    assert_eq!(
        state.units().len(),
        4,
        "only the slings remain in the world"
    );
}
