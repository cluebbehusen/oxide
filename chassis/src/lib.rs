#![doc = include_str!("../README.md")]
// Floats and hash-ordered collections would break bit-identical replays.
#![deny(clippy::float_arithmetic, clippy::disallowed_types)]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

pub mod compass;
pub mod fsx;
pub mod fx;
pub mod grid;
pub mod hash;
pub mod path;
pub mod replay;
pub mod rng;

/// Simulation time, counted in fixed-timestep ticks since scenario start.
pub type Tick = u64;
