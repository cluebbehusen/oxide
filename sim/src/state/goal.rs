//! Where a walking order is headed.

use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// A walking order's destination: the commanded tile, plus the nearest
/// reachable tile when the commanded one cannot be reached.
///
/// Serializes as `{x, y}` while `endpoint` is `None`, which is every order a
/// command writes and every order whose target is reachable. That keeps
/// reachable orders byte-identical to the plain tile they replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    /// Column of the commanded tile.
    pub x: i32,
    /// Row of the commanded tile.
    pub y: i32,
    /// The tile the unit settles for when [`Goal::target`] lies outside its
    /// reachable ground. Never equal to the target: a reachable target
    /// stores `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<TilePos>,
}

impl Goal {
    /// A goal on `tile` with no endpoint.
    pub const fn at(tile: TilePos) -> Self {
        Self {
            x: tile.x,
            y: tile.y,
            endpoint: None,
        }
    }

    /// The commanded tile.
    pub const fn tile(&self) -> TilePos {
        TilePos::new(self.x, self.y)
    }

    /// The tile the order is trying to reach.
    pub const fn target(&self) -> TilePos {
        self.tile()
    }

    /// Where the unit is actually walking: the endpoint when the target is
    /// out of reach, otherwise the target itself.
    pub fn destination(&self) -> TilePos {
        self.endpoint.unwrap_or(self.target())
    }

    /// Whether the unit is settling for a tile short of its target.
    pub(crate) fn short(&self) -> bool {
        self.destination() != self.target()
    }

    /// Stores `endpoint`, keeping `None` whenever it is the target itself.
    pub(crate) fn settle_for(&mut self, endpoint: TilePos) {
        self.endpoint = (endpoint != self.target()).then_some(endpoint);
    }

    /// Whether the stored endpoint obeys the normalization rule.
    pub(crate) fn canonical(&self) -> bool {
        self.endpoint != Some(self.target())
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
}
