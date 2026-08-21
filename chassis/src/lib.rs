//! Deterministic-simulation toolkit.
//!
//! Everything a lockstep game sim needs and nothing it doesn't: fixed-point
//! math, a seedable RNG, canonical state hashing, tile grids with A*, and a
//! replay format. The contract across the whole crate is *bit-identical
//! results on every run and every platform* — no floats, no hash-map
//! iteration, no wall clock.
//!
//! Game rules live elsewhere. This crate must stay reusable for the next game.

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
