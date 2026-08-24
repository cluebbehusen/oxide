//! The 0.15 Skyhook: boarding, riding, landing, stranding, and dying.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::state::Order;
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

fn arena(map: Vec<String>, units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "sling-arena".into(),
        seed: 13,
        map,
        players: players(500),
        units,
        buildings: Vec::new(),
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
            target: Target::Unit(sky),
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
fn a_loaded_sling_stands_down_when_peaks_seal_its_air_route() {
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

    let report = state.tick(&[cmd(
        0,
        Command::Unload {
            transport: sky,
            at: TilePos::new(18, 4),
            queue: false,
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::OrderStalled {
            unit,
            reason: oxide_sim::event::StallReason::NoRoute,
            ..
        } if *unit == sky
    )));
    let transport = state.unit(sky).expect("the airframe survives");
    assert_eq!(transport.order, Order::Idle);
    assert_eq!(transport.cargo.len(), 1, "a failed flight loses no cargo");
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
                target: Target::Unit(sky),
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
                target: Target::Unit(rider),
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
