//! Tile goals keep the clicked tile. A group spreads over the ground around
//! it when its owner's team has explored the tile; otherwise every member
//! heads for the tile itself until the end-of-tick exposure pass hands each
//! its slot. Seen and unseen clicks are paired throughout, so the spread can
//! never read ground the player has not seen.

use crate::common;
use common::{cmd, run_until};

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::event::StallReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{
    AttackTarget, Command, Event, Faction, Goal, Order, PlayerCommand, PlayerId, Scenario, State,
    UnitId, UnitKind,
};

/// An open sandbox of `width` by `height`, with `carve` editing the grid.
fn field(width: usize, height: usize, carve: impl Fn(&mut Vec<Vec<char>>)) -> Vec<String> {
    let mut rows = vec![vec!['.'; width]; height];
    carve(&mut rows);
    rows.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

/// Fills the rectangle `(x0, y0)..=(x1, y1)` with `glyph`.
fn fill(rows: &mut [Vec<char>], (x0, y0): (usize, usize), (x1, y1): (usize, usize), glyph: char) {
    for row in &mut rows[y0..=y1] {
        for cell in &mut row[x0..=x1] {
            *cell = glyph;
        }
    }
}

/// A sandbox over `map` with one seat per team entry.
fn sandbox(map: Vec<String>, teams: &[Option<u8>], units: &[(u8, UnitKind, i32, i32)]) -> State {
    Scenario {
        mode: ScenarioMode::Sandbox,
        name: "clicked-tile-goals".into(),
        seed: 23,
        map,
        players: teams
            .iter()
            .enumerate()
            .map(|(seat, &team)| PlayerSpec {
                name: format!("seat {seat}"),
                faction: Faction::Ferrous,
                team,
                scrap: 2_000,
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

/// The goal of a unit's active walking order.
fn goal(state: &State, id: UnitId) -> Option<Goal> {
    match state.unit(id)?.order {
        Order::Run { goal } | Order::Hunt { goal } | Order::Advance { goal } => Some(goal),
        Order::Unload { at } => Some(at),
        _ => None,
    }
}

fn explored(state: &State, player: u8, tile: TilePos) -> bool {
    state.vision(PlayerId(player)).explored(tile)
}

fn move_to(units: Vec<UnitId>, goal: TilePos) -> Command {
    Command::Run {
        units,
        goal,
        queue: false,
    }
}

fn no_routes(events: &[Event]) -> Vec<UnitId> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::OrderStalled {
                unit,
                reason: StallReason::NoRoute,
                ..
            } => Some(*unit),
            _ => None,
        })
        .collect()
}

fn distinct(mut tiles: Vec<TilePos>) -> usize {
    tiles.sort_unstable_by_key(|tile| (tile.y, tile.x));
    tiles.dedup();
    tiles.len()
}

#[test]
fn a_group_spreads_at_issue_on_explored_ground_and_on_exposure_otherwise() {
    let map = field(44, 12, |_| {});
    let units = [(3, 5), (3, 6), (4, 5), (4, 6)].map(|(x, y)| (0, UnitKind::Sentinel, x, y));
    let base = sandbox(map, &[None], &units);
    let group = ids(&base, 0);

    let seen = TilePos::new(8, 6);
    assert!(explored(&base, 0, seen));
    let mut state = base.clone();
    state.tick(&[cmd(0, move_to(group.clone(), seen))]);
    let goals: Vec<Goal> = group.iter().map(|&id| goal(&state, id).unwrap()).collect();
    assert!(
        goals
            .iter()
            .all(|goal| goal.tile() == seen && !goal.is_pending())
    );
    assert_eq!(
        goals[0],
        Goal::at(seen),
        "the first member takes the open click"
    );
    assert_eq!(
        distinct(goals.iter().map(Goal::target).collect()),
        group.len(),
        "the rest spread at issue: {goals:?}"
    );

    let unseen = TilePos::new(34, 6);
    assert!(!explored(&base, 0, unseen));
    let mut state = base;
    state.tick(&[cmd(0, move_to(group.clone(), unseen))]);
    let pending: Vec<Goal> = group.iter().map(|&id| goal(&state, id).unwrap()).collect();
    let oxide_sim::Aim::Pending { reverse, .. } = pending[0].aim else {
        panic!("an unseen click waits for exposure: {pending:?}");
    };
    for (rank, goal) in pending.iter().enumerate() {
        assert_eq!(goal.tile(), unseen);
        assert_eq!(goal.destination(), unseen, "one shared tile until seen");
        assert_eq!(
            goal.aim,
            oxide_sim::Aim::Pending {
                rank: rank as u8,
                reverse
            },
            "each member holds its place in one frame"
        );
    }
    run_until(&mut state, 400, |state, _| {
        let goals: Vec<Goal> = group.iter().filter_map(|&id| goal(state, id)).collect();
        assert_eq!(
            goals.len(),
            group.len(),
            "no member arrives before exposure"
        );
        if goals.iter().any(Goal::is_pending) {
            assert!(!explored(state, 0, unseen));
            assert!(goals.iter().all(|goal| goal.target() == unseen));
            return false;
        }
        assert!(explored(state, 0, unseen));
        assert_eq!(goals[0], Goal::at(unseen));
        assert_eq!(
            distinct(goals.iter().map(Goal::target).collect()),
            group.len()
        );
        true
    });
    let events = run_until(&mut state, 400, |state, _| {
        group
            .iter()
            .all(|&id| state.unit(id).unwrap().order == Order::Idle)
    });
    assert!(no_routes(&events).is_empty());
    assert!(
        group
            .iter()
            .all(|&id| state.unit(id).unwrap().tile().chebyshev(unseen) <= 2),
        "the group parks around the click"
    );
}

#[test]
fn a_teammates_sight_hands_a_far_unit_its_slot() {
    let map = field(60, 9, |_| {});
    let mut state = sandbox(
        map,
        &[Some(0), Some(0)],
        &[
            (0, UnitKind::Sentinel, 2, 4),
            (1, UnitKind::Sentinel, 54, 4),
        ],
    );
    let (mine, ally) = (ids(&state, 0)[0], ids(&state, 1)[0]);
    let click = TilePos::new(40, 4);
    assert!(!explored(&state, 0, click) && !explored(&state, 1, click));
    state.tick(&[
        cmd(0, move_to(vec![mine], click)),
        cmd(1, move_to(vec![ally], TilePos::new(47, 4))),
    ]);
    assert!(goal(&state, mine).unwrap().is_pending());
    run_until(&mut state, 300, |state, _| {
        !goal(state, mine).unwrap().is_pending()
    });
    assert_eq!(goal(&state, mine), Some(Goal::at(click)));
    let far = state.unit(mine).unwrap().tile();
    assert!(
        far.chebyshev(click) > UnitKind::Sentinel.stats().vision,
        "the click was exposed by the ally, not by {far:?}"
    );
}

/// A seat-0 Foundry in the corner, a rock outcrop near it, and a rock
/// outcrop far out of its sight.
fn rally_field() -> State {
    let map = field(48, 12, |rows| {
        rows[1][1] = '1';
        fill(rows, (9, 2), (10, 3), '#');
        fill(rows, (40, 6), (41, 7), '#');
    });
    let mut state = sandbox(map, &[None], &[]);
    let foundry = state.buildings()[0].id;
    assert_eq!(state.building(foundry).unwrap().kind, BuildingKind::Foundry);
    state.tick(&[]);
    state
}

/// Rallies the seat's Foundry onto `rally`, trains a Harvester, and returns
/// the newborn at the end of its birth tick.
fn rally_newborn(state: &mut State, rally: TilePos) -> UnitId {
    let foundry = state.buildings()[0].id;
    state.tick(&[
        cmd(
            0,
            Command::SetRally {
                building: foundry,
                rally: Some(rally),
            },
        ),
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        ),
    ]);
    let events = run_until(state, 400, |_, events| {
        events
            .iter()
            .any(|event| matches!(event, Event::UnitTrained { .. }))
    });
    events
        .iter()
        .find_map(|event| match event {
            Event::UnitTrained { unit, .. } => Some(*unit),
            _ => None,
        })
        .unwrap()
}

#[test]
fn a_rally_on_explored_rock_resolves_at_birth_and_an_unexplored_one_on_exposure() {
    let mut state = rally_field();
    let seen = TilePos::new(9, 2);
    assert!(explored(&state, 0, seen) && !state.passable(seen));
    let newborn = rally_newborn(&mut state, seen);
    let goal_at_birth = goal(&state, newborn).expect("the newborn walks to the rally");
    assert_eq!(goal_at_birth.tile(), seen);
    assert!(matches!(goal_at_birth.aim, oxide_sim::Aim::Slot(slot) if state.passable(slot)));

    let mut state = rally_field();
    let unseen = TilePos::new(40, 6);
    assert!(!explored(&state, 0, unseen) && !state.passable(unseen));
    let newborn = rally_newborn(&mut state, unseen);
    let pending = goal(&state, newborn).expect("the newborn walks to the rally");
    assert_eq!(pending.tile(), unseen);
    assert!(matches!(
        pending.aim,
        oxide_sim::Aim::Pending { rank: 0, .. }
    ));
    run_until(&mut state, 600, |state, _| {
        goal(state, newborn).is_some_and(|goal| !goal.is_pending())
    });
    assert!(matches!(
        goal(&state, newborn).unwrap().aim,
        oxide_sim::Aim::Slot(slot) if state.passable(slot)
    ));
    let events = run_until(&mut state, 600, |state, _| {
        state.unit(newborn).unwrap().order == Order::Idle
    });
    assert!(no_routes(&events).is_empty(), "the slot was reachable");
}

#[test]
fn an_off_map_rally_from_an_old_record_is_clamped_onto_the_map() {
    let mut state = rally_field();
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["buildings"][0]["rally"] = serde_json::json!({"x": -2, "y": 5});
    state = serde_json::from_value(doc).expect("the envelope admits it");
    let foundry = state.buildings()[0].id;
    state.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    )]);
    let events = run_until(&mut state, 400, |_, events| {
        events
            .iter()
            .any(|event| matches!(event, Event::UnitTrained { .. }))
    });
    let newborn = events
        .iter()
        .find_map(|event| match event {
            Event::UnitTrained { unit, .. } => Some(*unit),
            _ => None,
        })
        .unwrap();
    assert_eq!(goal(&state, newborn).unwrap().tile(), TilePos::new(0, 5));
    let events = run_until(&mut state, 400, |state, _| {
        state.unit(newborn).unwrap().order == Order::Idle
    });
    assert!(no_routes(&events).is_empty());
    assert_eq!(state.unit(newborn).unwrap().tile(), TilePos::new(0, 5));
}

#[test]
fn pacifists_sent_at_an_unexplored_radar_contact_share_its_tile() {
    let mut scenario = common::open_arena(
        40,
        30,
        vec![
            common::unit(1, UnitKind::Gnat, 14, 8),
            common::unit(0, UnitKind::Harvester, 5, 20),
            common::unit(0, UnitKind::Harvester, 7, 21),
        ],
    );
    scenario.buildings = vec![BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 5,
        y: 16,
    }];
    let mut state = scenario.build().unwrap();
    let contact_tile = TilePos::new(14, 8);
    assert!(!explored(&state, 0, contact_tile));
    let contact = state
        .vision(PlayerId(0))
        .tracks()
        .iter()
        .find(|track| track.visible_unit.is_none())
        .expect("the Array tracks the Gnat")
        .id;
    let walkers = ids(&state, 0);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: walkers.clone(),
            target: AttackTarget::Contact(contact),
            queue: false,
        },
    )]);
    let goals: Vec<Goal> = walkers
        .iter()
        .map(|&id| goal(&state, id).unwrap())
        .collect();
    assert_eq!(goals[0], goals[1], "one tile, one frame, rank 0 for both");
    assert_eq!(goals[0].tile(), contact_tile);
    assert!(matches!(
        goals[0].aim,
        oxide_sim::Aim::Pending { rank: 0, .. }
    ));
    run_until(&mut state, 400, |state, _| {
        walkers
            .iter()
            .all(|&id| goal(state, id).is_some_and(|goal| !goal.is_pending()))
    });
    assert_eq!(goal(&state, walkers[0]), goal(&state, walkers[1]));
    assert_eq!(goal(&state, walkers[0]), Some(Goal::at(contact_tile)));
}

#[test]
fn a_pending_group_into_an_unexplored_island_ends_at_the_shore() {
    let map = field(40, 11, |rows| {
        fill(rows, (14, 0), (25, 10), '#');
    });
    let units: Vec<_> = (0..8)
        .map(|i| (0, UnitKind::Sentinel, 2 + i % 4, 3 + i / 4 * 4))
        .collect();
    let mut state = sandbox(map, &[None], &units);
    let group = ids(&state, 0);
    let island = TilePos::new(33, 5);
    assert!(!explored(&state, 0, island));
    state.tick(&[cmd(0, move_to(group.clone(), island))]);
    assert!(
        group
            .iter()
            .all(|&id| goal(&state, id).unwrap().is_pending())
    );
    let events = run_until(&mut state, 800, |state, _| {
        group
            .iter()
            .all(|&id| state.unit(id).unwrap().order == Order::Idle)
    });
    let mut reports = no_routes(&events);
    reports.sort_unstable();
    assert_eq!(reports, group, "each member reports its shortfall once");
    assert!(
        group
            .iter()
            .all(|&id| state.unit(id).unwrap().tile().x >= 8),
        "the group settled at the shore"
    );
}

#[test]
fn an_explored_click_with_no_open_ground_near_it_ends_as_one_crowd() {
    let map = field(30, 15, |rows| {
        fill(rows, (11, 3), (19, 11), '#');
    });
    let units: Vec<_> = [(9, 6), (9, 7), (9, 8), (10, 6), (10, 8), (8, 7)]
        .map(|(x, y)| (0, UnitKind::Sentinel, x, y))
        .into();
    let mut state = sandbox(map, &[None], &units);
    let group = ids(&state, 0);
    let click = TilePos::new(15, 7);
    assert!(explored(&state, 0, click));
    state.tick(&[cmd(0, move_to(group.clone(), click))]);
    assert!(
        group
            .iter()
            .all(|&id| goal(&state, id).is_some_and(|goal| goal.target() == click)),
        "nothing near the click to spread over"
    );
    let events = run_until(&mut state, 300, |state, _| {
        group
            .iter()
            .all(|&id| state.unit(id).unwrap().order == Order::Idle)
    });
    let mut reports = no_routes(&events);
    reports.sort_unstable();
    assert_eq!(reports, group, "each member reports its shortfall once");
}

#[test]
fn reissuing_a_walk_keeps_its_path_seen_or_unseen() {
    for click in [TilePos::new(10, 6), TilePos::new(34, 6)] {
        let map = field(44, 12, |_| {});
        let mut state = sandbox(
            map,
            &[None],
            &[(0, UnitKind::Sentinel, 3, 5), (0, UnitKind::Sentinel, 3, 7)],
        );
        let group = ids(&state, 0);
        let order = cmd(0, move_to(group.clone(), click));
        state.tick(std::slice::from_ref(&order));
        for _ in 0..3 {
            state.tick(&[]);
        }
        let before: Vec<_> = group
            .iter()
            .map(|&id| state.unit(id).unwrap().clone())
            .collect();
        assert!(before.iter().all(|unit| unit.path.is_some()));
        state.inspect_command_phase(std::slice::from_ref(&order), |view| {
            for unit in &before {
                let after = view.unit(unit.id).unwrap();
                assert_eq!(after.order, unit.order, "{click}");
                assert_eq!(after.path, unit.path, "{click}");
            }
        });
    }
}

#[test]
fn a_reissue_onto_a_new_slot_adopts_it_without_touching_the_path() {
    let map = field(30, 12, |rows| {
        fill(rows, (20, 0), (20, 11), '#');
    });
    let mut state = sandbox(
        map,
        &[None],
        &[(0, UnitKind::Sentinel, 3, 5), (0, UnitKind::Sentinel, 3, 7)],
    );
    let group = ids(&state, 0);
    let click = TilePos::new(8, 6);
    state.tick(&[cmd(0, move_to(group.clone(), click))]);
    state.tick(&[]);
    let second = state.unit(group[1]).unwrap().clone();
    let slotted = goal(&state, group[1]).unwrap();
    assert!(matches!(slotted.aim, oxide_sim::Aim::Slot(_)));
    state.inspect_command_phase(&[cmd(0, move_to(vec![group[1]], click))], |view| {
        let after = view.unit(group[1]).unwrap();
        let Order::Run { goal } = after.order else {
            panic!("still walking");
        };
        assert_eq!(goal, Goal::at(click), "alone, it takes the click itself");
        assert_eq!(after.path, second.path, "the walk replans its own path");
    });
}

#[test]
fn an_engagement_resumes_toward_the_same_slot() {
    let map = field(40, 12, |_| {});
    let mut state = sandbox(
        map,
        &[None, None],
        &[
            (0, UnitKind::Sentinel, 3, 5),
            (0, UnitKind::Sentinel, 3, 6),
            (1, UnitKind::Harvester, 6, 8),
        ],
    );
    let group = ids(&state, 0);
    let click = TilePos::new(9, 5);
    assert!(explored(&state, 0, click));
    let command = cmd(
        0,
        Command::Hunt {
            units: group.clone(),
            goal: click,
            queue: false,
        },
    );
    let march = state.inspect_command_phase(std::slice::from_ref(&command), |view| {
        match view.unit(group[1]).unwrap().order {
            Order::Hunt { goal } => goal,
            other => panic!("marching: {other:?}"),
        }
    });
    assert!(matches!(march.aim, oxide_sim::Aim::Slot(_)));
    state.tick(&[command]);
    run_until(&mut state, 300, |state, _| {
        matches!(state.unit(group[1]).unwrap().order, Order::Attack { .. })
    });
    let Order::Attack { resume, .. } = state.unit(group[1]).unwrap().order else {
        unreachable!();
    };
    assert_eq!(
        resume.map(|goal| (goal.tile(), goal.aim)),
        Some((click, march.aim))
    );
    run_until(&mut state, 600, |state, _| {
        matches!(state.unit(group[1]).unwrap().order, Order::Hunt { .. })
    });
    let resumed = goal(&state, group[1]).unwrap();
    assert_eq!((resumed.tile(), resumed.aim), (click, march.aim));
}

#[test]
fn patrol_legs_spread_per_unit() {
    let map = field(20, 12, |_| {});
    let units = [(3, 5), (3, 6), (4, 5)].map(|(x, y)| (0, UnitKind::Sentinel, x, y));
    let mut state = sandbox(map, &[None], &units);
    let group = ids(&state, 0);
    let legs = [TilePos::new(9, 3), TilePos::new(9, 9)];
    assert!(legs.iter().all(|&leg| explored(&state, 0, leg)));
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: group.clone(),
            waypoints: legs.into(),
        },
    )]);
    for (leg, &click) in legs.iter().enumerate() {
        let targets: Vec<TilePos> = group
            .iter()
            .map(|&id| {
                let unit = state.unit(id).unwrap();
                let order = std::iter::once(&unit.order)
                    .chain(&unit.queue)
                    .find(|order| matches!(order, Order::Hunt { goal } if goal.tile() == click))
                    .copied()
                    .unwrap_or_else(|| panic!("unit {id} patrols {click}"));
                let Order::Hunt { goal } = order else {
                    unreachable!()
                };
                assert!(!goal.is_pending());
                goal.target()
            })
            .collect();
        assert_eq!(distinct(targets), group.len(), "leg {leg} spreads");
    }
}

/// A peak range across a sandbox, with open sky everywhere else.
fn range() -> Vec<String> {
    field(40, 11, |rows| {
        fill(rows, (30, 3), (31, 7), '^');
    })
}

#[test]
fn fliers_ordered_onto_a_peak_settle_off_it_seen_or_unseen() {
    for kind in [UnitKind::Wisp, UnitKind::Skyhook] {
        for (start, seen) in [(26, true), (3, false)] {
            let mut state = sandbox(range(), &[None], &[(0, kind, start, 5)]);
            let flier = ids(&state, 0)[0];
            let peak = TilePos::new(30, 5);
            assert_eq!(explored(&state, 0, peak), seen, "{kind:?} from {start}");
            state.tick(&[cmd(0, move_to(vec![flier], peak))]);
            let issued = goal(&state, flier).unwrap();
            assert_eq!(issued.tile(), peak);
            assert_eq!(issued.is_pending(), !seen, "{kind:?} from {start}");
            let events = run_until(&mut state, 400, |state, _| {
                state.unit(flier).unwrap().order == Order::Idle
            });
            assert!(no_routes(&events).is_empty(), "{kind:?} from {start}");
            let rest = state.unit(flier).unwrap().tile();
            assert!(state.passable_for(oxide_sim::stats::Domain::Air, rest));
            assert!(rest.chebyshev(peak) <= 2, "{kind:?} rests at {rest:?}");
        }
    }
}

#[test]
fn a_condor_hands_off_only_once_its_click_is_explored() {
    let map = field(50, 13, |_| {});
    let mut state = sandbox(map, &[None], &[(0, UnitKind::Condor, 3, 6)]);
    let condor = ids(&state, 0)[0];
    let click = TilePos::new(36, 6);
    assert!(!explored(&state, 0, click));
    state.tick(&[cmd(0, move_to(vec![condor], click))]);
    let reach = oxide_sim::stats::LANDING_HANDOFF_REACH;
    let mut pending_in_reach = 0;
    let mut was_pending = true;
    run_until(&mut state, 800, |state, _| {
        let unit = state.unit(condor).unwrap();
        match unit.order {
            Order::Run { goal } => {
                if goal.is_pending() {
                    assert!(!explored(state, 0, click));
                    pending_in_reach +=
                        u32::from(unit.pos.dist_sq(click.center()) <= reach * reach);
                }
                was_pending = goal.is_pending();
                false
            }
            Order::Land { goal: pad, from } => {
                assert!(!was_pending, "no handoff while the click is unexplored");
                assert_eq!(from, Some(click));
                assert!(
                    explored(state, 0, pad),
                    "the pad was chosen from seen ground"
                );
                true
            }
            other => panic!("unexpected order {other:?}"),
        }
    });
    assert!(
        pending_in_reach > 0,
        "premise: in reach before the click was seen"
    );
    run_until(&mut state, 800, |state, _| {
        state.unit(condor).unwrap().landed
    });
}

#[test]
fn off_map_tile_goals_are_refused_without_a_trace() {
    let map = field(20, 10, |rows| rows[1][1] = '1');
    let state = sandbox(
        map,
        &[None],
        &[(0, UnitKind::Sentinel, 6, 5), (0, UnitKind::Skyhook, 8, 5)],
    );
    let (sentinel, skyhook) = (ids(&state, 0)[0], ids(&state, 0)[1]);
    let foundry = state.buildings()[0].id;
    for tile in [
        TilePos::new(-1, 4),
        TilePos::new(20, 4),
        TilePos::new(4, -1),
        TilePos::new(4, 10),
    ] {
        let commands: [Command; 6] = [
            move_to(vec![sentinel], tile),
            Command::Hunt {
                units: vec![sentinel],
                goal: tile,
                queue: false,
            },
            Command::Advance {
                units: vec![sentinel],
                goal: tile,
                queue: false,
            },
            Command::Patrol {
                units: vec![sentinel],
                waypoints: vec![TilePos::new(6, 5), tile],
            },
            Command::Unload {
                transport: skyhook,
                at: tile,
                queue: false,
            },
            Command::SetRally {
                building: foundry,
                rally: Some(tile),
            },
        ];
        for command in commands {
            let mut refused = state.clone();
            let mut control = state.clone();
            let report = refused.tick(&[PlayerCommand {
                player: PlayerId(0),
                command: command.clone(),
            }]);
            control.tick(&[]);
            assert!(
                report.events.contains(&Event::CommandRejected {
                    player: PlayerId(0),
                    reason: RejectReason::OutOfBounds,
                }),
                "{command:?}"
            );
            assert_eq!(refused.hash(), control.hash(), "{command:?}");
        }
    }
}
