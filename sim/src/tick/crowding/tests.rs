use super::*;
use crate::state::PathFollow;
use crate::{PlayerId, Scenario, UnitKind};
use chassis::grid::TilePos;

#[test]
fn allied_arrivals_claim_positions_with_full_cross_owner_spacing() {
    let mut state = Scenario::skirmish().build().unwrap();
    state.units.clear();
    state.players[1].team = state.players[0].team;
    let point = TilePos::new(15, 8).center();
    let id = state.spawn_unit(PlayerId(0), UnitKind::Harvester, point);
    let ally = state.spawn_unit(
        PlayerId(1),
        UnitKind::Harvester,
        point - Vec2Fx::new(Fx::lit("0.5"), Fx::ZERO),
    );
    state.unit_mut(ally).unwrap().path = Some(PathFollow {
        goal: TilePos::new(15, 8),
        final_point: Some(point),
        waypoints: vec![TilePos::new(15, 8)],
        next: 0,
    });
    assert!(claimed(&state, id, point, false));
    assert!(
        !claimed(&state, id, point, true),
        "the nearer body keeps a contested position"
    );
    state.unit_mut(id).unwrap().path = state.unit(ally).unwrap().path.clone();
    assert!(claimed(&state, ally, point, true));
    assert_eq!(
        Pressure::new(&state, id, point, Fx::ZERO).neighbors[0].spacing,
        UnitKind::Harvester.stats().radius * 2
    );
    state.players[1].team = state.players[0].team.wrapping_add(1);
    assert!(!claimed(&state, id, point, false));
}

/// Two workers of one seat and one of an allied seat converge on one
/// position. Ranking by id within a seat but yielding across seats would
/// let each defer to the next in a cycle so none of them ever took it.
#[test]
fn exactly_the_nearest_of_mixed_allied_arrivals_keeps_a_position() {
    let mut state = Scenario::skirmish().build().unwrap();
    state.units.clear();
    state.players[1].team = state.players[0].team;
    let point = TilePos::new(15, 8).center();
    let offset = |distance: &str| point - Vec2Fx::new(Fx::lit(distance), Fx::ZERO);
    let farthest = state.spawn_unit(PlayerId(0), UnitKind::Harvester, offset("1.0"));
    let nearest = state.spawn_unit(PlayerId(0), UnitKind::Harvester, offset("0.3"));
    let middle = state.spawn_unit(
        PlayerId(1),
        UnitKind::Harvester,
        point + Vec2Fx::new(Fx::lit("0.6"), Fx::ZERO),
    );
    for id in [farthest, nearest, middle] {
        state.unit_mut(id).unwrap().path = Some(PathFollow {
            goal: TilePos::new(15, 8),
            final_point: Some(point),
            waypoints: vec![TilePos::new(15, 8)],
            next: 0,
        });
    }
    let keeps: Vec<_> = [farthest, nearest, middle]
        .into_iter()
        .filter(|&id| !claimed(&state, id, point, true))
        .collect();
    assert_eq!(keeps, [nearest]);
}

#[test]
fn a_destination_claims_its_point_from_across_the_arrival_window() {
    let mut state = Scenario::skirmish().build().unwrap();
    state.units.clear();
    let point = TilePos::new(15, 8).center();
    let id = state.spawn_unit(PlayerId(0), UnitKind::Harvester, point);
    let arriving = state.spawn_unit(
        PlayerId(0),
        UnitKind::Harvester,
        point + Vec2Fx::new(Fx::lit("1.2"), Fx::ZERO),
    );
    state.unit_mut(arriving).unwrap().path = Some(PathFollow {
        goal: TilePos::new(15, 8),
        final_point: Some(point),
        waypoints: vec![TilePos::new(15, 8)],
        next: 0,
    });
    assert!(claimed(&state, id, point, false));
}

/// Two workers bound for overlapping positions after their paths
/// crossed: each stands nearer the other's position. Ranking each claim
/// by its own position keeps exactly one of them.
#[test]
fn crossed_arrivals_to_overlapping_positions_keep_exactly_one() {
    let mut state = Scenario::skirmish().build().unwrap();
    state.units.clear();
    let mine = TilePos::new(15, 8).center();
    let theirs = mine + Vec2Fx::new(Fx::lit("0.3"), Fx::ZERO);
    let farther = state.spawn_unit(
        PlayerId(0),
        UnitKind::Harvester,
        mine + Vec2Fx::new(Fx::lit("0.5"), Fx::ZERO),
    );
    let nearer = state.spawn_unit(
        PlayerId(0),
        UnitKind::Harvester,
        mine - Vec2Fx::new(Fx::lit("0.1"), Fx::ZERO),
    );
    for (id, point) in [(farther, mine), (nearer, theirs)] {
        state.unit_mut(id).unwrap().path = Some(PathFollow {
            goal: TilePos::containing(point),
            final_point: Some(point),
            waypoints: vec![TilePos::containing(point)],
            next: 0,
        });
    }
    assert!(claimed(&state, farther, mine, true));
    assert!(!claimed(&state, nearer, theirs, true));
}

#[test]
fn same_owner_precedence_is_unchanged_by_enemy_id_interleaving() {
    for enemy_count in [0, 7] {
        let mut state = Scenario::skirmish().build().unwrap();
        state.units.clear();
        let point = TilePos::new(15, 8).center();
        let first = state.spawn_unit(PlayerId(0), UnitKind::Harvester, point);
        for _ in 0..enemy_count {
            state.spawn_unit(
                PlayerId(1),
                UnitKind::Harvester,
                TilePos::new(20, 8).center(),
            );
        }
        let second = state.spawn_unit(PlayerId(0), UnitKind::Harvester, point);
        for id in [first, second] {
            state.unit_mut(id).unwrap().path = Some(PathFollow {
                goal: TilePos::new(15, 8),
                final_point: Some(point),
                waypoints: vec![TilePos::new(15, 8)],
                next: 0,
            });
        }
        assert!(!claimed(&state, first, point, true));
        assert!(claimed(&state, second, point, true));
    }
}

#[test]
fn only_nearby_arrivals_claim_a_position_across_body_sizes() {
    for kind in [
        UnitKind::Scuttler,
        UnitKind::Harvester,
        UnitKind::Excavator,
        UnitKind::Breaker,
        UnitKind::Buzzard,
    ] {
        let mut state = Scenario::skirmish().build().unwrap();
        state.units.clear();
        let point = TilePos::new(15, 8).center();
        let id = state.spawn_unit(PlayerId(0), kind, TilePos::new(9, 8).center());
        let other = state.spawn_unit(PlayerId(0), kind, TilePos::new(8, 8).center());
        state.unit_mut(other).unwrap().path = Some(PathFollow {
            goal: TilePos::new(15, 8),
            final_point: Some(point),
            waypoints: vec![TilePos::new(15, 8)],
            next: 0,
        });
        assert!(
            !claimed(&state, id, point, false),
            "distant {kind:?} reserved the destination"
        );
        state.unit_mut(other).unwrap().pos = point - Vec2Fx::new(Fx::lit("0.5"), Fx::ZERO);
        assert!(
            claimed(&state, id, point, false),
            "nearby {kind:?} lost its arrival claim"
        );
        state.unit_mut(other).unwrap().path = None;
        assert!(
            !claimed(&state, id, point, true),
            "idle traffic cancelled an admitted route"
        );
    }
}

#[test]
fn chooser_finds_a_farther_reachable_position_after_four_sealed_candidates() {
    let scenario: Scenario = serde_json::from_value(serde_json::json!({
        "name":"split firing positions", "mode":"sandbox", "map":vec![".......^........";12],
        "players":[{"name":"Local","faction":"ferrous","scrap":0,"bot":false}],
        "units":[{"player":0,"kind":"sentinel","x":3,"y":5}]
    }))
    .unwrap();
    let state = scenario.build().unwrap();
    let id = state.units()[0].id;
    let goals = [(9, 3), (9, 4), (9, 5), (9, 6), (4, 5)].map(|(x, y)| TilePos::new(x, y));
    let candidates = goals
        .into_iter()
        .map(|goal| Position {
            goal,
            point: goal.center(),
        })
        .collect();
    let mut searches = 0;
    let path = choose(
        &state,
        id,
        candidates,
        TilePos::new(10, 5).center(),
        |tile| state.passable(tile),
        |goal| {
            searches += 1;
            crate::tick::route_for(&state, UnitKind::Sentinel, TilePos::new(3, 5), goal)
        },
    )
    .unwrap();
    assert_eq!(path.goal, goals[4]);
    assert_eq!(searches, 5);
}

#[test]
fn chooser_prefers_free_space_and_still_accepts_an_occupied_fallback() {
    let mut state = Scenario::skirmish().build().unwrap();
    state.units.clear();
    let busy = TilePos::new(15, 8).center();
    let free = TilePos::new(15, 10).center();
    let id = state.spawn_unit(
        PlayerId(0),
        UnitKind::Scuttler,
        TilePos::new(10, 8).center(),
    );
    state.spawn_unit(PlayerId(0), UnitKind::Scuttler, busy);
    let position = |point| Position {
        goal: TilePos::containing(point),
        point,
    };
    let path = choose(
        &state,
        id,
        vec![position(busy), position(free)],
        TilePos::new(17, 8).center(),
        |_| true,
        |goal| Some(vec![goal]),
    )
    .unwrap();
    assert_eq!(path.final_point, Some(free));
    let path = choose(
        &state,
        id,
        vec![position(busy)],
        TilePos::new(17, 8).center(),
        |_| true,
        |goal| Some(vec![goal]),
    )
    .unwrap();
    assert_eq!(path.final_point, Some(busy));
}

/// Narrowing the neighbors to a query area must never change a rank
/// inside it.
#[test]
fn pressure_ranks_ignore_only_neighbors_beyond_reach() {
    let mut state = Scenario::skirmish().build().expect("skirmish builds");
    for _ in 0..120 {
        state.tick(&[]);
    }
    let everywhere = Fx::from_num(4096);
    let mut claims = 0;
    for unit in state.units().iter().filter(|unit| unit.hp > 0) {
        for reach in [Fx::ZERO, Fx::lit("1.5"), Fx::from_num(3)] {
            let center = unit.pos;
            let near = Pressure::new(&state, unit.id, center, reach);
            let all = Pressure::new(&state, unit.id, center, everywhere);
            for dy in -4..=4 {
                for dx in -4..=4 {
                    let offset =
                        Vec2Fx::new(Fx::from_num(dx), Fx::from_num(dy)) * (reach / Fx::from_num(4));
                    if offset.length() > reach {
                        continue;
                    }
                    let point = center + offset;
                    for retained in [false, true] {
                        let rank = near.rank(point, retained);
                        assert_eq!(rank, all.rank(point, retained), "{:?}", unit.id);
                        claims += usize::from(rank != (0, 0, 0));
                    }
                }
            }
        }
    }
    assert!(claims > 0, "no neighbor ever pressed a probe");
}
