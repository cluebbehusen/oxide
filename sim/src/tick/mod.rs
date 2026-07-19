//! The tick pipeline.
//!
//! Phase order is part of the sim's contract — changing it changes game
//! outcomes (and therefore every regression hash):
//!
//! 1. **Commands** — validate and apply this tick's [`PlayerCommand`]s.
//! 2. **Production** — Foundries advance queues and spawn finished units
//!    (before brains, so a fresh unit acts on its birth tick).
//! 3. **Brains** — each unit, in id order, turns intent into action:
//!    acquiring targets, pathing, attacking, extracting, depositing. Damage
//!    lands immediately, so lower ids shoot first; a unit at 0 hp no longer
//!    acts. Deterministic, documented, and how classic lockstep RTSes do it.
//! 4. **Movement** — units advance along their paths.
//! 5. **Collision** — overlapping bodies are pushed apart until they fit;
//!    units are solid to each other but never block tiles.
//! 6. **Cleanup** — entities at 0 hp are removed, with events.
//! 7. **Vision** — every player's fog-of-war visible set is rebuilt from
//!    their surviving entities (explored only accumulates).
//! 8. **Victory** — a player with no buildings is out; last standing wins.
//!
//! After [`GameResult`] is set the world freezes: ticks still count up (so
//! timelines stay aligned) but nothing moves and commands are ignored.

mod brain;
mod commands;
mod movement;
mod production;

use crate::command::PlayerCommand;
use crate::event::{Event, TickReport};
use crate::state::{GameResult, State};
use crate::stats::PATH_EXPANSION_CAP;
use chassis::grid::TilePos;
use chassis::path::astar;

impl State {
    /// Advances the world by one fixed timestep, applying `commands` (all
    /// stamped for this tick). The returned report is presentation data —
    /// dropping it never affects the sim.
    pub fn tick(&mut self, commands: &[PlayerCommand]) -> TickReport {
        let tick = self.tick;
        let mut events = Vec::new();
        if self.result.is_none() {
            commands::apply(self, commands, &mut events);
            production::run(self, &mut events);
            brain::run(self, &mut events);
            movement::run(self);
            movement::resolve_collisions(self);
            cleanup(self, &mut events);
            self.refresh_vision();
            victory(self, &mut events);
        }
        self.tick += 1;
        TickReport { tick, events }
    }
}

/// Removes entities that hit 0 hp this tick, reporting each.
fn cleanup(state: &mut State, events: &mut Vec<Event>) {
    for unit in state.units.iter().filter(|u| u.hp == 0) {
        events.push(Event::UnitDied {
            unit: unit.id,
            kind: unit.kind,
            player: unit.player,
            pos: unit.pos,
        });
    }
    state.units.retain(|u| u.hp > 0);

    for building in state.buildings.iter().filter(|b| b.hp == 0) {
        events.push(Event::BuildingDestroyed {
            building: building.id,
            player: building.player,
            pos: building.center(),
        });
    }
    state.buildings.retain(|b| b.hp > 0);
}

/// Declares the result once at least one player has been eliminated.
///
/// Elimination is building-based: no buildings, no comeback. A one-player
/// scenario never self-declares victory (nobody has been eliminated).
fn victory(state: &mut State, events: &mut Vec<Event>) {
    if state.result.is_some() {
        return;
    }
    let alive = |p: usize| state.buildings.iter().any(|b| b.player.0 as usize == p);
    let survivors: Vec<usize> = (0..state.players.len()).filter(|&p| alive(p)).collect();
    if survivors.len() == state.players.len() {
        return;
    }
    let result = match survivors.as_slice() {
        [] => GameResult::Draw,
        [winner] => GameResult::Victory {
            winner: crate::ids::PlayerId(*winner as u8),
        },
        _ => return, // multiple survivors — play on
    };
    state.result = Some(result);
    events.push(Event::GameOver { result });
}

/// A* against the current world (terrain + buildings).
pub(crate) fn astar_for(state: &State, from: TilePos, to: TilePos) -> Option<Vec<TilePos>> {
    astar(
        state.map.width(),
        state.map.height(),
        from,
        to,
        |p| state.passable(p),
        PATH_EXPANSION_CAP,
    )
}

/// The ring of tiles surrounding a rectangle, row-major (deterministic).
pub(crate) fn rect_adjacent_tiles(
    anchor: TilePos,
    size: (i32, i32),
) -> impl Iterator<Item = TilePos> {
    let (w, h) = size;
    (-1..=h).flat_map(move |dy| {
        (-1..=w)
            .map(move |dx| anchor.offset(dx, dy))
            .filter(move |t| {
                let inside = t.x > anchor.x - 1
                    && t.x < anchor.x + w
                    && t.y > anchor.y - 1
                    && t.y < anchor.y + h;
                !inside
            })
    })
}

/// Whether `tile` touches (including diagonally) but does not overlap the
/// rectangle at `anchor`.
pub(crate) fn tile_adjacent_to_rect(tile: TilePos, anchor: TilePos, size: (i32, i32)) -> bool {
    let (w, h) = size;
    let inside =
        tile.x >= anchor.x && tile.y >= anchor.y && tile.x < anchor.x + w && tile.y < anchor.y + h;
    if inside {
        return false;
    }
    tile.x >= anchor.x - 1
        && tile.y >= anchor.y - 1
        && tile.x <= anchor.x + w
        && tile.y <= anchor.y + h
}

/// The nearest passable tile to `goal` within `radius`, scanning rings
/// outward, row-major within a ring — a deterministic "snap to walkable".
pub(crate) fn find_nearby_passable(state: &State, goal: TilePos, radius: i32) -> Option<TilePos> {
    for r in 0..=radius {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let t = goal.offset(dx, dy);
                if state.passable(t) {
                    return Some(t);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ring_has_expected_size_and_order() {
        // 2x2 rect → 12-tile ring.
        let ring: Vec<TilePos> = rect_adjacent_tiles(TilePos::new(5, 5), (2, 2)).collect();
        assert_eq!(ring.len(), 12);
        assert_eq!(
            ring[0],
            TilePos::new(4, 4),
            "row-major: top-left corner first"
        );
        assert!(
            ring.iter()
                .all(|t| tile_adjacent_to_rect(*t, TilePos::new(5, 5), (2, 2)))
        );
    }

    #[test]
    fn adjacency_excludes_inside_and_far() {
        let anchor = TilePos::new(3, 3);
        assert!(!tile_adjacent_to_rect(TilePos::new(3, 3), anchor, (2, 2)));
        assert!(!tile_adjacent_to_rect(TilePos::new(4, 4), anchor, (2, 2)));
        assert!(tile_adjacent_to_rect(TilePos::new(2, 2), anchor, (2, 2)));
        assert!(tile_adjacent_to_rect(TilePos::new(5, 4), anchor, (2, 2)));
        assert!(!tile_adjacent_to_rect(TilePos::new(6, 4), anchor, (2, 2)));
    }
}
