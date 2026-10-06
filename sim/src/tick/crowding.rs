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
    let pressure = Pressure::new(state, id, point, Fx::ZERO);
    let (occupied, _, arriving) = pressure.rank(point, retained);
    occupied > 0 || arriving > 0
}

struct Neighbor {
    pos: Vec2Fx,
    destination: Option<Vec2Fx>,
    spacing: Fx,
    productive: bool,
    /// For a unit of the same seat, whether its id comes first; unit ids
    /// order different seats unevenly, so they never rank across seats.
    earlier: Option<bool>,
}
struct Pressure {
    /// Where the asking unit stands.
    origin: Vec2Fx,
    neighbors: Vec<Neighbor>,
}
/// How far from its body a neighbor's destination may lie and still count
/// as a claim: the square root of the window in [`Pressure::new`].
const ARRIVAL_WINDOW: Fx = Fx::lit("1.25");

impl Pressure {
    /// The neighbors that can affect [`rank`](Self::rank) at any point
    /// within `reach` of `center`. A neighbor counts only through a body or
    /// a destination closer to the point than their spacing, and a counted
    /// destination lies within [`ARRIVAL_WINDOW`] of its body, so a body
    /// farther than `reach` plus both from `center` never counts.
    fn new(state: &State, id: UnitId, center: Vec2Fx, reach: Fx) -> Self {
        let unit = state.unit(id).expect("position owner");
        Self {
            origin: unit.pos,
            neighbors: state
                .units()
                .iter()
                .filter(|other| {
                    other.id != id
                        && other.hp > 0
                        && !state.hostile(other.player, unit.player)
                        && other.domain() == unit.domain()
                })
                .filter(|other| {
                    // Margin for fixed-point rounding in the squared distances.
                    let bound =
                        reach + spacing(unit, other) + ARRIVAL_WINDOW + const { Fx::lit("0.0625") };
                    other.pos.dist_sq(center) < bound * bound
                })
                .map(|other| {
                    let destination = other
                        .path
                        .as_ref()
                        .map(|path| path.final_point.unwrap_or(path.goal.center()))
                        .filter(|&point| {
                            other.pos.dist_sq(point) <= ARRIVAL_WINDOW * ARRIVAL_WINDOW
                        });
                    Neighbor {
                        pos: other.pos,
                        destination,
                        spacing: spacing(unit, other),
                        productive: productive(state, other),
                        earlier: (other.player == unit.player).then_some(other.id < id),
                    }
                })
                .collect(),
        }
    }
    /// Whether `other`, arriving at `destination`, outranks the asking
    /// unit's claim on `point`: the unit nearer its own position keeps it.
    /// Measuring each unit against its own position gives every ally a
    /// single rank, so two overlapping claims never each outrank the other
    /// and no cycle of units yields to the next. An exact tie goes to the
    /// earlier unit of one seat; across seats both yield, since nothing
    /// seat-fair separates them.
    fn precedes(&self, other: &Neighbor, destination: Vec2Fx, point: Vec2Fx) -> bool {
        let (theirs, ours) = (other.pos.dist_sq(destination), self.origin.dist_sq(point));
        theirs < ours || (theirs == ours && other.earlier.unwrap_or(true))
    }

    fn rank(&self, point: Vec2Fx, retained: bool) -> (usize, usize, usize) {
        let mut occupied = 0;
        let mut arriving = 0;
        let mut bodies = 0;
        for other in &self.neighbors {
            let distance = other.spacing * other.spacing;
            bodies += usize::from(point.dist_sq(other.pos) < distance);
            occupied += usize::from(other.productive && point.dist_sq(other.pos) < distance);
            arriving += usize::from(other.destination.is_some_and(|destination| {
                point.dist_sq(destination) < distance
                    && (!retained || self.precedes(other, destination, point))
            }));
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
    let unit = state.unit(id).expect("position owner");
    let push = unit.kind.stats().radius * 2 + const { Fx::lit("0.20") };
    // Waiting positions sit at most `push` beyond the candidates.
    let reach = candidates
        .iter()
        .map(|candidate| (candidate.point - center).length())
        .max()
        .unwrap_or(Fx::ZERO)
        + push;
    let pressure = Pressure::new(state, id, center, reach);
    candidates.sort_by_cached_key(|candidate| pressure.rank(candidate.point, false));
    if candidates
        .first()
        .is_some_and(|candidate| pressure.rank(candidate.point, false).0 > 0)
    {
        let mut waiting = candidates.clone();
        for candidate in &mut waiting {
            let outward = candidate.point - center;
            if outward != Vec2Fx::ZERO {
                candidate.point += outward * (push / outward.length());
                candidate.goal = crate::geometry::work_tile(candidate.point, unit.pos, center);
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
    /// position. Ranking by id within a seat but yielding across seats let
    /// each defer to the next in a cycle and none of them ever took it.
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
                        let offset = Vec2Fx::new(Fx::from_num(dx), Fx::from_num(dy))
                            * (reach / Fx::from_num(4));
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
}
