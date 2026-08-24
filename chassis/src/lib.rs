#![doc = include_str!("../README.md")]

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
