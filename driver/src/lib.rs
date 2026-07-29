//! The driver: Oxide's test harness and remote control.
//!
//! Three capabilities, all headless-friendly:
//!
//! - [`runner`] (from `oxide-kit`) executes scenarios and replays at full
//!   speed with no window, the workhorse behind integration tests and CI.
//! - [`render`] draws any sim state to a PNG with a CPU rasterizer
//!   (tiny-skia) — pixel-identical on every machine, which is what lets
//!   golden-image tests compare bytes instead of tuning tolerances. The GPU
//!   never enters the picture.
//! - [`client`] speaks the debug protocol to a live shell over TCP.
//! - [`session`] *serves* that same protocol windowless: a persistent
//!   headless match every `driver live` verb can drive without a GPU.
//!
//! The binary (`oxide-driver`) wraps them all in a CLI; see README.

pub mod audit;
pub mod auto;
pub mod balance;
pub mod client;
pub mod factorial;
pub mod gym;
pub mod pace;
pub mod pool;
pub mod session;
pub mod shots;
pub mod smoke;
pub mod sweep;

// Shared with the shell via oxide-kit; re-exported so the driver's
// public surface (and its own `crate::render`-style paths) survive
// the split unchanged.
pub use oxide_kit::{playback, render, runner, stats};
