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
mod tests {
    use super::*;

    #[test]
    fn a_reachable_goal_serializes_like_the_tile_it_replaced() {
        let tile = TilePos::new(7, -3);
        let mut goal = Goal::at(tile);
        goal.settle_for(tile);
        assert_eq!(goal.endpoint, None);
        assert_eq!(
            chassis::hash::state_hash(&goal),
            chassis::hash::state_hash(&tile)
        );
        assert_eq!(
            serde_json::to_string(&goal).unwrap(),
            serde_json::to_string(&tile).unwrap()
        );
        let parsed: Goal = serde_json::from_str(r#"{"x":7,"y":-3}"#).unwrap();
        assert_eq!(parsed, goal);
        assert_eq!(Goal::from(tile), goal);
        assert_eq!(Goal::slotted(tile, tile), goal);
    }

    #[test]
    fn an_endpoint_is_kept_only_when_it_differs_from_the_target() {
        let mut goal = Goal::at(TilePos::new(4, 4));
        assert!(!goal.short());
        goal.settle_for(TilePos::new(2, 4));
        assert_eq!(goal.endpoint, Some(TilePos::new(2, 4)));
        assert_eq!(goal.destination(), TilePos::new(2, 4));
        assert_eq!(goal.target(), TilePos::new(4, 4));
        assert!(goal.short());
        assert!(goal.canonical());
        goal.settle_for(TilePos::new(4, 4));
        assert_eq!(goal.endpoint, None);
        assert_eq!(goal.destination(), goal.tile());
        goal.endpoint = Some(goal.tile());
        assert!(!goal.canonical());
    }

    #[test]
    fn a_slot_redirects_the_target_but_keeps_the_clicked_tile() {
        let tile = TilePos::new(10, 5);
        let slot = TilePos::new(11, 5);
        let mut goal = Goal::slotted(tile, slot);
        assert_eq!(goal.aim, Aim::Slot(slot));
        assert_eq!(goal.tile(), tile);
        assert_eq!(goal.target(), slot);
        assert_eq!(goal.destination(), slot);
        assert!(goal.canonical());
        goal.settle_for(slot);
        assert_eq!(goal.endpoint, None, "the slot itself is no endpoint");
        goal.settle_for(tile);
        assert_eq!(goal.endpoint, Some(tile), "the clicked tile can be one");
        assert!(goal.canonical());

        let pending = Goal::pending(tile, 3, true);
        assert!(pending.is_pending());
        assert_eq!(pending.target(), tile);
        let json = serde_json::to_string(&pending).unwrap();
        assert_eq!(
            json,
            r#"{"x":10,"y":5,"aim":{"pending":{"rank":3,"reverse":true}}}"#
        );
        assert_eq!(serde_json::from_str::<Goal>(&json).unwrap(), pending);
        let json = serde_json::to_string(&Goal::slotted(tile, slot)).unwrap();
        assert_eq!(json, r#"{"x":10,"y":5,"aim":{"slot":{"x":11,"y":5}}}"#);
    }

    #[test]
    fn a_slot_on_the_clicked_tile_is_not_canonical() {
        let tile = TilePos::new(3, 3);
        let goal = Goal {
            aim: Aim::Slot(tile),
            ..Goal::at(tile)
        };
        assert!(!goal.canonical());
        let goal = Goal {
            endpoint: Some(TilePos::new(4, 3)),
            ..Goal::slotted(tile, TilePos::new(4, 3))
        };
        assert!(!goal.canonical(), "an endpoint on the slot");
    }

    #[test]
    fn adopting_a_reissue_keeps_the_endpoint_only_for_the_same_target() {
        let tile = TilePos::new(8, 8);
        let slot = TilePos::new(9, 8);
        let mut goal = Goal::slotted(tile, slot);
        goal.settle_for(TilePos::new(2, 2));
        let before = goal;

        goal.adopt(Goal::slotted(tile, slot));
        assert_eq!(goal, before, "the same slot keeps its endpoint");

        goal.adopt(Goal::pending(tile, 1, false));
        assert_eq!(
            goal.aim,
            Aim::Pending {
                rank: 1,
                reverse: false
            }
        );
        assert_eq!(goal.endpoint, None, "a new target resolves afresh");

        let mut pending = Goal::pending(tile, 0, false);
        pending.settle_for(TilePos::new(2, 2));
        pending.adopt(Goal::pending(tile, 4, true));
        assert_eq!(
            pending.aim,
            Aim::Pending {
                rank: 4,
                reverse: true
            }
        );
        assert_eq!(pending.endpoint, Some(TilePos::new(2, 2)));
    }
}
