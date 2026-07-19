//! Phase 2: Foundry production queues.
//!
//! One queue per building, front item in progress. A finished unit spawns on
//! the first passable ring tile (row-major scan — deterministic); if the
//! ring is fully blocked the unit waits at 100% until a tile opens up.

use super::rect_adjacent_tiles;
use crate::event::Event;
use crate::state::State;

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
        let (anchor, size, player) = (b.anchor, b.kind.stats().size, b.player);
        let spawn = rect_adjacent_tiles(anchor, size).find(|&t| state.passable(t));
        let Some(tile) = spawn else {
            continue; // fully walled in; retry next tick
        };
        let unit = state.spawn_unit(player, kind, tile.center());
        events.push(Event::UnitTrained { unit, kind, player });
        let b = state.building_mut(id).expect("still standing");
        b.queue.remove(0);
        b.progress = 0;
    }
}
