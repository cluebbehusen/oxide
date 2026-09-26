//! JSON-lines wire messages between a host and its clients.

use oxide_sim::{Command, PlayerCommand, Tick};
use serde::{Deserialize, Serialize};

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

impl ClientMessage {
    /// One wire line, without the trailing newline.
    pub fn encode(&self) -> String {
        encode(self)
    }

    /// Parses one wire line.
    pub fn decode(line: &str) -> serde_json::Result<Self> {
        serde_json::from_str(line)
    }
}

impl HostMessage {
    /// One wire line, without the trailing newline.
    pub fn encode(&self) -> String {
        encode(self)
    }

    /// Parses one wire line.
    pub fn decode(line: &str) -> serde_json::Result<Self> {
        serde_json::from_str(line)
    }
}

fn encode(message: &impl Serialize) -> String {
    serde_json::to_string(message).expect("wire messages always serialize")
}

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
    }
}
