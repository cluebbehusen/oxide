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
fn a_hello_matches_only_the_same_protocol_sim_and_commit() {
    assert!(same_build(PROTOCOL_VERSION, SIM_VERSION, "abc", "abc"));
    assert!(!same_build(PROTOCOL_VERSION + 1, SIM_VERSION, "abc", "abc"));
    assert!(!same_build(PROTOCOL_VERSION, SIM_VERSION + 1, "abc", "abc"));
    assert!(!same_build(PROTOCOL_VERSION, SIM_VERSION, "abd", "abc"));
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
    let hello = format!(
        r#"{{"type":"hello","protocol":{PROTOCOL_VERSION},"sim":{SIM_VERSION},"commit":"abc"}}"#
    );
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
        r#"{"type":"command","command":{"type":"stop","units":[7],"extra":true}}"#,
        r#"{"type":"command","command":{"type":"harvest","units":[7],"node":{"x":1,"y":2,"z":3}}}"#,
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
