#![doc = include_str!("../README.md")]

pub mod audit;
pub mod auto;
pub mod bot_eval;
pub mod client;
pub mod factorial;
pub mod pace;
pub mod pool;
pub mod profile;
pub mod replay_inspect;
pub mod replay_summary;
pub mod session;
pub mod shots;
pub mod smoke;
pub mod sweep;

// Shared with the shell via oxide-kit; re-exported so the driver's
// public surface (and its own `crate::render`-style paths) survive
// the split unchanged.
pub use oxide_kit::{playback, render, runner, stats};
