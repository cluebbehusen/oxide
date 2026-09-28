//! Removing one order from unit programs. A cancellation names its order by
//! key and by how many later orders share that key, so legs that finish
//! while the command is in flight never change which order goes.

mod common;
use common::{arena, cmd, run_until, unit};

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{
    AttackTarget, Command, Event, Faction, Order, OrderKey, PlayerCommand, PlayerId, Scenario,
    State, Target, UnitId, UnitKind,
};

/// An open sandbox, `width` by `height`.
fn sandbox(width: usize, height: usize, units: &[(u8, UnitKind, i32, i32)]) -> State {
    Scenario {
        mode: ScenarioMode::Sandbox,
        name: "cancel-order".into(),
        seed: 5,
        map: vec![".".repeat(width); height],
        players: [Faction::Ferrous, Faction::Cupric]
            .into_iter()
            .enumerate()
            .map(|(seat, faction)| PlayerSpec {
                name: format!("seat {seat}"),
                faction,
                team: None,
                scrap: 1_000,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units: units
            .iter()
            .map(|&(player, kind, x, y)| UnitSpec { player, kind, x, y })
            .collect(),
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("the fixture builds")
}

fn ids(state: &State, player: u8) -> Vec<UnitId> {
    state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(player))
        .map(|unit| unit.id)
        .collect()
}

/// The keys of a unit's program, active order first.
fn keys(state: &State, id: UnitId) -> Vec<OrderKey> {
    let unit = state.unit(id).expect("the unit lives");
    std::iter::once(&unit.order)
        .chain(&unit.queue)
        .filter_map(|order| order.key(state, unit.player))
        .collect()
}

fn walk(tile: TilePos) -> OrderKey {
    OrderKey::Walk { tile }
}

fn walks(tiles: &[TilePos]) -> Vec<OrderKey> {
    tiles.iter().copied().map(walk).collect()
}

/// `goals` as one program for `units`: the first replaces, the rest queue.
fn program(units: &[UnitId], goals: &[TilePos]) -> Vec<PlayerCommand> {
    goals
        .iter()
        .enumerate()
        .map(|(leg, &goal)| {
            cmd(
                0,
                Command::Move {
                    units: units.to_vec(),
                    goal,
                    queue: leg > 0,
                },
            )
        })
        .collect()
}

fn cancel(unit: UnitId, key: OrderKey, from_end: u8, units: &[UnitId]) -> PlayerCommand {
    cmd(
        0,
        Command::CancelOrder {
            unit,
            key,
            from_end,
            units: units.to_vec(),
        },
    )
}

fn rejection(events: &[Event]) -> Option<RejectReason> {
    events.iter().find_map(|event| match event {
        Event::CommandRejected { reason, .. } => Some(*reason),
        _ => None,
    })
}

/// Ticks `state` with `command`, asserting the refusal and that the world
/// ends exactly where an empty tick leaves it.
fn assert_refused(state: &mut State, command: PlayerCommand, reason: RejectReason) {
    let mut control = state.clone();
    control.tick(&[]);
    let report = state.tick(&[command]);
    assert_eq!(rejection(&report.events), Some(reason));
    assert_eq!(state.hash(), control.hash(), "a refusal changes nothing");
}

#[test]
fn the_active_and_a_queued_order_come_out_of_the_program() {
    let mut state = sandbox(30, 10, &[(0, UnitKind::Sentinel, 3, 5)]);
    let id = ids(&state, 0)[0];
    let [a, b, c] = [(20, 2), (20, 8), (26, 5)].map(|(x, y)| TilePos::new(x, y));
    state.tick(&program(&[id], &[a, b, c]));
    state.tick(&[cancel(id, walk(b), 0, &[])]);
    assert_eq!(keys(&state, id), walks(&[a, c]), "a queued leg comes out");
    state.tick(&[cancel(id, walk(a), 0, &[])]);
    assert_eq!(keys(&state, id), walks(&[c]), "the active leg comes out");
    let walker = state.unit(id).unwrap();
    assert!(
        matches!(walker.order, Order::Move { goal } if goal.tile() == c),
        "the next leg starts at once"
    );
    state.tick(&[cancel(id, walk(c), 0, &[])]);
    assert_eq!(state.unit(id).unwrap().order, Order::Idle);
}

#[test]
fn a_patrol_cut_to_one_leg_keeps_looping_and_the_last_leg_idles_it() {
    let mut state = sandbox(30, 10, &[(0, UnitKind::Sentinel, 3, 5)]);
    let id = ids(&state, 0)[0];
    let (a, b) = (TilePos::new(6, 5), TilePos::new(24, 5));
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![id],
            waypoints: vec![a, b],
        },
    )]);
    state.tick(&[cancel(id, walk(b), 0, &[])]);
    assert_eq!(keys(&state, id), walks(&[a]));
    assert!(state.unit(id).unwrap().looping, "one leg still loops");
    run_until(&mut state, 200, |state, _| {
        state.unit(id).unwrap().tile().chebyshev(a) <= 1
    });
    common::run(&mut state, 40);
    assert_eq!(keys(&state, id), walks(&[a]), "the leg comes around again");
    assert!(state.unit(id).unwrap().looping);

    state.tick(&[cancel(id, walk(a), 0, &[])]);
    let unit = state.unit(id).unwrap();
    assert_eq!(unit.order, Order::Idle);
    assert!(!unit.looping, "an idle unit no longer loops");
}

#[test]
fn a_cancelled_harvest_keeps_its_cargo() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 6, 2)])
        .build()
        .unwrap();
    let worker = state.units()[0].id;
    let node = TilePos::new(11, 4);
    let home = TilePos::new(1, 1);
    state.tick(&[
        cmd(
            0,
            Command::Harvest {
                units: vec![worker],
                node,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![worker],
                goal: home,
                queue: true,
            },
        ),
    ]);
    run_until(&mut state, 400, |state, _| {
        state.unit(worker).unwrap().carrying > 0
    });
    let cargo = state.unit(worker).unwrap().carrying;
    state.tick(&[cancel(worker, OrderKey::Harvest { anchor: node }, 0, &[])]);
    let unit = state.unit(worker).unwrap();
    assert_eq!(keys(&state, worker), walks(&[home]));
    assert_eq!(unit.carrying, cargo, "the hopper stays full");
}

#[test]
fn cancelling_the_last_build_order_on_an_unstarted_site_refunds_it() {
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 6, 2)]);
    scenario.players[0].scrap = 1_000;
    let mut state = scenario.build().unwrap();
    let worker = state.units()[0].id;
    let anchor = TilePos::new(9, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![worker],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .expect("the site stands")
        .id;
    let paid = state.player(PlayerId(0)).scrap;
    assert_eq!(state.building(site).unwrap().progress, 0, "test premise");
    let report = state.tick(&[cancel(worker, OrderKey::Build { site }, 0, &[])]);
    let cost = BuildingKind::Turret
        .base_stats()
        .construction
        .expect("buildable")
        .cost;
    assert!(report.events.contains(&Event::BuildCancelled {
        building: site,
        player: PlayerId(0),
        refund: cost,
    }));
    assert!(state.building(site).is_none());
    assert_eq!(state.player(PlayerId(0)).scrap, paid + cost);
    assert_eq!(state.unit(worker).unwrap().order, Order::Idle);
}

#[test]
fn an_engagement_on_the_march_goes_with_its_leg() {
    let mut state = sandbox(
        40,
        12,
        &[
            (0, UnitKind::Sentinel, 3, 5),
            (0, UnitKind::Sentinel, 3, 6),
            (1, UnitKind::Harvester, 7, 8),
        ],
    );
    let group = ids(&state, 0);
    let (click, next) = (TilePos::new(12, 5), TilePos::new(20, 5));
    state.tick(&[
        cmd(
            0,
            Command::AttackMove {
                units: group.clone(),
                goal: click,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::AttackMove {
                units: group.clone(),
                goal: next,
                queue: true,
            },
        ),
    ]);
    run_until(&mut state, 300, |state, _| {
        group.iter().any(|&id| {
            matches!(
                state.unit(id).unwrap().order,
                Order::Attack {
                    resume: Some(_),
                    ..
                }
            )
        })
    });
    let fighter = *group
        .iter()
        .find(|&&id| matches!(state.unit(id).unwrap().order, Order::Attack { .. }))
        .unwrap();
    assert_eq!(keys(&state, fighter), walks(&[click, next]));
    state.tick(&[cancel(fighter, walk(click), 0, &group)]);
    for id in group {
        assert_eq!(
            keys(&state, id),
            walks(&[next]),
            "{id} dropped the leg, fighting or still marching"
        );
    }
}

#[test]
fn walks_match_by_click_whatever_their_verb() {
    let mut state = sandbox(
        30,
        10,
        &[
            (0, UnitKind::Sentinel, 3, 4),
            (0, UnitKind::Harvester, 3, 6),
        ],
    );
    let [fighter, worker] = ids(&state, 0)[..] else {
        unreachable!()
    };
    let (a, b) = (TilePos::new(20, 5), TilePos::new(25, 5));
    state.tick(&[
        cmd(
            0,
            Command::AttackMove {
                units: vec![fighter, worker],
                goal: a,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Advance {
                units: vec![fighter, worker],
                goal: b,
                queue: true,
            },
        ),
    ]);
    assert!(matches!(
        state.unit(worker).unwrap().order,
        Order::Move { .. }
    ));
    assert!(matches!(
        state.unit(fighter).unwrap().order,
        Order::AttackMove { .. }
    ));
    state.tick(&[cancel(fighter, walk(a), 0, &[worker])]);
    for id in [fighter, worker] {
        assert_eq!(keys(&state, id), walks(&[b]));
    }
}

#[test]
fn only_the_named_verb_leaves_a_mixed_program() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 6, 2)])
        .build()
        .unwrap();
    let worker = state.units()[0].id;
    let node = TilePos::new(11, 4);
    let (a, b) = (TilePos::new(8, 6), TilePos::new(3, 6));
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![worker],
                goal: a,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Harvest {
                units: vec![worker],
                node,
                queue: true,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![worker],
                goal: b,
                queue: true,
            },
        ),
    ]);
    state.tick(&[cancel(worker, OrderKey::Harvest { anchor: node }, 0, &[])]);
    assert_eq!(keys(&state, worker), walks(&[a, b]));
}

#[test]
fn an_engagement_keys_the_same_through_either_target_form() {
    let state = sandbox(
        30,
        10,
        &[
            (0, UnitKind::Sentinel, 3, 5),
            (1, UnitKind::Harvester, 9, 5),
        ],
    );
    let enemy = ids(&state, 1)[0];
    let player = PlayerId(0);
    let contact = state
        .attack_objective(player, Target::Unit(enemy).into())
        .expect("the enemy is in sight");
    assert!(matches!(contact, AttackTarget::Contact(_)));
    let engage = |target| Order::Attack {
        target,
        pursue: false,
        resume: None,
    };
    let by_unit = engage(AttackTarget::Unit(enemy)).key(&state, player);
    assert_eq!(by_unit, engage(contact).key(&state, player));
    assert_eq!(by_unit, Some(OrderKey::Attack { objective: contact }));
}

#[test]
fn units_at_different_points_of_a_program_drop_the_same_leg() {
    let mut state = sandbox(
        40,
        12,
        &[(0, UnitKind::Sentinel, 8, 5), (0, UnitKind::Sentinel, 3, 8)],
    );
    let [ahead, behind] = ids(&state, 0)[..] else {
        unreachable!()
    };
    let [a, b, c] = [(9, 5), (30, 5), (30, 9)].map(|(x, y)| TilePos::new(x, y));
    state.tick(&program(&[ahead, behind], &[a, b, c]));
    run_until(&mut state, 100, |state, _| {
        keys(state, ahead) == walks(&[b, c])
    });
    assert_eq!(keys(&state, behind), walks(&[a, b, c]), "test premise");

    let mut late = state.clone();
    late.tick(&[cancel(behind, walk(b), 0, &[ahead])]);
    assert_eq!(keys(&late, behind), walks(&[a, c]));
    assert_eq!(keys(&late, ahead), walks(&[c]), "its active leg went");

    state.tick(&[cancel(behind, walk(a), 0, &[ahead])]);
    assert_eq!(keys(&state, behind), walks(&[b, c]));
    assert_eq!(
        keys(&state, ahead),
        walks(&[b, c]),
        "a unit already past the leg keeps its program"
    );
}

#[test]
fn a_leg_finished_in_flight_neither_shifts_nor_widens_the_removal() {
    let mut state = sandbox(
        40,
        12,
        &[(0, UnitKind::Sentinel, 8, 5), (0, UnitKind::Sentinel, 3, 8)],
    );
    let [subject, other] = ids(&state, 0)[..] else {
        unreachable!()
    };
    let [a, b, c] = [(9, 5), (30, 5), (30, 9)].map(|(x, y)| TilePos::new(x, y));
    state.tick(&program(&[subject, other], &[a, b, c]));
    assert_eq!(keys(&state, subject), walks(&[a, b, c]));
    // Both removals are picked from the program as it stands now.
    let drop_a = cancel(subject, walk(a), 0, &[other]);
    let drop_b = cancel(subject, walk(b), 0, &[other]);
    run_until(&mut state, 100, |state, _| {
        keys(state, subject) == walks(&[b, c])
    });
    assert_eq!(keys(&state, other), walks(&[a, b, c]), "test premise");

    let mut late = state.clone();
    late.tick(&[drop_b]);
    assert_eq!(keys(&late, subject), walks(&[c]), "the named leg went");
    assert_eq!(keys(&late, other), walks(&[a, c]));

    assert_refused(&mut state, drop_a, RejectReason::InvalidTarget);
    assert_eq!(keys(&state, subject), walks(&[b, c]));
    assert_eq!(
        keys(&state, other),
        walks(&[a, b, c]),
        "a refused subject edits no one"
    );
}

#[test]
fn a_repeated_leg_is_counted_from_the_end() {
    let mut state = sandbox(40, 12, &[(0, UnitKind::Sentinel, 3, 5)]);
    let id = ids(&state, 0)[0];
    let [a, b] = [(20, 3), (20, 8)].map(|(x, y)| TilePos::new(x, y));
    state.tick(&program(&[id], &[a, b, a, b]));
    let mut first = state.clone();
    first.tick(&[cancel(id, walk(a), 1, &[])]);
    assert_eq!(keys(&first, id), walks(&[b, a, b]));
    state.tick(&[cancel(id, walk(a), 0, &[])]);
    assert_eq!(keys(&state, id), walks(&[a, b, b]));
    assert_refused(
        &mut state,
        cancel(id, walk(a), 1, &[]),
        RejectReason::InvalidTarget,
    );
}

/// The documented limit: counting from the end names the same visit only
/// while every unit holds the same sequence. Patrol members at different
/// points of a circuit that repeats a waypoint, or a repeat queued to only
/// some of the units, can drop different visits.
#[test]
fn repeated_waypoints_out_of_step_drop_different_visits() {
    let mut state = sandbox(
        40,
        12,
        &[(0, UnitKind::Sentinel, 3, 4), (0, UnitKind::Sentinel, 3, 8)],
    );
    let [subject, other] = ids(&state, 0)[..] else {
        unreachable!()
    };
    let [a, b, c] = [(20, 2), (20, 9), (30, 5)].map(|(x, y)| TilePos::new(x, y));
    state.tick(&[
        cmd(
            0,
            Command::Patrol {
                units: vec![subject],
                waypoints: vec![a, b, a, c],
            },
        ),
        cmd(
            0,
            Command::Patrol {
                units: vec![other],
                waypoints: vec![b, a, c, a],
            },
        ),
    ]);
    let mut patrols = state.clone();
    patrols.tick(&[cancel(subject, walk(a), 1, &[other])]);
    assert_eq!(keys(&patrols, subject), walks(&[b, a, c]));
    assert_eq!(
        keys(&patrols, other),
        walks(&[b, c, a]),
        "the other patrol lost the visit before C, not the one after B"
    );

    state.tick(&program(&[subject], &[a, b, a]));
    state.tick(&program(&[other], &[a, b]));
    state.tick(&[cancel(subject, walk(a), 1, &[other])]);
    assert_eq!(keys(&state, subject), walks(&[b, a]));
    assert_eq!(
        keys(&state, other),
        walks(&[a, b]),
        "a unit without the repeat has no second-to-last visit"
    );
}

#[test]
fn a_landing_that_took_over_a_walk_goes_with_the_walk() {
    let mut state = sandbox(50, 13, &[(0, UnitKind::Condor, 3, 6)]);
    let condor = ids(&state, 0)[0];
    let (click, next) = (TilePos::new(14, 6), TilePos::new(40, 6));
    // Only a walk with nothing queued behind it hands over to a landing.
    state.tick(&program(&[condor], &[click]));
    run_until(&mut state, 800, |state, _| {
        matches!(state.unit(condor).unwrap().order, Order::Land { .. })
    });
    let Order::Land { from, .. } = state.unit(condor).unwrap().order else {
        unreachable!()
    };
    assert_eq!(from, Some(click), "test premise");
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![condor],
            goal: next,
            queue: true,
        },
    )]);
    assert!(matches!(
        state.unit(condor).unwrap().order,
        Order::Land { .. }
    ));
    assert_eq!(keys(&state, condor), walks(&[click, next]));
    state.tick(&[cancel(condor, walk(click), 0, &[])]);
    assert_eq!(keys(&state, condor), walks(&[next]));
}

#[test]
fn a_subject_listed_among_the_units_edits_once() {
    let mut state = sandbox(40, 12, &[(0, UnitKind::Sentinel, 3, 5)]);
    let id = ids(&state, 0)[0];
    let [a, b] = [(20, 3), (20, 8)].map(|(x, y)| TilePos::new(x, y));
    state.tick(&program(&[id], &[a, b, a]));
    state.tick(&[cancel(id, walk(a), 1, &[id, id])]);
    assert_eq!(keys(&state, id), walks(&[b, a]));
}

/// Every refusal is checked before any unit changes: `holder` holds each
/// named order, so an edit made ahead of the subject's check would show.
#[test]
fn refusals_leave_the_world_untouched() {
    let mut state = sandbox(
        40,
        12,
        &[
            (0, UnitKind::Sentinel, 3, 5),
            (0, UnitKind::Sentinel, 8, 5),
            (0, UnitKind::Skyhook, 9, 5),
            (1, UnitKind::Sentinel, 30, 5),
            (0, UnitKind::Sentinel, 3, 8),
        ],
    );
    let [subject, rider, skyhook, holder] = ids(&state, 0)[..] else {
        unreachable!()
    };
    let foreign = ids(&state, 1)[0];
    let [a, b, c] = [(20, 3), (20, 8), (30, 8)].map(|(x, y)| TilePos::new(x, y));
    state.tick(&[cmd(
        0,
        Command::Load {
            units: vec![rider],
            transport: skyhook,
            queue: false,
        },
    )]);
    run_until(&mut state, 200, |state, _| state.unit(rider).is_none());
    state.tick(&program(&[subject], &[a, b]));
    state.tick(&program(&[holder], &[a, c, a]));

    for (subject, reason) in [
        (foreign, RejectReason::NoValidUnits),
        (UnitId(9_999), RejectReason::NoValidUnits),
        (rider, RejectReason::NoValidUnits),
    ] {
        assert_refused(
            &mut state,
            cancel(subject, walk(a), 0, &[holder, skyhook]),
            reason,
        );
    }
    for (key, from_end) in [(walk(c), 0), (walk(a), 1)] {
        assert_refused(
            &mut state,
            cancel(subject, key, from_end, &[holder]),
            RejectReason::InvalidTarget,
        );
    }
    assert_eq!(keys(&state, subject), walks(&[a, b]));
    assert_eq!(keys(&state, holder), walks(&[a, c, a]));
}
