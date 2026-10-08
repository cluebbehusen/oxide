//! JSON-lines wire messages between a host and its clients. A connection
//! speaks [`JoinMessage`] and [`LobbyMessage`] until Go, then
//! [`ClientMessage`] and [`HostMessage`].

use crate::PROTOCOL_VERSION;
use oxide_sim::{Command, PlayerCommand, PlayerId, Scenario, Tick};
use serde::{Deserialize, Serialize};

/// A line a joining client sends before the match starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum JoinMessage {
    /// The client's wire protocol and build commit. Its shape never
    /// changes, so mismatched builds can still read each other's Hello.
    Hello {
        /// The client's [`PROTOCOL_VERSION`].
        protocol: u32,
        /// The commit the client was built from.
        commit: String,
    },
    /// The client built the match and its world at tick zero hashes to
    /// `hash`.
    Ready {
        /// `State::hash` at tick zero.
        hash: u64,
    },
}

/// A line the host sends to a joining client before the match starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LobbyMessage {
    /// The host's wire protocol and build commit, in the same fixed shape
    /// as the client's Hello.
    Hello {
        /// The host's [`PROTOCOL_VERSION`].
        protocol: u32,
        /// The commit the host was built from.
        commit: String,
    },
    /// The frozen roster's match: the client's seat and the scenario every
    /// machine builds.
    Start {
        /// The seat this client plays.
        seat: PlayerId,
        /// The match every machine builds.
        scenario: Box<Scenario>,
    },
    /// Every client is ready; the match begins at tick zero.
    Go,
}

impl JoinMessage {
    /// This build's Hello.
    pub fn hello(commit: &str) -> Self {
        Self::Hello {
            protocol: PROTOCOL_VERSION,
            commit: commit.to_owned(),
        }
    }
}

impl LobbyMessage {
    /// This build's Hello.
    pub fn hello(commit: &str) -> Self {
        Self::Hello {
            protocol: PROTOCOL_VERSION,
            commit: commit.to_owned(),
        }
    }
}

/// Whether a peer's Hello matches this build: the same protocol version and
/// the same commit.
pub fn same_build(protocol: u32, peer_commit: &str, commit: &str) -> bool {
    protocol == PROTOCOL_VERSION && peer_commit == commit
}

/// A line a client sends to the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    /// An order for the client's bound seat. The host attributes the seat
    /// from the connection, so a client cannot name another seat.
    Command {
        /// The order.
        command: Command,
    },
    /// The client executed the batch that left its world at `tick`.
    Ack {
        /// The world tick after the executed batch.
        tick: Tick,
        /// `State::hash` at `tick`, present exactly when
        /// [`crate::reports_hash`] holds for `tick`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    /// Sent when the client has sent nothing else recently.
    Heartbeat,
}

/// A line the host sends to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostMessage {
    /// The complete ordered commands every machine executes on `tick`.
    Batch {
        /// The world tick the batch executes on.
        tick: Tick,
        /// Human commands in sealed order, then bot commands.
        commands: Vec<PlayerCommand>,
    },
    /// Sent when the host has sent nothing else recently.
    Heartbeat,
    /// A client's hash report disagreed with the host at `tick`; the
    /// session has halted.
    Desync {
        /// The report tick that disagreed.
        tick: Tick,
    },
}

macro_rules! wire_lines {
    ($($message:ty),+) => {$(
        impl $message {
            /// One wire line, without the trailing newline.
            pub fn encode(&self) -> String {
                serde_json::to_string(self).expect("wire messages always serialize")
            }

            /// Parses one wire line.
            pub fn decode(line: &str) -> serde_json::Result<Self> {
                serde_json::from_str(line)
            }
        }
    )+};
}

wire_lines!(ClientMessage, HostMessage, JoinMessage, LobbyMessage);

#[cfg(test)]
mod tests;
