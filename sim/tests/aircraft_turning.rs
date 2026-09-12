//! Cruise steering, hovering arrivals, and fixed-gun alignment.

mod common;

use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;
use common::{cmd, open_arena, open_arena_with, unit};
use oxide_sim::{Command, Event, Order, State, Target, UnitKind};

const CRUISERS: [UnitKind; 6] = [
    UnitKind::Talon,
    UnitKind::Darter,
    UnitKind::Shrike,
    UnitKind::Sylph,
    UnitKind::Kestrel,
    UnitKind::Gnat,
];

fn facing(state: State, heading: u8) -> State {
    let mut snapshot = serde_json::to_value(state).unwrap();
    snapshot["units"][0]["heading"] = serde_json::json!(heading);
    serde_json::from_value(snapshot).unwrap()
}

fn move_to(state: &mut State, goal: TilePos, queue: bool) {
    let unit = &state.units()[0];
    state.tick(&[cmd(
        unit.player.0,
        Command::Move {
            units: vec![unit.id],
            goal,
            queue,
        },
    )]);
}

#[test]
fn cruising_aircraft_bank_on_reversal_and_hover_on_arrival() {
    for kind in CRUISERS {
        let mut state = facing(
            open_arena(80, 52, vec![unit(0, kind, 20, 26)])
                .build()
                .unwrap(),
            0,
        );
        move_to(&mut state, TilePos::new(60, 26), false);
        for _ in 0..9 {
            state.tick(&[]);
        }
        let before = state.units()[0].clone();
        move_to(&mut state, TilePos::new(10, 26), false);
        let delta = state.units()[0].pos - before.pos;
        assert!(delta.x > Fx::ZERO && delta.y != Fx::ZERO, "{kind:?}");
        for _ in 0..1000 {
            let before = state.units()[0].clone();
            state.tick(&[]);
            let after = &state.units()[0];
            assert!(
                after
                    .heading
                    .wrapping_sub(before.heading)
                    .cast_signed()
                    .unsigned_abs()
                    <= kind.cruise_turn_rate(),
                "{kind:?}"
            );
            assert!(after.pos.dist(before.pos) <= kind.stats().speed + Fx::DELTA * 16);
            assert!(!after.landed);
            if after.order == Order::Idle {
                break;
            }
        }
        let arrived = state.units()[0].clone();
        assert_eq!(arrived.order, Order::Idle, "{kind:?}");
        assert_eq!(arrived.tile(), TilePos::new(10, 26));
        for _ in 0..400 {
            state.tick(&[]);
            assert_eq!(state.units()[0].pos, arrived.pos);
            assert_eq!(state.units()[0].heading, arrived.heading);
            assert!(!state.units()[0].landed);
        }
    }
}

#[test]
fn nearby_and_queued_goals_converge_in_every_direction() {
    for kind in CRUISERS {
        for heading in (0..=255).step_by(32) {
            for (x, y) in [(21, 26), (19, 26), (20, 25), (21, 27)] {
                let mut state = facing(
                    open_arena(80, 52, vec![unit(0, kind, 20, 26)])
                        .build()
                        .unwrap(),
                    heading,
                );
                move_to(&mut state, TilePos::new(x, y), false);
                move_to(&mut state, TilePos::new(25, 30), true);
                for _ in 0..600 {
                    state.tick(&[]);
                    state.validate_invariants().unwrap();
                    if state.units()[0].order == Order::Idle {
                        break;
                    }
                }
                assert_eq!(state.units()[0].order, Order::Idle, "{kind:?} {heading}");
                assert_eq!(state.units()[0].tile(), TilePos::new(25, 30));
            }
        }
    }
}

#[test]
fn cruising_aircraft_recover_from_corners_and_route_around_peaks() {
    for kind in CRUISERS {
        for (x, y, heading, goal) in [
            (0, 0, 160, (25, 25)),
            (39, 0, 224, (15, 25)),
            (0, 39, 96, (25, 15)),
            (39, 39, 32, (15, 15)),
            (15, 20, 0, (25, 20)),
            (25, 20, 128, (15, 20)),
        ] {
            let mut state = facing(
                open_arena_with(40, 40, vec![unit(0, kind, x, y)], |rows| {
                    for row in rows.iter_mut().take(26).skip(14) {
                        row[20] = '^';
                    }
                })
                .build()
                .unwrap(),
                heading,
            );
            move_to(&mut state, TilePos::new(goal.0, goal.1), false);
            for _ in 0..1600 {
                state.tick(&[]);
                state.validate_invariants().unwrap();
                assert!(
                    !state
                        .map()
                        .tile(state.units()[0].tile())
                        .unwrap()
                        .terrain
                        .blocks_air()
                );
                if state.units()[0].order == Order::Idle {
                    break;
                }
            }
            assert_eq!(
                state.units()[0].tile(),
                TilePos::new(goal.0, goal.1),
                "{kind:?} {x},{y}"
            );
            assert_eq!(state.units()[0].order, Order::Idle);
        }
    }
}

#[test]
fn cruising_fighters_turn_to_targets_without_becoming_bombers() {
    for kind in [
        UnitKind::Talon,
        UnitKind::Darter,
        UnitKind::Shrike,
        UnitKind::Sylph,
    ] {
        let victim = if kind == UnitKind::Darter {
            UnitKind::Harvester
        } else {
            UnitKind::Skyhook
        };
        let mut state = facing(
            open_arena(
                80,
                52,
                vec![
                    unit(0, kind, 25, 26),
                    unit(1, victim, 27, 26),
                    unit(1, victim, 23, 26),
                ],
            )
            .build()
            .unwrap(),
            128,
        );
        let id = state.units()[0].id;
        let position = state.units()[0].pos;
        for target_slot in [1, 2] {
            let target = state.units()[target_slot].id;
            let mut fired = false;
            for tick in 0..100 {
                let commands = if tick == 0 {
                    vec![cmd(
                        0,
                        Command::Attack {
                            units: vec![id],
                            target: Target::Unit(target),
                            queue: false,
                        },
                    )]
                } else {
                    vec![]
                };
                let before = state.unit(id).unwrap().heading;
                let report = state.tick(&commands);
                let shooter = state.unit(id).unwrap();
                assert_eq!(shooter.pos, position);
                assert!(
                    shooter
                        .heading
                        .wrapping_sub(before)
                        .cast_signed()
                        .unsigned_abs()
                        <= kind.cruise_turn_rate()
                );
                assert!(state.shells().is_empty());
                if report.events.iter().any(
                    |event| matches!(event, Event::AttackHit { attacker, .. } if *attacker == id),
                ) {
                    assert!(tick > 0);
                    let wanted = if target_slot == 1 { 0u8 } else { 128u8 };
                    assert!(
                        shooter
                            .heading
                            .wrapping_sub(wanted)
                            .cast_signed()
                            .unsigned_abs()
                            <= 2
                    );
                    fired = true;
                    break;
                }
            }
            assert!(fired, "{kind:?}");
        }
    }
}

#[test]
fn cruise_turns_resume_identically_and_mirror_exactly() {
    for kind in CRUISERS {
        let mut a = facing(
            open_arena(80, 52, vec![unit(0, kind, 20, 26)])
                .build()
                .unwrap(),
            0,
        );
        let mut b = facing(
            open_arena(80, 52, vec![unit(1, kind, 59, 25)])
                .build()
                .unwrap(),
            128,
        );
        move_to(&mut a, TilePos::new(10, 12), false);
        move_to(&mut b, TilePos::new(69, 39), false);
        let mut restored: State = serde_json::from_slice(&serde_json::to_vec(&a).unwrap()).unwrap();
        for _ in 0..400 {
            let report = a.tick(&[]);
            b.tick(&[]);
            assert_eq!(restored.tick(&[]).events, report.events);
            assert_eq!(a.hash(), restored.hash());
            assert_eq!(
                a.units()[0].pos + b.units()[0].pos,
                Vec2Fx::new(Fx::from_num(80), Fx::from_num(52)),
                "{kind:?}"
            );
            assert_eq!(a.units()[0].heading.wrapping_add(128), b.units()[0].heading);
        }
    }
}

#[test]
fn advance_keeps_its_course_and_only_fires_forward() {
    for kind in [
        UnitKind::Talon,
        UnitKind::Darter,
        UnitKind::Shrike,
        UnitKind::Sylph,
    ] {
        let victim = if kind == UnitKind::Darter {
            UnitKind::Harvester
        } else {
            UnitKind::Skyhook
        };
        for target_x in [23, 27] {
            let mut state = facing(
                open_arena(
                    80,
                    52,
                    vec![unit(0, kind, 25, 26), unit(1, victim, target_x, 26)],
                )
                .build()
                .unwrap(),
                0,
            );
            let id = state.units()[0].id;
            let start = state.units()[0].pos;
            let report = state.tick(&[cmd(
                0,
                Command::Advance {
                    units: vec![id],
                    goal: TilePos::new(60, 26),
                    queue: false,
                },
            )]);
            let fired = report
                .events
                .iter()
                .any(|event| matches!(event, Event::AttackHit { attacker, .. } if *attacker == id));
            assert_eq!(fired, target_x == 27, "{kind:?}");
            assert_eq!(state.units()[0].heading, 0);
            assert!(state.units()[0].pos.x > start.x);
            assert_eq!(state.units()[0].pos.y, start.y);
            assert!(matches!(state.units()[0].order, Order::Advance { .. }));
            assert!(state.shells().is_empty());
        }
    }
}

#[test]
fn stop_interrupts_a_banked_patrol_and_holds_position() {
    for kind in CRUISERS {
        let mut state = facing(
            open_arena(80, 52, vec![unit(0, kind, 20, 26)])
                .build()
                .unwrap(),
            0,
        );
        let id = state.units()[0].id;
        state.tick(&[cmd(
            0,
            Command::Patrol {
                units: vec![id],
                waypoints: vec![TilePos::new(30, 26), TilePos::new(20, 26)],
            },
        )]);
        let mut crossed = false;
        for _ in 0..300 {
            state.tick(&[]);
            crossed |= state.units()[0].pos.x >= Fx::from_num(30);
        }
        assert!(crossed);
        assert!(state.units()[0].looping);
        let before = state.units()[0].pos;
        state.tick(&[cmd(0, Command::Stop { units: vec![id] })]);
        assert_eq!(state.units()[0].pos, before);
        for _ in 0..100 {
            state.tick(&[]);
            assert_eq!(state.units()[0].pos, before);
            assert!(!state.units()[0].landed);
        }
    }
}

#[test]
fn rotorcraft_retain_independent_travel_and_bombers_remain_committed() {
    for kind in [
        UnitKind::Buzzard,
        UnitKind::Wisp,
        UnitKind::Skyhook,
        UnitKind::Condor,
        UnitKind::Moth,
    ] {
        let mut state = facing(
            open_arena(80, 52, vec![unit(0, kind, 20, 26)])
                .build()
                .unwrap(),
            0,
        );
        move_to(&mut state, TilePos::new(60, 26), false);
        let before = state.units()[0].pos;
        move_to(&mut state, TilePos::new(10, 26), false);
        let delta = state.units()[0].pos - before;
        assert_eq!(kind.cruise_turn_rate(), 0);
        if kind.stats().turn_rate == 0 {
            assert!(delta.x < Fx::ZERO);
            assert!((delta.x + kind.stats().speed).abs() <= Fx::DELTA * 16);
            assert_eq!(delta.y, Fx::ZERO);
        } else {
            assert!(delta.x > Fx::ZERO && delta.y != Fx::ZERO);
        }
    }
}
