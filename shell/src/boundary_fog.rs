//! Presentation-only sight and exploration for the decorative off-map quarry.

use chassis::grid::{Grid, TilePos};
use oxide_sim::{PlayerId, State};

// The terraces and their vignette extend at most six tiles from the floor.
const PAD: i32 = 8;
const EXPLORED: u8 = 1;
const VISIBLE: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BoundaryFog {
    width: i32,
    height: i32,
    cells: Grid<u8>,
    visible: Vec<TilePos>,
}

impl BoundaryFog {
    pub(crate) fn valid_checkpoint(&self, state: &State) -> bool {
        self.width == state.map().width()
            && self.height == state.map().height()
            && self.cells.is_consistent()
            && self.cells.width() == self.width + PAD * 2
            && self.cells.height() == self.height + PAD * 2
            && self
                .cells
                .iter()
                .all(|(_, flags)| *flags == 0 || *flags == EXPLORED || *flags == EXPLORED | VISIBLE)
            && self.visible.iter().all(|pos| {
                self.cells
                    .get(*pos)
                    .is_some_and(|flags| *flags & VISIBLE != 0)
            })
            && self
                .visible
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.visible.len()
            && self.visible.len()
                == self
                    .cells
                    .iter()
                    .filter(|(_, flags)| **flags & VISIBLE != 0)
                    .count()
    }

    pub(crate) fn new(state: &State, viewer: PlayerId) -> Self {
        let width = state.map().width();
        let height = state.map().height();
        let mut fog = Self {
            width,
            height,
            cells: Grid::new(width + PAD * 2, height + PAD * 2, 0),
            visible: Vec::new(),
        };
        fog.observe(state, viewer);
        fog
    }

    pub(crate) fn observe(&mut self, state: &State, viewer: PlayerId) {
        for pos in self.visible.drain(..) {
            *self.cells.get_mut(pos).expect("previously stamped cell") &= EXPLORED;
        }
        let team = state.player(viewer).team;
        for unit in state
            .units()
            .iter()
            .filter(|u| state.player(u.player).team == team)
        {
            self.stamp(unit.tile(), (1, 1), unit.kind.stats().vision);
        }
        for building in state
            .buildings()
            .iter()
            .filter(|b| b.built && state.player(b.player).team == team)
        {
            self.stamp(
                building.anchor,
                building.stats().size,
                building.stats().vision,
            );
        }
    }

    fn stamp(&mut self, anchor: TilePos, size: (i32, i32), radius: i32) {
        let far = anchor.offset(size.0 - 1, size.1 - 1);
        if anchor.x >= radius
            && anchor.y >= radius
            && far.x + radius < self.width
            && far.y + radius < self.height
        {
            return;
        }
        for y in (anchor.y - radius).max(-PAD)..=(far.y + radius).min(self.height + PAD - 1) {
            for x in (anchor.x - radius).max(-PAD)..=(far.x + radius).min(self.width + PAD - 1) {
                if (0..self.width).contains(&x) && (0..self.height).contains(&y) {
                    continue;
                }
                let dx = (anchor.x - x).max(x - far.x).max(0);
                let dy = (anchor.y - y).max(y - far.y).max(0);
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                let pos = TilePos::new(x + PAD, y + PAD);
                let cell = self.cells.get_mut(pos).expect("clipped boundary cell");
                if *cell & VISIBLE == 0 {
                    self.visible.push(pos);
                }
                *cell = EXPLORED | VISIBLE;
            }
        }
    }

    pub(crate) fn visible(&self, tile: TilePos) -> bool {
        self.flags(tile) & VISIBLE != 0
    }

    pub(crate) fn explored(&self, tile: TilePos) -> bool {
        self.flags(tile) & EXPLORED != 0
    }

    fn flags(&self, tile: TilePos) -> u8 {
        self.cells.get(tile.offset(PAD, PAD)).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;
    use macroquad::prelude::vec2;
    use oxide_sim::{Command, PlayerCommand, Scenario};

    #[test]
    fn boundary_sight_matches_simulation_discs_on_an_expanded_map() {
        for shared_sight in [false, true] {
            let mut scenario = Scenario::skirmish();
            if shared_sight {
                for player in &mut scenario.players {
                    player.team = Some(0);
                }
                let mut enemy = scenario.players[1].clone();
                enemy.team = Some(1);
                scenario.players.push(enemy);
                scenario.map[12].replace_range(2..3, "3");
            }
            let state = scenario.build().unwrap();
            let hash = state.hash();
            let fog = BoundaryFog::new(&state, PlayerId(0));
            let width = state.map().width();
            let height = state.map().height();
            let blank = ".".repeat((width + PAD * 2) as usize);
            let padding = ".".repeat(PAD as usize);
            let mut rows = vec![blank.clone(); PAD as usize];
            rows.extend(
                scenario
                    .map
                    .iter()
                    .map(|row| format!("{padding}{row}{padding}")),
            );
            rows.extend(vec![blank; PAD as usize]);
            scenario.map = rows;
            for unit in &mut scenario.units {
                unit.x += PAD;
                unit.y += PAD;
            }
            for building in &mut scenario.buildings {
                building.x += PAD;
                building.y += PAD;
            }
            let expanded = scenario.build().unwrap();
            for y in -PAD..height + PAD {
                for x in -PAD..width + PAD {
                    if (0..width).contains(&x) && (0..height).contains(&y) {
                        continue;
                    }
                    let tile = TilePos::new(x, y);
                    assert_eq!(
                        fog.visible(tile),
                        expanded.vision(PlayerId(0)).visible(tile.offset(PAD, PAD)),
                        "{tile}, shared sight: {shared_sight}"
                    );
                    assert_eq!(fog.explored(tile), fog.visible(tile));
                }
            }
            assert_eq!(state.hash(), hash);
        }
    }

    #[test]
    fn grazing_sight_reveals_only_the_near_boundary_depth() {
        let state = Scenario::skirmish().build().unwrap();
        let mut fog = BoundaryFog::new(&state, PlayerId(0));
        fog.cells.fill(0);
        fog.visible.clear();
        fog.stamp(TilePos::new(15, 3), (1, 1), 4);
        assert!(fog.visible(TilePos::new(15, -1)));
        assert!(!fog.visible(TilePos::new(15, -2)));
        assert!(!fog.visible(TilePos::new(14, -1)));
        assert!(!fog.visible(TilePos::new(15, -100)));
        fog.stamp(TilePos::new(15, 1), (1, 1), 4);
        assert!(fog.visible(TilePos::new(15, -3)));
        assert!(fog.visible(TilePos::new(14, -2)));
        assert!(!fog.visible(TilePos::new(14, -3)));
    }

    #[test]
    fn boundary_exploration_survives_movement_bulk_ticks_and_resume() {
        let mut scenario = Scenario::skirmish();
        scenario.map = vec!["........................................".to_string(); 40];
        scenario.map[20].replace_range(20..21, "1");
        scenario.map[30].replace_range(30..31, "2");
        scenario.units.retain(|unit| unit.player == 0);
        scenario.units.truncate(1);
        scenario.units[0].x = 3;
        scenario.units[0].y = 3;
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        game.bots.clear();
        let tile = TilePos::new(3, -1);
        assert!(game.presentation.boundary_fog.visible(tile));
        let id = game.state.units()[0].id;
        game.pending.push(PlayerCommand {
            player: game.presentation.human,
            command: Command::Move {
                units: vec![id],
                goal: TilePos::new(15, 15),
                queue: false,
            },
        });
        game.advance_ticks(300);
        assert!(!game.presentation.boundary_fog.visible(tile));
        assert!(game.presentation.boundary_fog.explored(tile));
        let mut replay = game.recorder.clone();
        replay.meta.ticks = Some(game.state.current_tick());
        let resumed = Game::from_replay(replay).unwrap();
        assert_eq!(game.hash_hex(), resumed.hash_hex());
        assert_eq!(
            game.presentation.boundary_fog,
            resumed.presentation.boundary_fog
        );
    }
}
