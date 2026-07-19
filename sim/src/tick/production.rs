//! Phase 2: Foundry production queues.
//!
//! One queue per building, front item in progress. A finished unit spawns on
//! the first passable ring tile (row-major scan — deterministic); if the
//! ring is fully blocked the unit waits at 100% until a tile opens up. A
//! rally point, when set, hands the newborn its first order.

use super::{find_nearby_passable, rect_adjacent_tiles};
use crate::event::Event;
use crate::state::{Order, State};
use crate::stats::{GOAL_SNAP_RADIUS, UnitKind};
use chassis::grid::TilePos;

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    let ids: Vec<_> = state.buildings.iter().map(|b| b.id).collect();
    for id in ids {
        let Some(b) = state.building_mut(id) else {
            continue;
        };
        let Some(&kind) = b.queue.first() else {
            b.progress = 0;
            continue;
        };
        b.progress = (b.progress + 1).min(kind.stats().train_ticks);
        if b.progress < kind.stats().train_ticks {
            continue;
        }
        // Ready — look for a doorstep tile.
        let (anchor, size, player, rally) = (b.anchor, b.kind.stats().size, b.player, b.rally);
        let spawn = rect_adjacent_tiles(anchor, size).find(|&t| state.passable(t));
        let Some(tile) = spawn else {
            continue; // fully walled in; retry next tick
        };
        let unit = state.spawn_unit(player, kind, tile.center());
        events.push(Event::UnitTrained { unit, kind, player });
        if let Some(rally) = rally
            && let Some(order) = rally_order(state, kind, rally)
            && let Some(newborn) = state.unit_mut(unit)
        {
            newborn.order = order;
        }
        let b = state.building_mut(id).expect("still standing");
        b.queue.remove(0);
        b.progress = 0;
    }
}

/// What a rally means to a fresh unit: harvesters mine a rallied node,
/// fighters attack-move, everyone else walks. `None` (unwalkable rally
/// area) leaves the unit idle at the doorstep.
fn rally_order(state: &State, kind: UnitKind, rally: TilePos) -> Option<Order> {
    let stats = kind.stats();
    if stats.harvest.is_some() && state.map.scrap_at(rally) > 0 {
        return Some(Order::Harvest { node: rally });
    }
    let goal = find_nearby_passable(state, rally, GOAL_SNAP_RADIUS)?;
    Some(if stats.attack.is_some() {
        Order::AttackMove { goal }
    } else {
        Order::Move { goal }
    })
}
