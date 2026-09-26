#![doc = include_str!("../README.md")]

pub mod client;
pub mod host;
pub mod message;

use oxide_sim::{TICKS_PER_SECOND, Tick};
use std::time::Duration;

pub use client::{ClientEnd, ClientSession};
pub use host::{DropReason, HostEvent, HostSession};
pub use message::{ClientMessage, HostMessage};

/// Clients attach their state hash to acknowledgements of ticks that are
/// multiples of this.
pub const HASH_INTERVAL: Tick = 20;

/// How many ticks the host may seal beyond the slowest live client's
/// acknowledged tick.
pub const LEAD_CAP: Tick = 2 * TICKS_PER_SECOND as Tick;

/// Each side sends a heartbeat after this long without sending anything else.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);

/// A peer that has heard nothing for this long treats the other side as gone.
pub const SILENCE_TIMEOUT: Duration = Duration::from_secs(10);

/// The host drops a client it has been blocked on for this long.
pub const PROGRESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the acknowledgement of a batch that leaves the world at `tick`
/// carries a state hash.
pub fn reports_hash(tick: Tick) -> bool {
    tick.is_multiple_of(HASH_INTERVAL)
}
