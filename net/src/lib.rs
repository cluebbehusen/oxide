#![doc = include_str!("../README.md")]
// Floats and hash-ordered collections would break bit-identical replays.
#![deny(clippy::float_arithmetic, clippy::disallowed_types)]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

pub mod client;
pub mod host;
pub mod message;
pub mod start;
pub mod tcp;

use oxide_sim::{TICKS_PER_SECOND, Tick};
use std::time::Duration;

pub use client::{ClientEnd, ClientSession};
pub use host::{DropReason, HostEvent, HostSession};
pub use message::{ClientMessage, HostMessage, JoinMessage, LobbyMessage, same_build};
pub use start::{StartBarrier, StartFailed};
pub use tcp::{Closed, Connection, Listener};

/// The wire protocol version both sides' Hello must match. Bump it for any
/// wire change other than Hello itself, whose shape never changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// The port a host listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 4200;

/// The host abandons a start that has not collected every Ready by then.
pub const START_TIMEOUT: Duration = Duration::from_secs(20);

/// The longest accepted line, excluding its newline.
pub const MAX_LINE_BYTES: usize = 1 << 20;

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
