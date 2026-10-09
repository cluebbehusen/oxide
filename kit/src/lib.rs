#![doc = include_str!("../README.md")]

pub mod bench;
pub mod bot_execution;
pub mod checkpoint;
pub mod controller;
pub mod perceptual;
pub mod playback;
pub mod recording;
pub mod render;
mod replay;
pub mod runner;
pub mod stats;

use oxide_sim::{PlayerCommand, Scenario};

/// Upper bound on the ticks a replay may span before loading or running it
/// is refused (about 28 game-hours). A syntactically valid file can claim an
/// absurd duration and freeze a UI for minutes. The headless driver may opt
/// out.
pub const MAX_REPLAY_TICKS: u64 = 2_000_000;

/// The concrete session replay type every Oxide surface records,
/// saves, and replays.
pub type GameReplay = chassis::replay::Replay<Scenario, PlayerCommand, recording::WorldOrigin>;

pub use replay::{ReplayPlayback, bounded_replay_duration, load_replay, replay_duration};

pub mod recovery;

pub mod diagnostics;
