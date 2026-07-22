//! The tick pipeline.
//!
//! Phase order is part of the sim's contract — changing it changes game
//! outcomes (and therefore every regression hash):
//!
//! 1. **Commands** — validate and apply this tick's [`PlayerCommand`]s.
//! 2. **Production** — Foundries advance queues and spawn finished units
//!    (before brains, so a fresh unit acts on its birth tick).
//! 3. **Brains** — each unit, in id order, turns intent into action:
//!    acquiring targets, pathing, attacking, extracting, depositing. Shots
//!    are *buffered*, not applied — every machine decides against the same
//!    start-of-tick world, so seat order grants no reaction edge and
//!    mutual kills are possible.
//! 4. **Resolution** — buffered damage lands (decision order), then
//!    surviving victims retaliate against their earliest attacker.
//! 5. **Movement** — units advance along their paths.
//! 6. **Collision** — overlapping bodies are pushed apart until they fit;
//!    units are solid to each other but never block tiles.
//! 7. **Cleanup** — entities at 0 hp are removed, with events; every
//!    death deposits wreck salvage on its ground.
//! 8. **Decay** — on its global cadence, every wreck tile loses one
//!    salvage (deposits land first, so a fresh wreck survives its birth
//!    tick).
//! 9. **Vision** — every player's fog-of-war visible set is rebuilt from
//!    their surviving entities (explored only accumulates).
//! 10. **Victory** — a player with no Foundry is out; last standing wins.
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
            if self.tick.is_multiple_of(crate::stats::WRECK_DECAY_TICKS) {
                self.map.decay_wrecks();
            }
            self.refresh_vision();
            victory(self, &mut events);
        }
        self.tick += 1;
        TickReport { tick, events }
    }
}

/// Removes entities that hit 0 hp this tick, reporting each — and leaves
/// their price on the ground: a fraction of every destroyed machine's
/// cost lands as wreck salvage (buildings split theirs across the
/// footprint). Battles literally feed the salvagers.
fn cleanup(state: &mut State, events: &mut Vec<Event>) {
    let mut deposits: Vec<(TilePos, u32)> = Vec::new();
    for unit in state.units.iter().filter(|u| u.hp == 0) {
        events.push(Event::UnitDied {
            unit: unit.id,
            kind: unit.kind,
            player: unit.player,
            pos: unit.pos,
        });
        let value =
            unit.kind.stats().cost * crate::stats::WRECK_VALUE_NUM / crate::stats::WRECK_VALUE_DEN;
        deposits.push((unit.tile(), value));
    }
    state.units.retain(|u| u.hp > 0);

    for building in state.buildings.iter().filter(|b| b.hp == 0) {
        events.push(Event::BuildingDestroyed {
            building: building.id,
            player: building.player,
            pos: building.center(),
        });
        let stats = building.kind.stats();
        let price = stats
            .construction
            .map_or(crate::stats::FOUNDRY_WRECK_VALUE, |c| c.cost);
        let value = price * crate::stats::WRECK_VALUE_NUM / crate::stats::WRECK_VALUE_DEN;
        let tiles = (stats.size.0 * stats.size.1) as u32;
        for tile in building.tiles() {
            deposits.push((tile, value / tiles));
        }
    }
    state.buildings.retain(|b| b.hp > 0);

    for (tile, value) in deposits {
        // A tile under a surviving building swallows its deposit — a
        // flyer downed over a roof leaves nothing strippable, and wreck
        // must never coexist with a standing footprint (harvesters
        // cannot reach it, and the building's own eventual wreck would
        // double-stack). Buildings that died this tick are already gone
        // from the vec, so their footprints take deposits normally.
        if state.buildings.iter().any(|b| b.contains(tile)) {
            continue;
        }
        // Rock never opens up, so salvage there is bait no harvester
        // can ever strip — a downed flyer's value is simply lost. Scrap
        // node tiles keep their deposits: they become standable the
        // moment the node exhausts.
        if state
            .map
            .tile(tile)
            .is_none_or(|t| t.terrain == crate::map::Terrain::Rock)
        {
            continue;
        }
        state.map.add_wreck(tile, value);
    }
}

/// Declares the result once at least one team has been eliminated.
///
/// Elimination is Foundry-based: a team lives while *any* of its seats
/// holds a Foundry — no Foundry anywhere, no comeback; turrets and
/// factories left standing don't keep a team in the game (or 0.5's
/// buildable kinds would have silently rewritten the victory rule).
/// The per-seat command gate in `commands::apply` deliberately stays
/// player-scoped: a foundry-less seat on a living team spectates while
/// its team plays on.
fn victory(state: &mut State, events: &mut Vec<Event>) {
    if state.result.is_some() {
        return;
    }
    let mut teams: Vec<u8> = state.players.iter().map(|p| p.team).collect();
    teams.sort_unstable();
    teams.dedup();
    let alive = |team: u8| {
        state.buildings.iter().any(|b| {
            b.kind == crate::stats::BuildingKind::Foundry
                && state.players[b.player.0 as usize].team == team
        })
    };
    let survivors: Vec<u8> = teams.iter().copied().filter(|&t| alive(t)).collect();
    if survivors.len() == teams.len() {
        return;
    }
    let result = match survivors.as_slice() {
        [] => GameResult::Draw,
        [team] => GameResult::Victory { team: *team },
        _ => return, // multiple teams standing — play on
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

/// A route for a unit of the given kind: ground units A* around the
/// world, air units fly the straight line — one waypoint, landed exactly.
pub(crate) fn route_for(
    state: &State,
    kind: crate::stats::UnitKind,
    from: TilePos,
    to: TilePos,
) -> Option<Vec<TilePos>> {
    match kind.stats().domain {
        crate::stats::Domain::Ground => astar_for(state, from, to),
        crate::stats::Domain::Air => Some(vec![to]),
    }
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

/// The tile a commanded goal actually means to one movement domain:
/// ground snaps to the nearest walkable tile, air clamps onto the map —
/// any tile flies, rock included.
pub(crate) fn domain_goal(
    state: &State,
    goal: TilePos,
    domain: crate::stats::Domain,
) -> Option<TilePos> {
    match domain {
        crate::stats::Domain::Ground => {
            find_nearby_passable(state, goal, crate::stats::GOAL_SNAP_RADIUS)
        }
        crate::stats::Domain::Air => Some(TilePos::new(
            goal.x.clamp(0, state.map.width() - 1),
            goal.y.clamp(0, state.map.height() - 1),
        )),
    }
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
