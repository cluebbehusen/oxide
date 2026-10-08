use super::*;
use crate::HASH_INTERVAL;
use oxide_sim::{PlayerId, UnitId};

fn stop(unit: u32) -> Command {
    Command::Stop {
        units: vec![UnitId(unit)],
    }
}

fn batch_line(tick: Tick, unit: u32) -> String {
    HostMessage::Batch {
        tick,
        commands: vec![PlayerCommand {
            player: PlayerId(1),
            command: stop(unit),
        }],
    }
    .encode()
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

#[test]
fn batches_are_handed_out_only_after_they_arrive_and_in_order() {
    let mut client = ClientSession::new(secs(0));
    assert_eq!(client.next_batch(), None);
    client.receive(&batch_line(0, 10), secs(0)).unwrap();
    client.receive(&batch_line(1, 11), secs(0)).unwrap();
    assert_eq!(client.backlog(), 2);
    let first = client.next_batch().unwrap();
    assert_eq!(client.backlog(), 1);
    assert_eq!(first[0].command, stop(10));
    client.executed(|| unreachable!("tick 1 is not a report tick"));
    assert_eq!(client.next_batch().unwrap()[0].command, stop(11));
    client.executed(|| unreachable!("tick 2 is not a report tick"));
    assert_eq!(client.next_batch(), None);
}

#[test]
fn a_gap_or_repeat_in_batch_ticks_is_a_protocol_violation() {
    for ticks in [[0, 2], [0, 0]] {
        let mut client = ClientSession::new(secs(0));
        client.receive(&batch_line(ticks[0], 0), secs(0)).unwrap();
        assert_eq!(
            client.receive(&batch_line(ticks[1], 0), secs(0)),
            Err(ClientEnd::Protocol)
        );
        assert_eq!(
            client.next_batch(),
            None,
            "an ended session executes nothing"
        );
    }
    let mut client = ClientSession::new(secs(0));
    assert_eq!(client.receive("{", secs(0)), Err(ClientEnd::Protocol));
    assert_eq!(client.poll(secs(0)), Err(ClientEnd::Protocol));
}

#[test]
fn every_batch_is_acknowledged_with_a_hash_on_report_ticks() {
    let mut client = ClientSession::new(secs(0));
    for tick in 0..HASH_INTERVAL {
        client.receive(&batch_line(tick, 0), secs(0)).unwrap();
        client.next_batch().unwrap();
        client.executed(|| 99);
    }
    let acks: Vec<ClientMessage> = client
        .take_outgoing()
        .iter()
        .map(|line| ClientMessage::decode(line).unwrap())
        .collect();
    let expected: Vec<ClientMessage> = (1..=HASH_INTERVAL)
        .map(|tick| ClientMessage::Ack {
            tick,
            hash: (tick == HASH_INTERVAL).then_some(99),
        })
        .collect();
    assert_eq!(acks, expected);
}

#[test]
fn commands_are_sent_bare() {
    let mut client = ClientSession::new(secs(0));
    client.send(stop(3));
    assert_eq!(
        client.take_outgoing(),
        vec![ClientMessage::Command { command: stop(3) }.encode()]
    );
}

#[test]
fn a_silent_host_ends_the_session_and_heartbeats_keep_it() {
    let mut client = ClientSession::new(secs(0));
    let mut now = secs(0);
    while now < secs(30) {
        client
            .receive(&HostMessage::Heartbeat.encode(), now)
            .unwrap();
        client.poll(now).unwrap();
        now += secs(1);
    }
    let heard = now.checked_sub(secs(1)).unwrap();
    client
        .poll(
            (heard + SILENCE_TIMEOUT)
                .checked_sub(Duration::from_millis(1))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.poll(heard + SILENCE_TIMEOUT),
        Err(ClientEnd::HostSilent)
    );
    assert_eq!(
        client.receive(&HostMessage::Heartbeat.encode(), now),
        Err(ClientEnd::HostSilent),
        "an ended session stays ended"
    );
}

#[test]
fn an_idle_client_heartbeats_and_speaking_resets_the_timer() {
    let heartbeat = ClientMessage::Heartbeat.encode();
    let mut client = ClientSession::new(secs(0));
    client.poll(Duration::from_millis(499)).unwrap();
    assert!(client.take_outgoing().is_empty());
    client.poll(HEARTBEAT_INTERVAL).unwrap();
    assert_eq!(client.take_outgoing(), vec![heartbeat.clone()]);
    client.send(stop(1));
    client.poll(secs(1)).unwrap();
    assert_eq!(client.take_outgoing().len(), 1, "only the command");
    client.poll(secs(1) + Duration::from_millis(499)).unwrap();
    assert!(client.take_outgoing().is_empty());
    client.poll(secs(1) + HEARTBEAT_INTERVAL).unwrap();
    assert_eq!(client.take_outgoing(), vec![heartbeat]);
}

#[test]
fn a_desync_notice_ends_the_session() {
    let mut client = ClientSession::new(secs(0));
    client.receive(&batch_line(0, 0), secs(0)).unwrap();
    assert_eq!(
        client.receive(&HostMessage::Desync { tick: 20 }.encode(), secs(0)),
        Err(ClientEnd::Desync { tick: 20 })
    );
    assert_eq!(client.next_batch(), None);
    client.send(stop(1));
    assert!(
        client.take_outgoing().is_empty(),
        "an ended session is quiet"
    );
}

#[test]
#[should_panic(expected = "execute a batch")]
fn acknowledging_without_a_batch_is_a_caller_bug() {
    ClientSession::new(secs(0)).executed(|| 0);
}

#[test]
#[should_panic(expected = "report the previous batch")]
fn taking_a_batch_before_acknowledging_the_last_is_a_caller_bug() {
    let mut client = ClientSession::new(secs(0));
    client.receive(&batch_line(0, 0), secs(0)).unwrap();
    client.receive(&batch_line(1, 0), secs(0)).unwrap();
    client.next_batch();
    client.next_batch();
}
