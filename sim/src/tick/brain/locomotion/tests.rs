use super::auto_land_probe_due;
use crate::stats::{AUTO_LAND_IDLE_TICKS, AUTO_LAND_RETRY_TICKS};

#[test]
fn indexed_arrival_matches_live_neighbors_and_domains() {
    use crate::{Order, PlayerId, Scenario, UnitKind};
    use chassis::fx::{Fx, Vec2Fx};
    use chassis::grid::TilePos;
    let mut state = Scenario::skirmish().build().unwrap();
    state.units.clear();
    let goal = TilePos::new(15, 12);
    let mover = state.spawn_unit(PlayerId(0), UnitKind::Sentinel, goal.center());
    for y in -4..=4 {
        for x in -4..=4 {
            state.spawn_unit(
                PlayerId(0),
                if x % 2 == 0 {
                    UnitKind::Sentinel
                } else {
                    UnitKind::Condor
                },
                goal.center() + Vec2Fx::new(Fx::from_num(x), Fx::from_num(y)),
            );
        }
    }
    let mut index = super::super::super::spatial::UnitIndex::new();
    for offset in -12..=12 {
        state.unit_mut(mover).unwrap().pos =
            goal.center() + Vec2Fx::new(Fx::from_num(offset) / 4, Fx::lit("0.49"));
        index.rebuild(&state.units);
        // These facts can change during the brain pass without invalidating
        // the position index. Queries must still read them from the world.
        for phase in 0..4 {
            for (slot, unit) in state.units.iter_mut().enumerate().skip(1) {
                unit.landed = phase % 2 == 0 && unit.kind == UnitKind::Condor;
                unit.order = if (slot + phase) % 3 == 0 {
                    Order::Run { goal: goal.into() }
                } else {
                    Order::Idle
                };
                unit.drive_speed = if (slot + phase) % 5 == 0 {
                    Fx::ONE
                } else {
                    Fx::ZERO
                };
            }
            let unit = state.unit(mover).unwrap();
            let near_sq = crate::stats::ARRIVAL_NEAR * crate::stats::ARRIVAL_NEAR;
            let expected = unit.pos.dist_sq(goal.center()) <= near_sq
                && state.units.iter().any(|other| {
                    other.id != mover
                        && other.hp > 0
                        && other.domain() == unit.domain()
                        && other.path.is_none()
                        && other.drive_speed == Fx::ZERO
                        && other.order == Order::Idle
                        && other.pos.dist_sq(goal.center()) <= near_sq
                        && unit.pos.dist(other.pos)
                            <= unit.kind.stats().radius
                                + other.kind.stats().radius
                                + Fx::lit("0.05")
                });
            assert_eq!(
                super::touching_settled_arrival(&state, &index, mover, goal),
                expected
            );
        }
    }
}

#[test]
fn coasting_arrival_does_not_complete_a_neighbor_order() {
    use super::{touching_settled_arrival, walk};
    use crate::state::{Order, PathFollow};
    use chassis::fx::{Fx, Vec2Fx};
    use chassis::grid::TilePos;

    let mut state = crate::Scenario::skirmish().build().unwrap();
    state.units.truncate(2);
    let goal = TilePos::new(12, 8);
    for unit in &mut state.units {
        unit.kind = crate::UnitKind::Sentinel;
        unit.order = Order::Run { goal: goal.into() };
        unit.path = Some(PathFollow {
            final_point: None,
            goal,
            waypoints: vec![goal],
            next: 0,
        });
    }
    state.units[0].pos = Vec2Fx::new(Fx::lit("12.01"), Fx::lit("8.5"));
    state.units[0].drive_speed = state.units[0].kind.stats().speed;
    let gap = state.units[0].kind.stats().radius * 2 + Fx::lit("0.02");
    state.units[1].pos = state.units[0].pos - Vec2Fx::new(gap, Fx::ZERO);
    let leader = state.units[0].id;
    let follower = state.units[1].id;
    let mut events = Vec::new();

    let mut index = super::super::super::spatial::UnitIndex::new();
    index.rebuild(&state.units);
    let mut reach = super::super::super::reach::Reach::new(&state);
    walk(&mut state, &index, &mut reach, leader, &mut events);
    assert!(state.units[0].path.is_none());
    assert!(state.units[0].drive_speed > Fx::ZERO);
    assert_ne!(state.units[1].tile(), goal);
    assert!(!touching_settled_arrival(&state, &index, follower, goal));
    walk(&mut state, &index, &mut reach, follower, &mut events);
    assert_eq!(state.units[1].order, Order::Run { goal: goal.into() });

    state.units[0].drive_speed = Fx::ZERO;
    assert!(touching_settled_arrival(&state, &index, follower, goal));
    walk(&mut state, &index, &mut reach, follower, &mut events);
    assert_eq!(state.units[1].order, Order::Idle);
}

/// A sandbox over `map` holding one seat and the given units.
fn sandbox(map: &[&str], units: &[(crate::UnitKind, i32, i32)]) -> crate::State {
    crate::Scenario {
        mode: crate::scenario::ScenarioMode::Sandbox,
        name: "locomotion".into(),
        seed: 5,
        map: map.iter().map(|row| (*row).to_owned()).collect(),
        players: vec![crate::scenario::PlayerSpec {
            name: "p0".into(),
            faction: crate::Faction::Ferrous,
            team: None,
            scrap: 0,
            bot: false,
            bot_config: None,
        }],
        units: units
            .iter()
            .map(|&(kind, x, y)| crate::scenario::UnitSpec {
                player: 0,
                kind,
                x,
                y,
            })
            .collect(),
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("locomotion fixture builds")
}

#[test]
fn a_hovering_flier_sent_onto_a_raw_peak_stops_beside_it() {
    use crate::{Event, Order, StallReason, UnitKind};
    use chassis::grid::TilePos;
    let map = [
        "............",
        "............",
        "....^^^.....",
        "....^^^.....",
        "....^^^.....",
        "............",
        "............",
    ];
    let peak = TilePos::new(5, 3);
    for kind in [UnitKind::Wisp, UnitKind::Skyhook] {
        let mut state = sandbox(&map, &[(kind, 1, 1)]);
        let flier = state.units()[0].id;
        // Commands snap peaks away; a forged order may not.
        state.unit_mut(flier).unwrap().order = Order::Run { goal: peak.into() };
        let mut stalls = 0;
        for _ in 0..100 {
            let report = state.tick(&[]);
            stalls += report
                .events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Event::OrderStalled {
                            reason: StallReason::NoRoute,
                            ..
                        }
                    )
                })
                .count();
            if state.unit(flier).unwrap().order == Order::Idle {
                break;
            }
        }
        let unit = state.unit(flier).unwrap();
        assert_eq!(unit.order, Order::Idle, "{kind:?} never settled");
        assert_eq!(stalls, 1, "{kind:?} reports the shortfall once");
        assert_eq!(unit.tile(), TilePos::new(5, 1), "{kind:?}");
    }
}

#[test]
fn a_short_walk_ends_on_meeting_the_crowd_parked_at_its_endpoint() {
    use crate::{Event, Goal, Order, StallReason, UnitKind};
    use chassis::fx::{Fx, Vec2Fx};
    use chassis::grid::TilePos;
    // Rock fills columns 10-13, so (9, 1) is the reachable tile nearest
    // the target (12, 1).
    let map = ["..........####"; 3];
    let endpoint = TilePos::new(9, 1);
    let target = TilePos::new(12, 1);
    let home = TilePos::new(0, 0);
    // Four parked bodies run west from the endpoint in touching steps.
    // The walker touches only the last, more than ARRIVAL_NEAR out.
    let run = |goal: Goal, crowd: bool| {
        let bodies = if crowd { 4 } else { 0 };
        let units = vec![(UnitKind::Sentinel, 0, 1); bodies + 1];
        let mut state = sandbox(&map, &units);
        let step = UnitKind::Sentinel.stats().radius * 2;
        for (slot, unit) in state.units.iter_mut().enumerate() {
            let steps = if slot == bodies {
                4
            } else {
                i32::try_from(slot).unwrap()
            };
            unit.pos = endpoint.center() - Vec2Fx::new(step * Fx::from_num(steps), Fx::ZERO);
        }
        let walker = state.units[bodies].id;
        let walker_pos = state.units[bodies].pos;
        assert!(
            walker_pos.dist(endpoint.center()) > crate::stats::ARRIVAL_NEAR,
            "premise: the ordinary arrival wave cannot reach the walker"
        );
        let unit = state.unit_mut(walker).unwrap();
        unit.order = Order::Run { goal };
        unit.queue.push_back(Order::Run { goal: home.into() });
        let mut index = super::super::super::spatial::UnitIndex::new();
        index.rebuild(&state.units);
        let mut reach = super::super::super::reach::Reach::new(&state);
        let mut events = Vec::new();
        super::walk(&mut state, &index, &mut reach, walker, &mut events);
        let stalls = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::OrderStalled { unit, reason: StallReason::NoRoute, .. }
                        if *unit == walker
                )
            })
            .count();
        (state.unit(walker).unwrap().order, events.len(), stalls)
    };
    let mut short = Goal::at(target);
    short.endpoint = Some(endpoint);

    assert_eq!(
        run(short, true),
        (Order::Run { goal: home.into() }, 1, 1),
        "meeting the crowd ends the walk once, and the program carries on"
    );
    assert_eq!(
        run(Goal::at(endpoint), true),
        (
            Order::Run {
                goal: endpoint.into()
            },
            0,
            0
        ),
        "a reachable goal keeps its ordinary arrival reach"
    );
    assert_eq!(
        run(short, false),
        (Order::Run { goal: short }, 0, 0),
        "a walker touching nothing keeps walking"
    );
}

#[test]
fn auto_land_probes_fire_on_the_retry_cadence_not_every_tick() {
    assert!(!auto_land_probe_due(AUTO_LAND_IDLE_TICKS - 1));
    assert!(auto_land_probe_due(AUTO_LAND_IDLE_TICKS));
    for offset in 1..AUTO_LAND_RETRY_TICKS {
        assert!(
            !auto_land_probe_due(AUTO_LAND_IDLE_TICKS + offset),
            "a failed probe must wait out the retry period (offset {offset})"
        );
    }
    assert!(auto_land_probe_due(
        AUTO_LAND_IDLE_TICKS + AUTO_LAND_RETRY_TICKS
    ));
    assert!(
        auto_land_probe_due(u16::MAX),
        "a saturated orbiter degrades to every-tick probing, never to silence"
    );
}
