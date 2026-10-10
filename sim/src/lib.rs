#![doc = include_str!("../README.md")]
// Floats and hash-ordered collections would break bit-identical replays.
#![deny(clippy::float_arithmetic, clippy::disallowed_types)]

pub mod building_contact;
pub mod command;
pub mod event;
pub mod geometry;
pub mod ids;
pub mod map;
pub mod observation;
pub mod scenario;
pub mod state;
pub mod stats;
mod tick;
pub mod vision;

pub use command::{Command, PlayerCommand};
pub use event::{Event, GroundMotion, StallReason, TickReport, UnitRepairSource};
pub use ids::{AttackTarget, BuildingId, ContactId, PlayerId, RememberedBuilding, Target, UnitId};
pub use scenario::Scenario;
pub use state::{
    Aim, Building, ExtractorIncome, Faction, GameResult, Goal, Leash, Order, OrderKey,
    PlaceRefusal, Player, Recovery, State, StateIntegrityError, Unit,
};
pub use stats::{BuildingKind, ProjectileKind, UnitKind};
pub use tick::CommandPhaseView;
pub use vision::{GhostBuilding, Vision};

/// Version stamped into replays, saves and checkpoints; a replay is only
/// guaranteed to reproduce on the sim version that recorded it. It tracks
/// simulation behavior and saved-state shape, independent of the package
/// version.
pub const SIM_VERSION: u32 = 1;

/// Fixed simulation rate. The shell converts wall time into ticks; the sim
/// itself only ever counts ticks.
pub const TICKS_PER_SECOND: u32 = 20;

/// Simulation time in ticks, re-exported from chassis.
pub type Tick = chassis::Tick;

/// Serde skip predicate: omits a field that still holds its default value.
pub(crate) fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
