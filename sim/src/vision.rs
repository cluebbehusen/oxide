//! Fog of war: per-player visibility.
//!
//! Each player owns two boolean grids. `visible` is recomputed from scratch
//! every tick — the union of vision discs around that player's units and
//! buildings. `explored` only ever accumulates. Vision is radius-based;
//! rocks do not block line of sight (a deliberate simplification, cheap and
//! predictable).
//!
//! What fog *enforces* is deliberately narrow: targeted attack commands
//! require the issuer to see the victim. Everything else — what the shell
//! draws, what a player knows — is presentation reading these grids. The
//! built-in bot reads full state (a classic cheating AI), but the commands
//! it issues still pass the same validation as everyone else's.

use crate::ids::PlayerId;
use crate::state::State;
use chassis::grid::{Grid, TilePos};
use serde::{Deserialize, Serialize};

/// One player's view of the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vision {
    visible: Grid<bool>,
    explored: Grid<bool>,
}

impl Vision {
    pub(crate) fn new(width: i32, height: i32) -> Self {
        Self {
            visible: Grid::new(width, height, false),
            explored: Grid::new(width, height, false),
        }
    }

    /// Whether the player currently sees `pos`.
    pub fn visible(&self, pos: TilePos) -> bool {
        self.visible.get(pos).copied().unwrap_or(false)
    }

    /// Whether the player has ever seen `pos`.
    pub fn explored(&self, pos: TilePos) -> bool {
        self.explored.get(pos).copied().unwrap_or(false)
    }

    fn stamp_disc(&mut self, center: TilePos, radius: i32) {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                let pos = center.offset(dx, dy);
                if let Some(cell) = self.visible.get_mut(pos) {
                    *cell = true;
                }
                if let Some(cell) = self.explored.get_mut(pos) {
                    *cell = true;
                }
            }
        }
    }
}

/// Rebuilds every player's `visible` set from their live entities.
pub(crate) fn refresh(state: &mut State) {
    let mut vision = std::mem::take(&mut state.vision);
    for (index, view) in vision.iter_mut().enumerate() {
        let player = PlayerId(index as u8);
        view.visible.fill(false);
        for unit in state.units.iter().filter(|u| u.player == player) {
            view.stamp_disc(unit.tile(), unit.kind.stats().vision);
        }
        for building in state.buildings.iter().filter(|b| b.player == player) {
            let radius = building.kind.stats().vision;
            for tile in building.tiles() {
                view.stamp_disc(tile, radius);
            }
        }
    }
    state.vision = vision;
}
