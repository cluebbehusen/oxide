//! Bots: command sources that read [`crate::State`] and emit
//! [`crate::PlayerCommand`]s, exactly like a mouse or the debug socket.
//!
//! Since 0.7 the brain is layered (the architecture both the scripted
//! and any learned policy share):
//!
//! ```text
//! Observation -> policy Intent -> Executive -> PlayerCommand[]
//! ```
//!
//! [`observation`] builds what a bot may know — omnisciently or
//! fog-honestly; [`Intent`]s are the policy's vocabulary; the
//! [`Executive`] owns army bookkeeping and lowers intents to commands.
//! The 0.6 rule-cascade bot survives as [`classic::Bot`], the benchmark
//! opponent every new tier must beat.

pub mod brain;
pub mod classic;
pub mod executive;
pub mod observation;
pub mod orient;
pub mod tiers;
pub mod utility;

pub use brain::Brain;
pub use classic::Bot;
pub use executive::{Army, ArmyId, ArmyState, Doctrine, Executive, Intent};
pub use observation::{BuildingObs, Observation, UnitObs};
pub use orient::Orientation;
pub use tiers::Difficulty;
pub use utility::{Dials, UtilityPolicy};
