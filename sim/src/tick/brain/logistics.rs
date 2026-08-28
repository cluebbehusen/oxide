//! Transport logistics: boarding walks, the unload disgorge, and their
//! deferred resolution.
//!
//! Cargo lives OUTSIDE the world's unit list (see
//! [`crate::state::Unit::cargo`]), so the moment a machine embarks
//! nothing can see, target, or command it. But the brain phase decides
//! against the start-of-tick world — the unit list must hold still
//! under it, or every spatial-index slot behind the acting unit goes
//! stale. Board and unload therefore only BUFFER intent here, exactly
//! like damage; [`resolve`] mutates the list after the last brain has
//! decided.

use super::super::route_for;
use crate::event::{Event, StallReason};
use crate::ids::UnitId;
use crate::state::{Order, PathFollow, State, Unit};
use chassis::grid::TilePos;

/// Embarkations and landings one tick's brains asked for, applied by
/// [`resolve`] once the decision loop is over.
#[derive(Default)]
pub(in crate::tick) struct Pending {
    /// (rider, carrier) pairs within reach of a sling with room.
    boardings: Vec<(UnitId, UnitId)>,
    /// (carrier, drop point) pairs standing on their drop tile.
    landings: Vec<(UnitId, TilePos)>,
}

/// Total sling room a transport's current riders occupy.
fn cargo_load(transport: &Unit) -> u8 {
    transport
        .cargo
        .iter()
        .map(|u| u.kind.stats().transport_size)
        .sum()
}

/// Walk within [`crate::stats::LOAD_REACH`] of the carrier and ask to
/// climb aboard. A full sling, a dead carrier, or a carrier that
/// stopped being ours stands the boarder down where it is.
pub(super) fn board(
    state: &mut State,
    id: UnitId,
    transport: UnitId,
    pending: &mut Pending,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let (pos, tile, kind, player, my_size) = (
        unit.pos,
        unit.tile(),
        unit.kind,
        unit.player,
        unit.kind.stats().transport_size,
    );
    let Some(carrier) = state
        .unit(transport)
        .filter(|t| t.hp > 0 && t.player == player)
    else {
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    };
    let capacity = carrier.kind.stats().transport_capacity;
    let carrier_pos = carrier.pos;
    if cargo_load(carrier) + my_size > capacity {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::TransportFull,
        });
        return;
    }
    if pos.dist_sq(carrier_pos) <= crate::stats::LOAD_REACH * crate::stats::LOAD_REACH {
        // In reach: stop walking and ask for the sling. The list itself
        // must not change under the other brains, so the embark waits
        // for resolution.
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        pending.boardings.push((id, transport));
        return;
    }
    // Chase a standable tile inside boarding reach. Aircraft may hover over a
    // building, scrap, or blocked terrain that a ground rider can never occupy;
    // routing to the carrier's exact tile would reject an otherwise legal Load.
    let stale = state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_none_or(|p| {
            !state.passable(p.goal)
                || p.goal.center().dist_sq(carrier_pos)
                    > crate::stats::LOAD_REACH * crate::stats::LOAD_REACH
        });
    if !stale {
        return;
    }
    match boarding_route(state, kind, tile, carrier_pos) {
        Some((goal, waypoints)) => {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
        }
        None => {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.clear_program();
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
                reason: StallReason::NoRoute,
            });
        }
    }
}

/// Fewest-waypoint deterministic ground route to a standable tile within
/// boarding reach. A route that stays on the rider's current tile cannot help
/// when the exact rider and carrier positions are still too far apart, so it
/// is not a boarding route. Waypoint count wins, then `(y, x)`; a truly sealed
/// carrier still reports `NoRoute` through the caller.
fn boarding_route(
    state: &State,
    kind: crate::stats::UnitKind,
    from: TilePos,
    carrier_pos: chassis::fx::Vec2Fx,
) -> Option<(TilePos, Vec<TilePos>)> {
    let center = TilePos::containing(carrier_pos);
    let reach_sq = crate::stats::LOAD_REACH * crate::stats::LOAD_REACH;
    let mut routes = Vec::new();
    for y in center.y - 2..=center.y + 2 {
        for x in center.x - 2..=center.x + 2 {
            let goal = TilePos::new(x, y);
            if !state.passable(goal) || goal.center().dist_sq(carrier_pos) > reach_sq {
                continue;
            }
            if let Some(waypoints) = route_for(state, kind, from, goal)
                && !waypoints.is_empty()
            {
                routes.push((waypoints.len(), goal.y, goal.x, waypoints));
            }
        }
    }
    routes
        .into_iter()
        .min_by_key(|(length, y, x, _)| (*length, *y, *x))
        .map(|(_, y, x, waypoints)| (TilePos::new(x, y), waypoints))
}

/// Fly to the drop point; standing on it, ask to set the riders down.
pub(super) fn unload(
    state: &mut State,
    id: UnitId,
    at: TilePos,
    pending: &mut Pending,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let (pos, tile, kind, player) = (unit.pos, unit.tile(), unit.kind, unit.player);
    if tile == at {
        pending.landings.push((id, at));
        return;
    }
    let has_fresh_path = unit.path.as_ref().is_some_and(|p| p.goal == at);
    if has_fresh_path {
        return;
    }
    match route_for(state, kind, tile, at) {
        Some(waypoints) => {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                goal: at,
                waypoints,
                next: 0,
            });
        }
        None => {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.clear_program();
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
                reason: StallReason::NoRoute,
            });
        }
    }
}

/// Applies the tick's buffered embarkations and landings, after every
/// brain has decided and before anything moves. Buffers are re-sorted
/// by id: the brain loop alternates direction by tick parity, and the
/// list mutations here must not inherit that swing.
pub(in crate::tick) fn resolve(state: &mut State, mut pending: Pending, events: &mut Vec<Event>) {
    pending.boardings.sort_unstable_by_key(|&(rider, _)| rider);
    for (rider_id, transport) in pending.boardings {
        let Some(carrier) = state.unit(transport).filter(|t| t.hp > 0) else {
            continue; // the sling died mid-tick; the order retries next tick
        };
        let capacity = carrier.kind.stats().transport_capacity;
        let held = cargo_load(carrier);
        let carrier_pos = carrier.pos;
        // hp > 0 mirrors the carrier filter above: a rider dealt lethal
        // damage this same tick must die in cleanup, not be entombed in
        // the sling as a zero-hp corpse the death pass can no longer see.
        let Some(rider) = state.unit(rider_id).filter(|r| r.hp > 0) else {
            continue;
        };
        // Two boarders can clear the same last slot in one decision
        // pass; the first (by id) takes it and the other stalls out
        // through the ordinary full-sling arm next tick.
        if held + rider.kind.stats().transport_size > capacity {
            continue;
        }
        let slot = state
            .units
            .iter()
            .position(|u| u.id == rider_id)
            .expect("just seen");
        let mut rider = state.units.remove(slot);
        let player = rider.player;
        rider.order = Order::Idle;
        rider.queue.clear();
        rider.looping = false;
        rider.path = None;
        rider.leash = None;
        rider.settled = 0;
        rider.progress = 0;
        rider.pos = carrier_pos;
        let carrier = state.unit_mut(transport).expect("just seen");
        carrier.cargo.push(rider);
        events.push(Event::UnitBoarded {
            transport,
            unit: rider_id,
            player,
        });
    }

    pending
        .landings
        .sort_unstable_by_key(|&(carrier, _)| carrier);
    for (id, at) in pending.landings {
        let Some(carrier) = state.unit(id).filter(|t| t.hp > 0) else {
            continue;
        };
        let (pos, player) = (carrier.pos, carrier.player);
        // Claim drop tiles in the deterministic ring order every scan in
        // this sim uses — (chebyshev, y, x), center first.
        let mut open: Vec<TilePos> = Vec::new();
        for r in 0..=crate::stats::UNLOAD_SCAN_RADIUS {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = at.offset(dx, dy);
                    if state.passable(t) {
                        open.push(t);
                    }
                }
            }
        }
        let mut placed = 0usize;
        while placed < open.len() {
            let carrier = state.unit_mut(id).expect("just seen");
            if carrier.cargo.is_empty() {
                break;
            }
            let mut rider = carrier.cargo.remove(0);
            let spot = open[placed];
            rider.pos = spot.center();
            let rider_id = rider.id;
            // The unit list stays sorted by id: the rider's id predates
            // every machine spawned while it flew.
            let slot = state
                .units
                .iter()
                .position(|u| u.id > rider_id)
                .unwrap_or(state.units.len());
            state.units.insert(slot, rider);
            placed += 1;
            events.push(Event::UnitUnloaded {
                transport: id,
                unit: rider_id,
                player,
                at: spot,
            });
        }
        let carrier = state.unit_mut(id).expect("just seen");
        let stranded = !carrier.cargo.is_empty();
        carrier.advance_queue();
        if stranded {
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
                reason: StallReason::NoOpenGround,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scenario;
    use crate::scenario::UnitSpec;
    use crate::stats::UnitKind;
    use chassis::fx::{Fx, Vec2Fx};

    #[test]
    fn boarding_route_moves_when_tile_centers_are_misleadingly_close() {
        let mut scenario = Scenario::skirmish();
        scenario.name = "exact boarding reach".to_owned();
        scenario.map = vec![
            "############".to_owned(),
            "#1.........#".to_owned(),
            "#..........#".to_owned(),
            "#..........#".to_owned(),
            "#..........#".to_owned(),
            "#........2.#".to_owned(),
            "#..........#".to_owned(),
            "############".to_owned(),
        ];
        scenario.units = vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Skyhook,
                x: 6,
                y: 4,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 5,
                y: 4,
            },
        ];
        scenario.buildings.clear();
        scenario.meta = None;
        let mut state = scenario.build().expect("boarding arena builds");
        let transport = state
            .units()
            .iter()
            .find(|unit| unit.kind == UnitKind::Skyhook)
            .expect("scenario has a transport")
            .id;
        let rider = state
            .units()
            .iter()
            .find(|unit| unit.kind == UnitKind::Sentinel)
            .expect("scenario has a rider")
            .id;
        let carrier_pos = Vec2Fx::new(Fx::lit("6.99"), Fx::lit("4.5"));
        let rider_pos = Vec2Fx::new(Fx::lit("5.01"), Fx::lit("4.5"));
        state.unit_mut(transport).expect("transport lives").pos = carrier_pos;
        state.unit_mut(rider).expect("rider lives").pos = rider_pos;

        let rider_tile = TilePos::containing(rider_pos);
        let reach_sq = crate::stats::LOAD_REACH * crate::stats::LOAD_REACH;
        assert!(rider_tile.center().dist_sq(carrier_pos) <= reach_sq);
        assert!(rider_pos.dist_sq(carrier_pos) > reach_sq);

        let mut pending = Pending::default();
        let mut events = Vec::new();
        board(&mut state, rider, transport, &mut pending, &mut events);

        let path = state
            .unit(rider)
            .and_then(|unit| unit.path.as_ref())
            .expect("the rider receives a real approach path");
        assert_ne!(path.goal, rider_tile);
        assert!(!path.waypoints.is_empty());
        assert!(events.is_empty());
        assert!(pending.boardings.is_empty());
    }
}
