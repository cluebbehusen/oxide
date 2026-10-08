//! Where a walking order is headed.

use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// A walking order's destination: the clicked tile, the spread slot the unit
/// takes around it, and the nearest reachable tile when that cannot be
/// reached.
///
/// Serializes as `{x, y}` while `aim` is [`Aim::Tile`] and `endpoint` is
/// `None`. That keeps an order whose clicked tile is its reachable target
/// byte-identical to the plain tile it replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    /// Column of the clicked tile.
    pub x: i32,
    /// Row of the clicked tile.
    pub y: i32,
    /// Which tile around the clicked one this unit is aiming for.
    #[serde(default, skip_serializing_if = "Aim::is_tile")]
    pub aim: Aim,
    /// The tile the unit settles for when [`Goal::target`] lies outside its
    /// reachable ground. Never equal to the target: a reachable target
    /// stores `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<TilePos>,
}

/// How a goal's clicked tile becomes the tile its unit aims for.
///
/// A group spreads over the open tiles around the clicked one, but only once
/// its owner's team has explored that tile: until then each member heads for
/// the clicked tile itself, and the order keeps what it needs to take its
/// slot later.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aim {
    /// The clicked tile itself.
    #[default]
    Tile,
    /// The clicked tile until it is explored, then the spread slot at
    /// `rank`, scanned in the frame `reverse` fixed when the order was
    /// issued. Every rank is valid; ranks past the last slot share it.
    Pending {
        /// The unit's place among its movement domain's half of the group.
        rank: u8,
        /// Whether the slot scan runs half-turned.
        reverse: bool,
    },
    /// A resolved spread slot. Never the clicked tile, which is
    /// [`Aim::Tile`].
    Slot(TilePos),
}

impl Aim {
    /// Whether this is the default aim at the clicked tile.
    pub fn is_tile(&self) -> bool {
        *self == Aim::Tile
    }
}

impl Goal {
    /// A goal on `tile` with no slot or endpoint.
    pub const fn at(tile: TilePos) -> Self {
        Self {
            x: tile.x,
            y: tile.y,
            aim: Aim::Tile,
            endpoint: None,
        }
    }

    /// A goal on `tile` that takes its spread slot once the tile is explored.
    pub(crate) const fn pending(tile: TilePos, rank: u8, reverse: bool) -> Self {
        Self {
            aim: Aim::Pending { rank, reverse },
            ..Self::at(tile)
        }
    }

    /// A goal on `tile` aiming for `slot`, keeping [`Aim::Tile`] whenever the
    /// slot is the tile itself.
    pub(crate) fn slotted(tile: TilePos, slot: TilePos) -> Self {
        Self {
            aim: if slot == tile {
                Aim::Tile
            } else {
                Aim::Slot(slot)
            },
            ..Self::at(tile)
        }
    }

    /// The clicked tile.
    pub const fn tile(&self) -> TilePos {
        TilePos::new(self.x, self.y)
    }

    /// The tile the order is trying to reach: its spread slot, or the
    /// clicked tile until it has one.
    pub const fn target(&self) -> TilePos {
        match self.aim {
            Aim::Slot(slot) => slot,
            Aim::Tile | Aim::Pending { .. } => self.tile(),
        }
    }

    /// Where the unit is actually walking: the endpoint when the target is
    /// out of reach, otherwise the target itself.
    pub fn destination(&self) -> TilePos {
        self.endpoint.unwrap_or(self.target())
    }

    /// Whether the goal is still waiting for its clicked tile to be
    /// explored.
    pub const fn is_pending(&self) -> bool {
        matches!(self.aim, Aim::Pending { .. })
    }

    /// Whether the unit is settling for a tile short of its target.
    pub(crate) fn short(&self) -> bool {
        self.destination() != self.target()
    }

    /// Stores `endpoint`, keeping `None` whenever it is the target itself.
    pub(crate) fn settle_for(&mut self, endpoint: TilePos) {
        self.endpoint = (endpoint != self.target()).then_some(endpoint);
    }

    /// Takes `new`'s clicked tile and aim, keeping the resolved endpoint
    /// only while the target it was resolved for is unchanged.
    pub(crate) fn adopt(&mut self, new: Goal) {
        let endpoint = self.endpoint.filter(|_| new.target() == self.target());
        *self = Goal { endpoint, ..new };
    }

    /// Whether the slot and endpoint obey the normalization rules.
    pub(crate) fn canonical(&self) -> bool {
        self.aim != Aim::Slot(self.tile()) && self.endpoint != Some(self.target())
    }
}

impl From<TilePos> for Goal {
    fn from(tile: TilePos) -> Self {
        Self::at(tile)
    }
}

#[cfg(test)]
mod tests;
