//! The driver: Oxide's test harness and remote control.
//!
//! Three capabilities, all headless-friendly:
//!
//! - [`runner`] executes scenarios and replays at full speed with no window,
//!   the workhorse behind integration tests and CI.
//! - [`render`] draws any sim state to a PNG with a CPU rasterizer
//!   (tiny-skia) — pixel-identical on every machine, which is what lets
//!   golden-image tests compare bytes instead of tuning tolerances. The GPU
//!   never enters the picture.
//! - [`client`] speaks the debug protocol to a live shell over TCP.
//!
//! The binary (`oxide-driver`) wraps all three in a CLI; see README.

pub mod client;
pub mod render;
pub mod runner;
pub mod smoke;
