//! Entity identifiers.
//!
//! Ids are dense, monotonically assigned, and never reused within a run.
//! Entity vectors stay sorted by id, which makes id order *the* canonical
//! iteration order for every system — determinism rule 5.

use serde::{Deserialize, Serialize};

/// Identifies a unit for its whole life. Never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UnitId(pub u32);

/// Identifies a building for its whole life. Never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BuildingId(pub u32);

/// Index into [`crate::State::players`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u8);

/// Zero-based rank of a live unit among its owner's units in canonical id
/// order.
///
/// Global ids encode scenario insertion and cross-seat production order, so
/// using them directly to spread units across equivalent choices can give
/// mirrored seats different choices. An owner-local rank preserves the
/// canonical ordering within a seat without carrying that physical-seat
/// history into the choice.
pub(crate) fn owner_local_unit_rank(
    target: UnitId,
    owner: PlayerId,
    units: impl IntoIterator<Item = (UnitId, PlayerId)>,
) -> usize {
    let mut found = false;
    let mut rank = 0;
    for (id, player) in units {
        if id == target && player == owner {
            found = true;
        } else if player == owner && id < target {
            rank += 1;
        }
    }
    debug_assert!(found, "the ranked unit belongs to the supplied roster");
    rank
}

/// Something that can be attacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Target {
    /// An enemy unit.
    Unit(UnitId),
    /// An enemy building.
    Building(BuildingId),
}

impl core::fmt::Display for UnitId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "u{}", self.0)
    }
}

impl core::fmt::Display for BuildingId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "b{}", self.0)
    }
}

impl core::fmt::Display for PlayerId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "p{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_local_unit_rank_ignores_sparse_and_mixed_enemy_ids() {
        let units = [
            (UnitId(2), PlayerId(1)),
            (UnitId(4), PlayerId(0)),
            (UnitId(9), PlayerId(1)),
            (UnitId(20), PlayerId(0)),
            (UnitId(31), PlayerId(1)),
        ];

        assert_eq!(owner_local_unit_rank(UnitId(20), PlayerId(0), units), 1);
        assert_eq!(
            owner_local_unit_rank(UnitId(31), PlayerId(1), units.into_iter().rev()),
            2,
            "input iteration order cannot alter the canonical id rank"
        );
    }
}
