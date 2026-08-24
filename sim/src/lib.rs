#![doc = include_str!("../README.md")]

pub mod bot;
pub mod command;
pub mod event;
pub mod ids;
pub mod map;
pub mod scenario;
pub mod state;
pub mod stats;
mod tick;
pub mod vision;

pub use command::{Command, PlayerCommand};
pub use event::{Event, StallReason, TickReport, UnitRepairSource};
pub use ids::{BuildingId, PlayerId, Target, UnitId};
pub use scenario::Scenario;
pub use state::{
    Building, Faction, GameResult, Leash, Order, PlaceRefusal, Player, State, StateIntegrityError,
    Unit,
};
pub use stats::{BuildingKind, UnitKind};
pub use tick::CommandPhaseView;
pub use vision::{GhostBuilding, Vision};

/// Version stamped into replays; a replay is only guaranteed to reproduce on
/// the sim version that recorded it.
pub const SIM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fixed simulation rate. The shell converts wall time into ticks; the sim
/// itself only ever counts ticks.
pub const TICKS_PER_SECOND: u32 = 20;

/// Simulation time in ticks, re-exported from chassis.
pub type Tick = chassis::Tick;
