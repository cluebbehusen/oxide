#![doc = include_str!("../README.md")]

pub mod bench;
pub mod matchup;
pub mod perceptual;
pub mod playback;
pub mod render;
pub mod runner;
pub mod stats;

use oxide_sim::{PlayerCommand, Scenario};

/// Upper bound on the tick count an interactive surface will replay.
/// A syntactically valid file can claim an absurd duration and freeze
/// a UI for minutes; ~28 game-hours is beyond any honest session. The
/// headless driver may opt out.
pub const MAX_REPLAY_TICKS: u64 = 2_000_000;

/// The concrete session replay type every Oxide surface records,
/// saves, and replays.
pub type GameReplay = chassis::replay::Replay<Scenario, PlayerCommand>;
