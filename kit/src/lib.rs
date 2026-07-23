//! Shared engine-side toolkit for Oxide's shell and driver.
//!
//! Everything here serves both the game (`oxide-shell`) and the dev
//! harness (`oxide-driver`) without belonging to either: the headless
//! [`runner`] that executes scenarios and replays at full speed, the
//! replay [`playback`] engine behind the viewer and the CLI, the
//! [`stats`] extractor behind post-match screens and `replay-stats`,
//! and the CPU [`render`]er (tiny-skia, pixel-identical on every
//! machine) behind golden-image tests and map previews. The split
//! exists so the shell never depends on the dev harness.

pub mod bench;
pub mod composition;
pub mod matchup;
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
