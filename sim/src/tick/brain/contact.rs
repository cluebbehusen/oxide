//! Bounded final approaches shared by building work and contact weapons.
use super::locomotion::approach_rect;
use super::*;
use crate::geometry::rect_approach_key_from;
use crate::stats::Domain;
use crate::tick::crowding;
use crate::tick::route_for;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

pub(super) fn approach(state: &mut State, id: UnitId, building: BuildingId) -> bool {
    approach_building(state, id, building)
}

fn approach_building(state: &mut State, id: UnitId, building: BuildingId) -> bool {
    let b = state.building(building).expect("live target");
    let (anchor, size) = (b.anchor, b.kind.size());
    let unit = state.unit(id).expect("contact unit");
    let (from, pos, player, kind) = (unit.tile(), unit.pos, unit.player, unit.kind);
    if pos.dist_sq(crate::geometry::footprint_contact(pos, anchor, size))
        > const { Fx::lit("6.25") }
    {
        return approach_rect(state, id, anchor, size);
    }
    // A held position stays only while it is still free and admissible:
    // ground claimed beside it since it was chosen can leave the motor
    // unable to land there.
    if unit.path.as_ref().is_some_and(|path| {
        path.final_point.is_some_and(|point| {
            !crowding::claimed(state, id, point, true)
                && admissible(state, unit, b, path.goal, point)
        })
    }) {
        return true;
    }
    let frame = crate::tick::rect_approach_origin(state, player, from, anchor, size);
    let mut candidates: Vec<_> = crate::geometry::work_positions(
        anchor,
        size,
        clearance(unit),
        unit.kind.stats().radius * crate::stats::SAME_OWNER_COMPRESSION,
    )
    .into_iter()
    .filter(|entry| state.passable(crate::geometry::work_tile(*entry, pos, b.center())))
    .filter_map(|entry| {
        endpoint(state, id, building, entry).map(|point| crowding::Position {
            goal: crate::geometry::work_tile(entry, pos, b.center()),
            point,
        })
    })
    .collect();
    candidates.sort_by_key(|candidate| {
        (
            pos.dist_sq(candidate.point),
            rect_approach_key_from(from, frame, anchor, size, candidate.goal),
            (candidate.point.x - pos.x) * (frame.center().y - pos.y)
                - (candidate.point.y - pos.y) * (frame.center().x - pos.x),
        )
    });
    let path = crowding::choose(
        state,
        id,
        candidates,
        anchor.center()
            + Vec2Fx::new(Fx::from_num(size.0 - 1), Fx::from_num(size.1 - 1)) / Fx::from_num(2),
        |tile| state.passable(tile),
        |goal| route_for(state, kind, from, goal),
    );
    let reachable = path.is_some();
    state.unit_mut(id).expect("contact unit").path = path;
    reachable
}

pub(in crate::tick) fn clearance(unit: &crate::Unit) -> Fx {
    unit.kind.stats().contact_reach.map_or_else(
        || unit.kind.stats().radius + crate::stats::WORK_FOOTPRINT_GAP,
        |reach| unit.kind.stats().radius + reach - const { Fx::lit("0.05") },
    )
}

pub(in crate::tick) fn endpoint(
    state: &State,
    id: UnitId,
    building: BuildingId,
    entry: Vec2Fx,
) -> Option<Vec2Fx> {
    let unit = state.unit(id)?;
    let b = state.building(building)?;
    let point = state.contact_surface(b).stance(entry, clearance(unit));
    admissible(
        state,
        unit,
        b,
        crate::geometry::work_tile(entry, unit.pos, b.center()),
        point,
    )
    .then_some(point)
}

/// Whether a contact position can be driven to straight from its goal
/// tile's center, the way the motor finishes every final approach.
fn admissible(
    state: &State,
    unit: &crate::Unit,
    building: &crate::Building,
    goal: TilePos,
    point: Vec2Fx,
) -> bool {
    point.dist_sq(goal.center()) <= const { Fx::lit("2.25") }
        && state
            .ground_terrain()
            .with_contact(Some(state.contact_surface(building)))
            .contact_clear(goal.center(), point, collision_radius(unit))
}

pub(in crate::tick) fn collision_radius(unit: &crate::Unit) -> Fx {
    unit.kind
        .stats()
        .radius
        .min(clearance(unit) - const { Fx::lit("0.002") })
}

pub(in crate::tick) fn surface_for(
    state: &State,
    unit: &crate::Unit,
) -> Option<crate::building_contact::Surface> {
    if unit.domain() != Domain::Ground {
        return None;
    }
    let target = match unit.order {
        Order::Attack { target, .. } if unit.kind.stats().contact_reach.is_some() => state
            .attack_view(unit.player, target)
            .and_then(|view| match view.entity {
                Some(Target::Building(id)) => Some(id),
                _ => None,
            }),
        Order::Build { site } => Some(site),
        Order::Repair { building } | Order::Salvage { building } => Some(building),
        Order::ReturnCargo { foundry, .. } => Some(foundry),
        Order::Harvest { .. } if unit.carrying() > 0 => {
            unit.unloading().map(|u| u.foundry).or_else(|| {
                state
                    .buildings()
                    .iter()
                    .find(|b| {
                        b.hp > 0
                            && b.player == unit.player
                            && b.kind.is_drop_off()
                            && unit.path.as_ref().is_some_and(|p| {
                                crate::tick::tile_adjacent_to_rect(p.goal, b.anchor, b.kind.size())
                            })
                    })
                    .map(|b| b.id)
            })
        }
        _ => None,
    };
    let b = target
        .and_then(|id| state.building(id))
        .filter(|b| {
            b.hp > 0 && unit.pos.dist_sq(b.closest_point_to(unit.pos)) <= const { Fx::lit("2.25") }
        })
        .or_else(|| {
            (!state.passable(unit.tile()))
                .then(|| {
                    state
                        .buildings_at(unit.tile())
                        .find(|b| !b.kind.is_stealthy() && !b.provisional())
                })
                .flatten()
        })?;
    Some(state.contact_surface(b))
}

/// A worker that already reaches the surface stops where it stands instead
/// of finishing the walk to its chosen position, so a position it cannot
/// land on never keeps it from working.
pub(super) fn approach_worker(state: &mut State, id: UnitId, building: BuildingId) -> bool {
    let unit = state.unit(id).expect("contact unit");
    if state.in_building_work_reach(unit, building) {
        state.unit_mut(id).expect("contact unit").path = None;
        return true;
    }
    approach_building(state, id, building)
}
