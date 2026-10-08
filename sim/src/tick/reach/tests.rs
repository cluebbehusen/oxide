use super::*;
use crate::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
use crate::state::Faction;
use crate::stats::UnitKind;
use crate::{Event, Scenario};

/// A sandbox over `map` with `seats` players and one unit per spec.
fn world(map: &[&str], seats: usize, units: &[(u8, UnitKind, i32, i32)]) -> State {
    world_with_teams(map, &vec![None; seats], units)
}

/// A sandbox over `map` with one player per team entry.
fn world_with_teams(
    map: &[&str],
    teams: &[Option<u8>],
    units: &[(u8, UnitKind, i32, i32)],
) -> State {
    let scenario = Scenario {
        mode: ScenarioMode::Sandbox,
        name: "reach".into(),
        seed: 3,
        map: map.iter().map(|row| (*row).to_owned()).collect(),
        players: teams
            .iter()
            .enumerate()
            .map(|(seat, &team)| PlayerSpec {
                name: format!("p{seat}"),
                faction: Faction::Ferrous,
                team,
                scrap: 0,
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
    };
    scenario.build().expect("reach fixture builds")
}

/// A 3x3 wall around an open pocket at (5, 3), centered on an 11x7 map.
const POCKET: [&str; 7] = [
    "...........",
    "...........",
    "....###....",
    "....#.#....",
    "....###....",
    "...........",
    "...........",
];

fn id(state: &State, slot: usize) -> UnitId {
    state.units[slot].id
}

#[test]
fn rings_follow_the_spread_scan_order() {
    for r in 0i32..6 {
        let mut expected = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) == r {
                    expected.push((dx, dy));
                }
            }
        }
        assert_eq!(ring(r).collect::<Vec<_>>(), expected, "ring {r}");
    }
}

#[test]
fn a_reachable_target_is_its_own_endpoint() {
    let state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 1)]);
    let mut reach = Reach::new(&state);
    let target = TilePos::new(9, 5);
    assert_eq!(
        reach.endpoint(&state, id(&state, 0), target, None),
        Some(target)
    );
    assert!(
        reach.endpoints.is_empty(),
        "no scan runs for a reachable target"
    );
}

#[test]
fn a_cache_hit_equals_a_miss() {
    let state = world(
        &POCKET,
        1,
        &[(0, UnitKind::Sentinel, 1, 1), (0, UnitKind::Sentinel, 9, 1)],
    );
    let pocket = TilePos::new(5, 3);
    let mut shared = Reach::new(&state);
    let first = shared.endpoint(&state, id(&state, 0), pocket, None);
    let second = shared.endpoint(&state, id(&state, 1), pocket, None);
    assert_eq!(
        shared.endpoints.len(),
        1,
        "premise: both units share one component, target and frame"
    );
    assert_eq!(
        second,
        Reach::new(&state).endpoint(&state, id(&state, 1), pocket, None)
    );
    assert_eq!(
        first,
        Reach::new(&state).endpoint(&state, id(&state, 0), pocket, None)
    );
    assert_eq!(first, Some(TilePos::new(5, 1)));
}

#[test]
fn mid_phase_depletion_reopens_the_ground() {
    let mut state = world(
        &[
            "...........",
            "....###....",
            "....#.s....",
            "....###....",
            "...........",
        ],
        1,
        &[(0, UnitKind::Sentinel, 1, 2)],
    );
    let walker = id(&state, 0);
    let pocket = TilePos::new(5, 2);
    let mut reach = Reach::new(&state);
    let sealed = reach.endpoint(&state, walker, pocket, None);
    assert!(sealed.is_some_and(|tile| tile != pocket));
    let node = TilePos::new(6, 2);
    while state.map.extract_scrap(node).is_some_and(|left| left > 0) {}
    assert!(state.passable(node), "premise: the node is gone");
    assert_eq!(
        reach.endpoint(&state, walker, pocket, None),
        sealed,
        "labels describe the ground they were built from"
    );
    reach.forget_ground();
    assert_eq!(reach.endpoint(&state, walker, pocket, None), Some(pocket));
}

#[test]
fn mirrored_endpoints_are_symmetric() {
    let state = world(
        &POCKET,
        2,
        &[
            (0, UnitKind::Sentinel, 1, 3),
            (1, UnitKind::Sentinel, 9, 3),
            (0, UnitKind::Sentinel, 2, 5),
            (1, UnitKind::Sentinel, 8, 1),
        ],
    );
    let mirror = |tile: TilePos| TilePos::new(10 - tile.x, 6 - tile.y);
    let pocket = TilePos::new(5, 3);
    let mut reach = Reach::new(&state);
    for (left, right) in [(0, 1), (2, 3)] {
        let a = reach.endpoint(&state, id(&state, left), pocket, None);
        let b = reach.endpoint(&state, id(&state, right), mirror(pocket), None);
        assert_eq!(a.map(mirror), b, "units {left} and {right}");
        assert!(a.is_some_and(|tile| tile != pocket));
    }
    // Four shore tiles tie at distance two; each side keeps its own
    // frame's first rather than an absolute corner.
    assert_eq!(
        reach.endpoint(&state, id(&state, 0), pocket, None),
        Some(TilePos::new(5, 1))
    );
    assert_eq!(
        reach.endpoint(&state, id(&state, 1), pocket, None),
        Some(TilePos::new(5, 5))
    );
}

#[test]
fn a_recompute_keeps_a_stored_endpoint_that_is_still_tied() {
    let state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 3)]);
    let walker = id(&state, 0);
    let pocket = TilePos::new(5, 3);
    let mut reach = Reach::new(&state);
    let tied = TilePos::new(3, 3);
    assert_eq!(
        reach.endpoint(&state, walker, pocket, Some(tied)),
        Some(tied)
    );
    assert_eq!(
        reach.endpoint(&state, walker, pocket, Some(TilePos::new(5, 0))),
        Some(TilePos::new(5, 1)),
        "a farther stored tile yields to the nearest"
    );
    assert_eq!(
        reach.endpoint(&state, walker, pocket, Some(TilePos::new(4, 2))),
        Some(TilePos::new(5, 1)),
        "a closed stored tile yields to the nearest"
    );
}

#[test]
fn a_blocked_start_whose_only_open_neighbour_is_diagonal_is_sealed() {
    let mut state = world(
        &[".##..", "###..", "###..", "....."],
        1,
        &[(0, UnitKind::Sentinel, 4, 3)],
    );
    let walker = id(&state, 0);
    state.units[0].pos = TilePos::new(1, 1).center();
    let mut reach = Reach::new(&state);
    for target in [TilePos::new(0, 0), TilePos::new(4, 3)] {
        assert_eq!(reach.endpoint(&state, walker, target, None), None);
        assert!(
            super::super::route_for(&state, UnitKind::Sentinel, TilePos::new(1, 1), target)
                .is_none(),
            "A* agrees: no corner cut leaves the tile"
        );
    }
    // One open cardinal neighbour is enough.
    state.units[0].pos = TilePos::new(2, 1).center();
    assert_eq!(
        Reach::new(&state).endpoint(&state, walker, TilePos::new(4, 3), None),
        Some(TilePos::new(4, 3))
    );
}

#[test]
fn an_off_map_target_clamps_onto_the_map() {
    let state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 1)]);
    let walker = id(&state, 0);
    let mut reach = Reach::new(&state);
    for (target, clamped) in [
        (TilePos::new(-2_048, 3), TilePos::new(0, 3)),
        (TilePos::new(4, 2_048), TilePos::new(4, 6)),
        (TilePos::new(2_048, -2_048), TilePos::new(10, 0)),
    ] {
        assert_eq!(
            reach.endpoint(&state, walker, target, None),
            Some(clamped),
            "{target:?}"
        );
    }
}

#[test]
fn an_off_map_unit_does_not_panic() {
    let mut state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 1)]);
    let walker = id(&state, 0);
    state.units[0].pos = TilePos::new(-40, -40).center();
    let mut reach = Reach::new(&state);
    assert_eq!(
        reach.endpoint(&state, walker, TilePos::new(5, 3), None),
        None
    );
    state.units[0].pos = TilePos::new(-1, 2).center();
    assert_eq!(
        Reach::new(&state).endpoint(&state, walker, TilePos::new(5, 3), None),
        None,
        "an open neighbour across the edge is no way back"
    );
    assert!(
        super::super::route_for(
            &state,
            UnitKind::Sentinel,
            TilePos::new(-1, 2),
            TilePos::new(0, 2)
        )
        .is_none(),
        "A* agrees: no route starts off the map"
    );
}

#[test]
fn a_crowd_chain_reaches_back_from_the_endpoint() {
    let mut state = world(
        &["..............", "..............", ".............."],
        1,
        &[
            (0, UnitKind::Sentinel, 1, 1),
            (0, UnitKind::Sentinel, 2, 1),
            (0, UnitKind::Sentinel, 3, 1),
            (0, UnitKind::Sentinel, 4, 1),
            (0, UnitKind::Sentinel, 9, 1),
            (0, UnitKind::Wisp, 5, 1),
        ],
    );
    // A line of touching parked bodies from the endpoint eastward.
    let step = UnitKind::Sentinel.stats().radius * 2;
    for slot in 0..4 {
        state.units[slot].pos = TilePos::new(1, 1).center()
            + chassis::fx::Vec2Fx::new(step * Fx::from_num(slot), Fx::ZERO);
    }
    let end = state.units[3].pos;
    state.units[4].pos = end + chassis::fx::Vec2Fx::new(step, Fx::ZERO);
    state.units[4].order = Order::Run {
        goal: TilePos::new(0, 1).into(),
    };
    state.units[5].pos = end + chassis::fx::Vec2Fx::new(step, Fx::ZERO);
    let mut index = UnitIndex::new();
    index.rebuild(&state.units);
    let endpoint = TilePos::new(1, 1);
    let mut reach = Reach::new(&state);
    assert!(reach.crowd_touches(&state, &index, id(&state, 4), endpoint));
    assert!(
        !reach.crowd_touches(&state, &index, id(&state, 5), endpoint),
        "a flier never touches a ground crowd"
    );
    // A body that starts walking mid-phase still counts: the crowd is the
    // one that stood when the phase began, whoever asks first.
    state.units[2].order = Order::Run {
        goal: TilePos::new(12, 1).into(),
    };
    assert!(reach.crowd_touches(&state, &index, id(&state, 4), endpoint));
    assert!(
        !Reach::new(&state).crowd_touches(&state, &index, id(&state, 4), endpoint),
        "the broken chain no longer reaches the endpoint"
    );
    state.units[4].pos = TilePos::new(12, 1).center();
    index.rebuild(&state.units);
    assert!(
        !Reach::new(&state).crowd_touches(&state, &index, id(&state, 4), endpoint),
        "a walker touching nothing has not arrived"
    );
    assert!(!reach.crowd_touches(&state, &index, UnitId(9_999), endpoint));
}

#[test]
fn a_crowd_chain_counts_only_bodies_its_side_may_count() {
    // Seat 0 walks; seat 1 is its teammate and seat 2 is hostile. Bodies
    // 0-3 run east from the endpoint in touching steps, the first three
    // inside the arrival disk. Body 4 is the walker's own and touches
    // body 3; the walker touches only body 4.
    let run = |owners: [u8; 4]| {
        let mut units: Vec<(u8, UnitKind, i32, i32)> = owners
            .iter()
            .map(|&owner| (owner, UnitKind::Sentinel, 1, 1))
            .collect();
        units.push((0, UnitKind::Sentinel, 1, 1));
        units.push((0, UnitKind::Sentinel, 1, 1));
        let mut state = world_with_teams(
            &["..............", "..............", ".............."],
            &[Some(0), Some(0), Some(1)],
            &units,
        );
        let step = UnitKind::Sentinel.stats().radius * 2;
        for (slot, unit) in state.units.iter_mut().enumerate() {
            unit.pos = TilePos::new(1, 1).center()
                + chassis::fx::Vec2Fx::new(step * Fx::from_num(slot), Fx::ZERO);
        }
        let walker = id(&state, 5);
        state.units[5].order = Order::Run {
            goal: TilePos::new(0, 1).into(),
        };
        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
        Reach::new(&state).crowd_touches(&state, &index, walker, TilePos::new(1, 1))
    };
    assert!(run([0, 0, 0, 0]), "the walker's own crowd");
    assert!(run([1, 1, 1, 1]), "a teammate's crowd");
    assert!(
        run([2, 2, 0, 0]),
        "a friendly body inside the disk seeds it"
    );
    assert!(
        !run([2, 2, 2, 0]),
        "hostile bodies holding the whole disk seed nothing"
    );
    assert!(
        !run([0, 0, 2, 2]),
        "hostile bodies never link a friendly body to the disk"
    );
    assert!(
        !run([2, 2, 2, 2]),
        "nor does a hostile line reaching out from it"
    );
}

#[test]
fn a_hostile_line_ends_no_walk_whether_or_not_its_side_sees_it() {
    // Parked bodies run east from the endpoint in touching steps to one
    // of the walker's own, which the walker touches. Seen from that end
    // of the line, the endpoint lies beyond sight.
    const LINE: usize = 17;
    let endpoint = TilePos::new(1, 1);
    let run = |line_owner: u8, spotter: bool| {
        let mut units = vec![(line_owner, UnitKind::Sentinel, 1, 1); LINE];
        units.extend([(0, UnitKind::Sentinel, 1, 1); 2]);
        if spotter {
            units.push((0, UnitKind::Wisp, 1, 1));
        }
        let mut state = world_with_teams(
            &["................................"; 3],
            &[Some(0), Some(1)],
            &units,
        );
        let step = UnitKind::Sentinel.stats().radius * 2;
        for (slot, unit) in state.units.iter_mut().take(LINE + 2).enumerate() {
            unit.pos =
                endpoint.center() + chassis::fx::Vec2Fx::new(step * Fx::from_num(slot), Fx::ZERO);
        }
        state.units[LINE + 1].order = Order::Run {
            goal: TilePos::new(0, 1).into(),
        };
        state.refresh_vision();
        let walker = id(&state, LINE + 1);
        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
        let seen = state.vision(PlayerId(0)).visible(endpoint);
        let arrived = Reach::new(&state).crowd_touches(&state, &index, walker, endpoint);
        (seen, arrived)
    };
    assert_eq!(
        run(0, false),
        (true, true),
        "the side's own line ends the walk"
    );
    assert_eq!(
        run(1, false),
        (false, false),
        "a hostile line out of sight is not counted"
    );
    assert_eq!(
        run(1, true),
        (true, false),
        "nor is one in plain sight, so sight never changes the answer"
    );
}

#[test]
fn a_depletion_during_the_brain_phase_reopens_the_ground_for_later_walkers() {
    // The scrap node at (6, 2) seals the pocket at (5, 2). A scout routes
    // first and builds the ground labels with the node closed; the
    // harvester then chips the node's last scrap before the walker plans.
    let mut state = world(
        &[
            "...........",
            "....###....",
            "....#.s....",
            "....###....",
            "...........",
        ],
        1,
        &[
            (0, UnitKind::Sentinel, 0, 0),
            (0, UnitKind::Harvester, 7, 2),
            (0, UnitKind::Sentinel, 10, 4),
        ],
    );
    let node = TilePos::new(6, 2);
    let pocket = TilePos::new(5, 2);
    while state.map.extract_scrap(node).is_some_and(|left| left > 1) {}
    let ticks_per_scrap = UnitKind::Harvester
        .stats()
        .harvest
        .expect("a harvester harvests")
        .ticks_per_scrap;
    let (scout, harvester, walker) = (id(&state, 0), id(&state, 1), id(&state, 2));
    state.units[0].order = Order::Run {
        goal: TilePos::new(0, 4).into(),
    };
    state.units[1].order = Order::Harvest {
        node,
        anchor: None,
        retiring: false,
    };
    state.units[1].pos = crate::geometry::work_approach_point(
        node.offset(1, 0),
        node,
        (1, 1),
        UnitKind::Harvester.stats().radius,
    );
    state.units[1].progress = ticks_per_scrap - 1;
    state.units[2].order = Order::Run {
        goal: pocket.into(),
    };
    assert_eq!(state.tick % 2, 0, "premise: the brains run in id order");
    assert!(scout < harvester && harvester < walker);
    let report = state.tick(&[]);
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::NodeDepleted { pos } if *pos == node)),
        "premise: the harvester emptied the node this tick"
    );
    assert!(
        state.unit(scout).unwrap().path.is_some(),
        "premise: the scout routed before the node ran dry"
    );
    let unit = state.unit(walker).unwrap();
    assert_eq!(
        unit.order,
        Order::Run {
            goal: pocket.into()
        }
    );
    assert_eq!(unit.path.as_ref().map(|path| path.goal), Some(pocket));
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::OrderStalled { .. })),
        "nothing reports the pocket unreachable"
    );
}
