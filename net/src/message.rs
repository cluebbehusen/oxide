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
mod tests {
    use super::*;
    use oxide_sim::{PlayerId, UnitId};

    fn stop(unit: u32) -> Command {
        Command::Stop {
            units: vec![UnitId(unit)],
        }
    }

    #[test]
    fn every_message_survives_a_round_trip() {
        let client = [
            ClientMessage::Command { command: stop(7) },
            ClientMessage::Ack {
                tick: 19,
                hash: None,
            },
            ClientMessage::Ack {
                tick: 20,
                hash: Some(u64::MAX),
            },
            ClientMessage::Heartbeat,
        ];
        for message in client {
            assert_eq!(ClientMessage::decode(&message.encode()).unwrap(), message);
        }
        let host = [
            HostMessage::Batch {
                tick: 3,
                commands: vec![PlayerCommand {
                    player: PlayerId(1),
                    command: stop(7),
                }],
            },
            HostMessage::Heartbeat,
            HostMessage::Desync { tick: 40 },
        ];
        for message in host {
            assert_eq!(HostMessage::decode(&message.encode()).unwrap(), message);
        }
        for message in [JoinMessage::hello("abc"), JoinMessage::Ready { hash: 9 }] {
            assert_eq!(JoinMessage::decode(&message.encode()).unwrap(), message);
        }
        for message in [
            LobbyMessage::hello("abc"),
            LobbyMessage::Start {
                seat: PlayerId(2),
                scenario: Box::new(Scenario::skirmish()),
            },
            LobbyMessage::Go,
        ] {
            assert_eq!(LobbyMessage::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn a_hello_matches_only_the_same_protocol_and_commit() {
        assert!(same_build(PROTOCOL_VERSION, "abc", "abc"));
        assert!(!same_build(PROTOCOL_VERSION + 1, "abc", "abc"));
        assert!(!same_build(PROTOCOL_VERSION, "abd", "abc"));
    }

    #[test]
    fn wire_shape_is_stable() {
        assert_eq!(
            ClientMessage::Command { command: stop(7) }.encode(),
            r#"{"type":"command","command":{"type":"stop","units":[7]}}"#
        );
        assert_eq!(
            ClientMessage::Ack {
                tick: 19,
                hash: None
            }
            .encode(),
            r#"{"type":"ack","tick":19}"#
        );
        assert_eq!(
            ClientMessage::Ack {
                tick: 20,
                hash: Some(5)
            }
            .encode(),
            r#"{"type":"ack","tick":20,"hash":5}"#
        );
        assert_eq!(
            HostMessage::Batch {
                tick: 3,
                commands: vec![PlayerCommand {
                    player: PlayerId(1),
                    command: stop(7),
                }],
            }
            .encode(),
            r#"{"type":"batch","tick":3,"commands":[{"player":1,"command":{"type":"stop","units":[7]}}]}"#
        );
        assert_eq!(HostMessage::Heartbeat.encode(), r#"{"type":"heartbeat"}"#);
        assert_eq!(
            HostMessage::Desync { tick: 40 }.encode(),
            r#"{"type":"desync","tick":40}"#
        );
        let hello = format!(r#"{{"type":"hello","protocol":{PROTOCOL_VERSION},"commit":"abc"}}"#);
        assert_eq!(JoinMessage::hello("abc").encode(), hello);
        assert_eq!(LobbyMessage::hello("abc").encode(), hello);
        assert_eq!(
            JoinMessage::Ready { hash: 9 }.encode(),
            r#"{"type":"ready","hash":9}"#
        );
        assert_eq!(LobbyMessage::Go.encode(), r#"{"type":"go"}"#);
        let start = LobbyMessage::Start {
            seat: PlayerId(2),
            scenario: Box::new(Scenario::skirmish()),
        }
        .encode();
        assert!(start.starts_with(r#"{"type":"start","seat":2,"scenario":{"#));
    }

    #[test]
    fn malformed_lines_are_rejected() {
        for line in [
            r#"{"type":"ack","tick":1,"extra":true}"#,
            r#"{"type":"acknowledge","tick":1}"#,
            r#"{"type":"ack"}"#,
            r#"{"type":"heartbeat"}{"type":"heartbeat"}"#,
            "not json",
        ] {
            assert!(ClientMessage::decode(line).is_err(), "{line}");
        }
        for line in [
            r#"{"type":"batch","tick":1,"commands":[],"extra":true}"#,
            r#"{"type":"halt","tick":1}"#,
            r#"{"type":"batch","commands":[]}"#,
        ] {
            assert!(HostMessage::decode(line).is_err(), "{line}");
        }
        for line in [
            r#"{"type":"hello","protocol":1}"#,
            r#"{"type":"ready","hash":1,"seat":0}"#,
            r#"{"type":"ack","tick":1}"#,
        ] {
            assert!(JoinMessage::decode(line).is_err(), "{line}");
        }
        for line in [
            r#"{"type":"hello","protocol":1,"commit":"abc","extra":true}"#,
            r#"{"type":"start","seat":1}"#,
            r#"{"type":"batch","tick":0,"commands":[]}"#,
        ] {
            assert!(LobbyMessage::decode(line).is_err(), "{line}");
        }
    }
}
