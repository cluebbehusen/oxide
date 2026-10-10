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
            .filter(|b| b.built() && state.player(b.player).team == team)
        {
            self.stamp(
                building.anchor,
                building.kind.size(),
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
mod tests;
