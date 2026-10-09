//! Occupied work and firing positions share admission and yielding.
use crate::{State, Unit, UnitId};
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::{CARDINALS, TilePos};
use std::collections::BTreeSet;

pub(crate) fn spacing(a: &Unit, b: &Unit) -> Fx {
    let distance = a.kind.stats().radius + b.kind.stats().radius;
    if a.player == b.player {
        distance * crate::stats::SAME_OWNER_COMPRESSION
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

impl Pressure {
    /// The neighbors that can affect [`rank`](Self::rank) at any point
    /// within `reach` of `center`. A neighbor counts only through a body or
    /// a destination closer to the point than their spacing, and a counted
    /// destination lies within [`crate::stats::ARRIVAL_WINDOW`] of its body, so a body
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
                    let bound = reach
                        + spacing(unit, other)
                        + crate::stats::ARRIVAL_WINDOW
                        + const { Fx::lit("0.0625") };
                    other.pos.dist_sq(center) < bound * bound
                })
                .map(|other| {
                    let destination = other
                        .path
                        .as_ref()
                        .map(|path| path.final_point.unwrap_or(path.goal.center()))
                        .filter(|&point| {
                            other.pos.dist_sq(point)
                                <= crate::stats::ARRIVAL_WINDOW * crate::stats::ARRIVAL_WINDOW
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
    let push = unit.kind.stats().radius * 2 + crate::stats::WAITING_CLEARANCE;
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
                    labels[tile.row_major(width)]
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
                || !accessible.contains(&labels[tile.row_major(state.map().width())])
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
mod tests;
