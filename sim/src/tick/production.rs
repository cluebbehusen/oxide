//! Phase 2: Foundry production queues.
//!
//! One queue per building, front item in progress. A finished unit spawns on
//! the first passable ring tile (row-major scan — deterministic); if the
//! ring is fully blocked the unit waits at 100% until a tile opens up. A
//! rally point, when set, hands the newborn its first order.

use super::rect_adjacent_tiles;
use crate::event::Event;
use crate::ids::PlayerId;
use crate::state::{Order, State};
use crate::stats::UnitKind;
use chassis::grid::TilePos;

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    // Reclaimers trickle first: every built one grinds ambient debris
    // into a scrap each period. Order is building-id order (commutative
    // anyway — the credits are per-player sums).
    if state.tick.is_multiple_of(crate::stats::RECLAIMER_PERIOD) {
        let credits: Vec<PlayerId> = state
            .buildings
            .iter()
            .filter(|b| b.built && b.hp > 0 && b.kind == crate::stats::BuildingKind::Reclaimer)
            .map(|b| b.player)
            .collect();
        for player in credits {
            let bank = &mut state.player_mut(player).scrap;
            *bank = bank.saturating_add(1);
        }
    }

    let ids: Vec<_> = state.buildings.iter().map(|b| b.id).collect();
    for id in ids {
        let Some(b) = state.building_mut(id) else {
            continue;
        };
        if !b.built {
            continue; // a site's progress belongs to its builder
        }
        let Some(&kind) = b.queue.front() else {
            b.progress = 0;
            continue;
        };
        b.progress = (b.progress + 1).min(kind.stats().train_ticks);
        if b.progress < kind.stats().train_ticks {
            continue;
        }
        // Ready — look for a doorstep tile (any in-bounds tile serves a
        // flyer; the ground ring can be walled shut).
        let (anchor, size, player, rally) = (b.anchor, b.kind.stats().size, b.player, b.rally);
        let domain = kind.stats().domain;
        let spawn = rect_adjacent_tiles(anchor, size).find(|&t| state.passable_for(domain, t));
        let Some(tile) = spawn else {
            continue; // fully walled in; retry next tick
        };
        let unit = state.spawn_unit(player, kind, tile.center());
        events.push(Event::UnitTrained { unit, kind, player });
        if let Some(rally) = rally
            && let Some(order) = rally_order(state, player, kind, rally)
            && let Some(newborn) = state.unit_mut(unit)
        {
            newborn.order = order;
        }
        let b = state.building_mut(id).expect("still standing");
        b.queue.pop_front();
        b.progress = 0;
    }
}

/// What a rally means to a fresh unit: harvesters mine a rallied node,
/// fighters attack-move, everyone else walks. `None` (unwalkable rally
/// area) leaves the unit idle at the doorstep.
///
/// "Node" is judged by the owner's *remembered* scrap, not the live map —
/// it refreshes while the ground is visible and freezes when sight is
/// lost, so a rally can neither probe unexplored tiles nor know a distant
/// node ran dry. Stale beliefs resolve honestly: the newborn walks out
/// and discovers.
fn rally_order(state: &State, owner: PlayerId, kind: UnitKind, rally: TilePos) -> Option<Order> {
    let stats = kind.stats();
    if stats.harvest.is_some()
        && (state.vision(owner).remembered_scrap(rally) > 0
            || state.vision(owner).remembered_wreck(rally) > 0)
    {
        return Some(Order::Harvest { node: rally });
    }
    let goal = super::domain_goal(state, rally, stats.domain)?;
    Some(if stats.can_fight() {
        Order::AttackMove { goal }
    } else {
        Order::Move { goal }
    })
}
