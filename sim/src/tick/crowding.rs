//! Occupied work and firing positions share admission and yielding.
use crate::{State, Unit, UnitId};
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::{CARDINALS, TilePos};
use std::collections::BTreeSet;

pub(crate) fn compression() -> Fx {
    const { Fx::lit("0.65") }
}
pub(crate) fn spacing(a: &Unit, b: &Unit) -> Fx {
    let distance = a.kind.stats().radius + b.kind.stats().radius;
    if a.player == b.player {
        distance * compression()
    } else {
        distance
    }
}
pub(crate) fn claimed(state: &State, id: UnitId, point: Vec2Fx, retained: bool) -> bool {
    let pressure = Pressure::new(state, id);
    let (occupied, _, arriving) = pressure.rank(point, retained);
    occupied > 0 || arriving > 0
}

struct Neighbor {
    pos: Vec2Fx,
    destination: Option<Vec2Fx>,
    spacing: Fx,
    productive: bool,
    precedes: bool,
}
struct Pressure {
    neighbors: Vec<Neighbor>,
}
impl Pressure {
    fn new(state: &State, id: UnitId) -> Self {
        let unit = state.unit(id).expect("position owner");
        Self {
            neighbors: state
                .units()
                .iter()
                .filter(|other| {
                    other.id != id
                        && other.hp > 0
                        && !state.hostile(other.player, unit.player)
                        && other.domain() == unit.domain()
                })
                .map(|other| {
                    let destination = other
                        .path
                        .as_ref()
                        .map(|path| path.final_point.unwrap_or(path.goal.center()))
                        .filter(|&point| other.pos.dist_sq(point) <= const { Fx::lit("1.5625") });
                    Neighbor {
                        pos: other.pos,
                        destination,
                        spacing: spacing(unit, other),
                        productive: productive(state, other),
                        precedes: other.player != unit.player || other.id < id,
                    }
                })
                .collect(),
        }
    }
    fn rank(&self, point: Vec2Fx, retained: bool) -> (usize, usize, usize) {
        let mut occupied = 0;
        let mut arriving = 0;
        let mut bodies = 0;
        for other in &self.neighbors {
            let distance = other.spacing * other.spacing;
            bodies += usize::from(point.dist_sq(other.pos) < distance);
            occupied += usize::from(other.productive && point.dist_sq(other.pos) < distance);
            arriving += usize::from(
                (!retained || other.precedes)
                    && other
                        .destination
                        .is_some_and(|p| point.dist_sq(p) < distance),
            );
        }
        (occupied, bodies, arriving)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Position {
    pub goal: chassis::grid::TilePos,
    pub point: Vec2Fx,
}

/// Callers supply legal candidates in their geometric tie order and their own route predicate.
pub(crate) fn choose(
    state: &State,
    id: UnitId,
    mut candidates: Vec<Position>,
    center: Vec2Fx,
    open: impl Fn(chassis::grid::TilePos) -> bool,
    mut route: impl FnMut(chassis::grid::TilePos) -> Option<Vec<chassis::grid::TilePos>>,
) -> Option<crate::state::PathFollow> {
    let pressure = Pressure::new(state, id);
    candidates.sort_by_cached_key(|candidate| pressure.rank(candidate.point, false));
    if candidates
        .first()
        .is_some_and(|candidate| pressure.rank(candidate.point, false).0 > 0)
    {
        let unit = state.unit(id).expect("position owner");
        let mut waiting = candidates.clone();
        for candidate in &mut waiting {
            let outward = candidate.point - center;
            if outward != Vec2Fx::ZERO {
                candidate.point += outward
                    * ((unit.kind.stats().radius * 2 + const { Fx::lit("0.20") })
                        / outward.length());
                candidate.goal = chassis::grid::TilePos::containing(candidate.point);
            }
        }
        waiting.retain(|candidate| {
            crate::geometry::circle_clear(candidate.point, unit.kind.stats().radius, &open)
        });
        waiting.sort_by_cached_key(|candidate| {
            (
                pressure.rank(candidate.point, false),
                unit.pos.dist_sq(candidate.point),
            )
        });
        if !waiting.is_empty() {
            candidates = waiting;
        }
    }
    let mut failed = BTreeSet::new();
    let mut reachable = None;
    for candidate in candidates {
        if failed.contains(&candidate.goal) {
            continue;
        }
        if failed.len() == 4 && reachable.is_none() {
            let width = state.map().width();
            let height = state.map().height();
            let labels = chassis::path::cardinal_components(width, height, &open);
            let label = |tile: TilePos| {
                if tile.x < 0 || tile.y < 0 || tile.x >= width || tile.y >= height {
                    0
                } else {
                    labels[(tile.y * width + tile.x) as usize]
                }
            };
            let from = state.unit(id).expect("position owner").tile();
            let mut accessible = BTreeSet::new();
            let own = label(from);
            if own != 0 {
                accessible.insert(own);
            } else {
                for (dx, dy) in CARDINALS {
                    let next = label(from.offset(dx, dy));
                    if next != 0 {
                        accessible.insert(next);
                    }
                }
            }
            reachable = Some((labels, accessible));
        }
        if reachable.as_ref().is_some_and(|(labels, accessible)| {
            let tile = candidate.goal;
            tile.x < 0
                || tile.y < 0
                || tile.x >= state.map().width()
                || tile.y >= state.map().height()
                || !accessible.contains(&labels[(tile.y * state.map().width() + tile.x) as usize])
        }) {
            continue;
        }
        if let Some(mut waypoints) = route(candidate.goal) {
            if waypoints.is_empty() {
                waypoints.push(candidate.goal);
            }
            return Some(crate::state::PathFollow {
                goal: candidate.goal,
                final_point: Some(candidate.point),
                waypoints,
                next: 0,
            });
        }
        failed.insert(candidate.goal);
    }
    None
}
pub(crate) fn productive(state: &State, unit: &Unit) -> bool {
    if unit.kind.stats().turn_rate > 0 || !unit.work_stopped() {
        return false;
    }
    if unit.unloading.is_some() {
        return true;
    }
    match unit.order {
        crate::Order::Harvest {
            node,
            retiring: false,
            ..
        } => {
            unit.kind
                .stats()
                .harvest
                .is_some_and(|h| unit.carrying < h.capacity)
                && unit.in_work_reach(node, (1, 1))
        }
        crate::Order::Build { site } => state.in_building_work_reach(unit, site),
        crate::Order::Repair { building } | crate::Order::Salvage { building } => {
            state.in_building_work_reach(unit, building)
        }
        crate::Order::RepairUnit { unit: patient } => state
            .unit(patient)
            .is_some_and(|p| p.work_stopped() && unit.in_repair_reach(p)),
        crate::Order::Attack { target, .. } => {
            state.attack_view(unit.player, target).is_some_and(|view| {
                let aim = match view.entity {
                    Some(crate::Target::Building(id))
                        if unit.kind.stats().contact_reach.is_some() =>
                    {
                        state.building(id).map_or(view.position, |b| {
                            state.contact_surface(b).closest(unit.pos)
                        })
                    }
                    _ => view.aim_from(unit.pos),
                };
                unit.kind.stats().weapons.iter().any(|w| {
                    let full = unit.domain() == crate::stats::Domain::Ground
                        && view.domain == Some(crate::stats::Domain::Ground)
                        && !w.indirect;
                    let clear = |tile| {
                        state.map().tile(tile).is_some_and(|t| {
                            !t.terrain.blocks_all_fire()
                                && (!full || !t.terrain.blocks_direct_fire())
                        })
                    };
                    let range = unit.kind.stats().contact_reach.map_or(w.range, |reach| {
                        let target_radius = match view.entity {
                            Some(crate::Target::Unit(id)) => {
                                state.unit(id).map_or(Fx::ZERO, |u| u.kind.stats().radius)
                            }
                            _ => Fx::ZERO,
                        };
                        w.range
                            .min(unit.kind.stats().radius + target_radius + reach)
                    });
                    view.domain.is_some_and(|d| w.targets.covers(d))
                        && unit.pos.dist_sq(aim) <= range * range
                        && unit.pos.dist_sq(aim) >= w.minimum_range * w.minimum_range
                        && clear(chassis::grid::TilePos::containing(aim))
                        && !chassis::path::line_blocked(unit.pos, aim, clear)
                })
            })
        }
        _ => false,
    }
}

pub(crate) fn standable(state: &State, unit: &Unit, point: Vec2Fx) -> bool {
    crate::geometry::circle_clear(point, unit.kind.stats().radius, |tile| {
        state.passable_for(unit.domain(), tile)
    })
}

#[cfg(test)]
mod tests {
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
        assert!(claimed(&state, id, point, true));
        assert_eq!(
            Pressure::new(&state, id).neighbors[0].spacing,
            UnitKind::Harvester.stats().radius * 2
        );
        state.players[1].team = state.players[0].team.wrapping_add(1);
        assert!(!claimed(&state, id, point, false));
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
            "name":"split firing positions", "mode":"sandbox", "seed":42,
            "map":vec![".......^........";12],
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
}
