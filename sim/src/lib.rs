//! Oxide game rules: a pure, deterministic, headless RTS simulation.
//!
//! The entire game is [`State`] plus one function, [`State::tick`], which
//! consumes the commands stamped for that tick and advances the world by
//! exactly one fixed timestep. No rendering, no input handling, no wall
//! clock, no floats — given the same [`scenario::Scenario`] and the same
//! command log, two runs produce bit-identical states on any platform.
//! That property is load-bearing: replays, regression hashes, and the debug
//! tooling all assume it.
//!
//! The tick pipeline runs in a fixed order (commands → production → unit
//! brains → movement → separation → deaths → victory); see [`State::tick`]
//! for why the order matters.

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
pub use event::{Event, TickReport};
pub use ids::{BuildingId, PlayerId, Target, UnitId};
pub use scenario::Scenario;
pub use state::{Building, Faction, GameResult, Order, Player, State, Unit};
pub use stats::{BuildingKind, UnitKind};
pub use vision::Vision;

/// Version stamped into replays; a replay is only guaranteed to reproduce on
/// the sim version that recorded it.
pub const SIM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fixed simulation rate. The shell converts wall time into ticks; the sim
/// itself only ever counts ticks.
pub const TICKS_PER_SECOND: u32 = 20;

/// Simulation time in ticks, re-exported from chassis.
pub type Tick = chassis::Tick;
