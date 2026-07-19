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
