use super::*;
use crate::HASH_INTERVAL;
use oxide_sim::UnitId;

const HOST: PlayerId = PlayerId(0);
const A: PlayerId = PlayerId(1);
const B: PlayerId = PlayerId(2);

fn stop(unit: u32) -> Command {
    Command::Stop {
        units: vec![UnitId(unit)],
    }
}

fn from(seat: PlayerId, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: seat,
        command,
    }
}

fn command_line(unit: u32) -> String {
    ClientMessage::Command {
        command: stop(unit),
    }
    .encode()
}

fn ack_line(tick: Tick, hash: u64) -> String {
    ClientMessage::Ack {
        tick,
        hash: reports_hash(tick).then_some(hash),
    }
    .encode()
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Seals, publishes and executes one tick with no bot commands, with the
/// host hashing to `hash`.
fn step(host: &mut HostSession, now: Duration, hash: u64) -> Option<Vec<PlayerCommand>> {
    let batch = host.seal(now)?;
    host.publish(&batch);
    host.executed(|| hash);
    Some(batch)
}

fn ack_through(host: &mut HostSession, seat: PlayerId, from: Tick, to: Tick, now: Duration) {
    for tick in from..=to {
        host.receive(seat, &ack_line(tick, 0), now);
    }
}

#[test]
fn commands_join_the_next_sealed_tick_in_rotated_seat_order() {
    let mut host = HostSession::new(HOST, &[B, A], secs(0));
    let submit_all = |host: &mut HostSession| {
        host.receive(B, &command_line(20), secs(0));
        host.receive(A, &command_line(10), secs(0));
        host.submit(stop(0));
        host.receive(A, &command_line(11), secs(0));
    };
    let expected = |order: [PlayerId; 3]| -> Vec<PlayerCommand> {
        order
            .iter()
            .flat_map(|&seat| match seat.0 {
                0 => vec![from(HOST, stop(0))],
                1 => vec![from(A, stop(10)), from(A, stop(11))],
                _ => vec![from(B, stop(20))],
            })
            .collect()
    };
    submit_all(&mut host);
    assert_eq!(step(&mut host, secs(0), 0).unwrap(), expected([HOST, A, B]));
    submit_all(&mut host);
    assert_eq!(step(&mut host, secs(0), 0).unwrap(), expected([A, B, HOST]));
    submit_all(&mut host);
    assert_eq!(step(&mut host, secs(0), 0).unwrap(), expected([B, HOST, A]));
    assert_eq!(step(&mut host, secs(0), 0).unwrap(), Vec::new());
}

#[test]
fn published_batches_carry_the_complete_batch_to_every_live_client() {
    let mut host = HostSession::new(HOST, &[A, B], secs(0));
    let mut batch = host.seal(secs(0)).unwrap();
    batch.push(from(PlayerId(3), stop(30)));
    host.publish(&batch);
    let line = HostMessage::Batch {
        tick: 0,
        commands: batch,
    }
    .encode();
    assert_eq!(host.take_outgoing(), vec![(A, line.clone()), (B, line)]);
}

#[test]
fn the_lead_cap_waits_for_the_slowest_live_client() {
    let mut host = HostSession::new(HOST, &[A, B], secs(0));
    for _ in 0..LEAD_CAP {
        assert!(step(&mut host, secs(0), 0).is_some());
    }
    assert!(step(&mut host, secs(0), 0).is_none());
    ack_through(&mut host, A, 1, 5, secs(0));
    assert!(step(&mut host, secs(0), 0).is_none(), "B has acked nothing");
    ack_through(&mut host, B, 1, 1, secs(0));
    assert!(step(&mut host, secs(0), 0).is_some());
    assert!(step(&mut host, secs(0), 0).is_none());
    assert!(host.poll(secs(0)).is_empty());
}

#[test]
fn out_of_order_or_misreported_acks_are_protocol_violations() {
    type Violation = fn(&mut HostSession);
    let cases: [(&str, Violation); 5] = [
        ("skipped", |host| host.receive(A, &ack_line(2, 0), secs(0))),
        ("duplicate", |host| {
            host.receive(A, &ack_line(1, 0), secs(0));
            host.receive(A, &ack_line(1, 0), secs(0));
        }),
        ("unpublished", |host| {
            ack_through(host, A, 1, 3, secs(0));
            host.receive(A, &ack_line(4, 0), secs(0));
        }),
        ("unexpected hash", |host| {
            host.receive(
                A,
                &ClientMessage::Ack {
                    tick: 1,
                    hash: Some(1),
                }
                .encode(),
                secs(0),
            );
        }),
        ("garbage", |host| host.receive(A, "{", secs(0))),
    ];
    for (name, break_protocol) in cases {
        let mut host = HostSession::new(HOST, &[A], secs(0));
        for _ in 0..3 {
            step(&mut host, secs(0), 0).unwrap();
        }
        break_protocol(&mut host);
        assert_eq!(
            host.poll(secs(0)),
            vec![HostEvent::Dropped {
                seat: A,
                reason: DropReason::Protocol
            }],
            "{name}"
        );
    }
}

#[test]
fn a_report_tick_ack_without_a_hash_is_a_protocol_violation() {
    let mut host = HostSession::new(HOST, &[A], secs(0));
    for _ in 0..HASH_INTERVAL {
        step(&mut host, secs(0), 0).unwrap();
    }
    ack_through(&mut host, A, 1, HASH_INTERVAL - 1, secs(0));
    host.receive(
        A,
        &ClientMessage::Ack {
            tick: HASH_INTERVAL,
            hash: None,
        }
        .encode(),
        secs(0),
    );
    assert_eq!(
        host.poll(secs(0)),
        vec![HostEvent::Dropped {
            seat: A,
            reason: DropReason::Protocol
        }]
    );
}

#[test]
fn a_heartbeating_client_is_dropped_after_blocking_the_host_for_the_progress_timeout() {
    let mut host = HostSession::new(HOST, &[A, B], secs(0));
    for _ in 0..LEAD_CAP {
        step(&mut host, secs(0), 0).unwrap();
    }
    ack_through(&mut host, B, 1, LEAD_CAP, secs(0));
    assert!(step(&mut host, secs(1), 0).is_none());
    let deadline = secs(1) + PROGRESS_TIMEOUT;
    let mut now = secs(1);
    while now < deadline {
        host.receive(A, &ClientMessage::Heartbeat.encode(), now);
        host.receive(B, &ClientMessage::Heartbeat.encode(), now);
        assert!(step(&mut host, now, 0).is_none());
        assert!(host.poll(now).is_empty(), "dropped early at {now:?}");
        now += Duration::from_millis(250);
    }
    assert_eq!(
        host.poll(deadline),
        vec![HostEvent::Dropped {
            seat: A,
            reason: DropReason::Stalled
        }]
    );
    let batch = step(&mut host, deadline, 0).expect("A left the gate");
    assert_eq!(batch, vec![from(A, Command::Surrender)]);
}

#[test]
fn time_spent_paused_never_counts_toward_the_progress_timeout() {
    let mut host = HostSession::new(HOST, &[A], secs(0));
    for _ in 0..LEAD_CAP {
        step(&mut host, secs(0), 0).unwrap();
    }
    assert!(step(&mut host, secs(1), 0).is_none());
    host.set_paused(true);
    for second in 2..30 {
        host.receive(A, &ClientMessage::Heartbeat.encode(), secs(second));
        assert!(step(&mut host, secs(second), 0).is_none());
        assert!(host.poll(secs(second)).is_empty());
    }
    host.set_paused(false);
    assert!(step(&mut host, secs(30), 0).is_none());
    host.receive(A, &ClientMessage::Heartbeat.encode(), secs(35));
    assert!(host.poll(secs(35)).is_empty());
    host.receive(A, &ClientMessage::Heartbeat.encode(), secs(40));
    assert_eq!(
        host.poll(secs(40)),
        vec![HostEvent::Dropped {
            seat: A,
            reason: DropReason::Stalled
        }]
    );
}

#[test]
fn repeating_the_pause_state_keeps_the_progress_timer() {
    let mut host = HostSession::new(HOST, &[A], secs(0));
    for _ in 0..LEAD_CAP {
        step(&mut host, secs(0), 0).unwrap();
    }
    assert!(step(&mut host, secs(1), 0).is_none());
    for second in 2..11 {
        host.set_paused(false);
        host.receive(A, &ClientMessage::Heartbeat.encode(), secs(second));
        assert!(step(&mut host, secs(second), 0).is_none());
        assert!(host.poll(secs(second)).is_empty());
    }
    host.set_paused(false);
    host.receive(A, &ClientMessage::Heartbeat.encode(), secs(11));
    assert_eq!(
        host.poll(secs(11)),
        vec![HostEvent::Dropped {
            seat: A,
            reason: DropReason::Stalled
        }]
    );
}

#[test]
fn a_silent_client_is_dropped_and_heartbeats_keep_it() {
    let mut host = HostSession::new(HOST, &[A, B], secs(0));
    let mut now = secs(0);
    while now < SILENCE_TIMEOUT {
        host.receive(A, &ClientMessage::Heartbeat.encode(), now);
        assert!(host.poll(now).is_empty());
        now += secs(1);
    }
    assert_eq!(
        host.poll(SILENCE_TIMEOUT),
        vec![HostEvent::Dropped {
            seat: B,
            reason: DropReason::Silent
        }]
    );
}

#[test]
fn every_drop_leaves_the_gate_at_once_and_seals_one_surrender() {
    type DropA = fn(&mut HostSession, Duration);
    let later = PROGRESS_TIMEOUT;
    let drops: [(DropReason, DropA); 4] = [
        (DropReason::Closed, |host, _| host.disconnected(A)),
        (DropReason::Protocol, |host, now| host.receive(A, "{", now)),
        (DropReason::Silent, |_, _| {}),
        (DropReason::Stalled, |host, now| {
            host.receive(A, &ClientMessage::Heartbeat.encode(), now);
        }),
    ];
    for (reason, drop_a) in drops {
        let mut host = HostSession::new(HOST, &[A, B], secs(0));
        for _ in 0..LEAD_CAP {
            step(&mut host, secs(0), 0).unwrap();
        }
        ack_through(&mut host, B, 1, LEAD_CAP, secs(0));
        assert!(step(&mut host, secs(0), 0).is_none(), "blocked on A");
        host.take_outgoing();
        host.receive(B, &ClientMessage::Heartbeat.encode(), later);
        drop_a(&mut host, later);
        assert_eq!(
            host.poll(later),
            vec![HostEvent::Dropped { seat: A, reason }],
            "{reason:?}"
        );
        host.receive(A, &command_line(10), later);
        host.disconnected(A);
        assert!(host.poll(later).is_empty(), "{reason:?}: dropped twice");
        let batch = step(&mut host, later, 0).expect("A left the gate");
        assert_eq!(batch, vec![from(A, Command::Surrender)], "{reason:?}");
        assert!(
            host.take_outgoing().iter().all(|(seat, _)| *seat == B),
            "{reason:?}: a dropped client is sent nothing"
        );
    }
}

#[test]
fn idle_clients_get_heartbeats_and_batches_reset_the_timer() {
    let mut host = HostSession::new(HOST, &[A], secs(0));
    let heartbeat = (A, HostMessage::Heartbeat.encode());
    assert!(host.poll(Duration::from_millis(499)).is_empty());
    assert!(host.take_outgoing().is_empty());
    host.poll(HEARTBEAT_INTERVAL);
    assert_eq!(host.take_outgoing(), vec![heartbeat.clone()]);
    host.receive(A, &ClientMessage::Heartbeat.encode(), secs(1));
    step(&mut host, secs(1), 0).unwrap();
    host.poll(secs(1));
    assert_eq!(host.take_outgoing().len(), 1, "only the batch");
    host.poll(secs(1) + Duration::from_millis(499));
    assert!(host.take_outgoing().is_empty());
    host.poll(secs(1) + HEARTBEAT_INTERVAL);
    assert_eq!(host.take_outgoing(), vec![heartbeat]);
}

#[test]
fn a_mismatched_hash_halts_the_session_and_tells_every_client() {
    let mut host = HostSession::new(HOST, &[A, B], secs(0));
    for _ in 0..HASH_INTERVAL {
        step(&mut host, secs(0), 7).unwrap();
    }
    host.take_outgoing();
    for tick in 1..=HASH_INTERVAL {
        host.receive(A, &ack_line(tick, 7), secs(0));
    }
    assert!(host.poll(secs(0)).is_empty(), "a matching report is quiet");
    assert_eq!(host.hashes.len(), 1, "B has yet to report");
    for tick in 1..=HASH_INTERVAL {
        host.receive(B, &ack_line(tick, 8), secs(0));
    }
    assert!(host.hashes.is_empty(), "every live client reported");
    assert_eq!(
        host.poll(secs(0)),
        vec![HostEvent::Desync {
            seat: B,
            tick: HASH_INTERVAL
        }]
    );
    let desync = HostMessage::Desync {
        tick: HASH_INTERVAL,
    }
    .encode();
    assert_eq!(host.take_outgoing(), vec![(A, desync.clone()), (B, desync)]);
    assert!(host.seal(secs(0)).is_none());
}

#[test]
fn a_host_is_caught_up_once_every_live_client_acknowledged_every_batch() {
    let mut host = HostSession::new(HOST, &[A, B], secs(0));
    assert!(host.caught_up());
    for _ in 0..3 {
        step(&mut host, secs(0), 0).unwrap();
    }
    ack_through(&mut host, A, 1, 3, secs(0));
    ack_through(&mut host, B, 1, 2, secs(0));
    assert!(!host.caught_up(), "B is a batch behind");
    host.disconnected(B);
    assert!(host.caught_up(), "a dropped client is not waited on");
}

#[test]
fn a_host_without_clients_is_never_gated() {
    let mut host = HostSession::new(HOST, &[], secs(0));
    for _ in 0..(3 * LEAD_CAP) {
        step(&mut host, secs(0), 0).unwrap();
    }
    assert!(host.hashes.is_empty());
}

#[test]
#[should_panic(expected = "publish the sealed batch")]
fn sealing_twice_without_publishing_is_a_caller_bug() {
    let mut host = HostSession::new(HOST, &[], secs(0));
    host.seal(secs(0));
    host.seal(secs(0));
}

#[test]
#[should_panic(expected = "seal a batch")]
fn publishing_without_sealing_is_a_caller_bug() {
    HostSession::new(HOST, &[], secs(0)).publish(&[]);
}

#[test]
#[should_panic(expected = "must be distinct")]
fn duplicate_seats_are_a_caller_bug() {
    HostSession::new(HOST, &[A, A], secs(0));
}

#[test]
#[should_panic(expected = "is not a client seat")]
fn lines_from_unknown_seats_are_a_caller_bug() {
    HostSession::new(HOST, &[A], secs(0)).receive(B, "{}", secs(0));
}

/// A Stop order whose client line is exactly `bytes` long, padded with
/// unit ids.
fn bulky_line(bytes: usize) -> String {
    let mut units = Vec::new();
    let line = |units: &[UnitId]| {
        ClientMessage::Command {
            command: Command::Stop {
                units: units.to_vec(),
            },
        }
        .encode()
    };
    while line(&units).len() + 2 <= bytes {
        units.push(UnitId(1));
    }
    let mut padded = line(&units);
    while padded.len() < bytes {
        let last = units.last_mut().expect("padding grows a unit id");
        last.0 = last.0 * 10 + 1;
        padded = line(&units);
    }
    assert_eq!(padded.len(), bytes, "padding lands exactly");
    padded
}

#[test]
fn an_oversized_client_line_drops_the_client() {
    let mut host = HostSession::new(HOST, &[A], secs(0));
    host.receive(A, &bulky_line(crate::MAX_CLIENT_LINE_BYTES + 1), secs(0));
    assert_eq!(
        host.poll(secs(0)),
        [HostEvent::Dropped {
            seat: A,
            reason: DropReason::Protocol,
        }]
    );
}

#[test]
fn a_client_flooding_a_paused_host_is_dropped() {
    let mut host = HostSession::new(HOST, &[A, B], secs(0));
    host.set_paused(true);
    let line = bulky_line(crate::MAX_CLIENT_LINE_BYTES);
    let fit = crate::CLIENT_PENDING_BYTES / line.len();
    for _ in 0..fit {
        host.receive(A, &line, secs(0));
        host.receive(B, &command_line(1), secs(0));
    }
    assert_eq!(host.poll(secs(0)), []);
    host.receive(A, &line, secs(0));
    assert_eq!(
        host.poll(secs(0)),
        [HostEvent::Dropped {
            seat: A,
            reason: DropReason::Protocol,
        }]
    );
}

#[test]
fn pending_bytes_cover_the_published_batch() {
    let pending =
        [Command::Surrender, stop(1), stop(70_000)].map(|command| Pending::new(A, command));
    let batch = |commands: Vec<PlayerCommand>| HostMessage::Batch { tick: 9, commands }.encode();
    let empty = batch(Vec::new()).len();
    let full = batch(
        pending
            .iter()
            .map(|queued| queued.command.clone())
            .collect(),
    )
    .len();
    let counted: usize = pending.iter().map(|queued| queued.bytes).sum();
    assert!(full <= empty + counted, "{full} > {empty} + {counted}");
}

#[test]
fn every_published_batch_fits_one_line() {
    let clients = [1, 2, 3, 4, 5, 6].map(PlayerId);
    let mut host = HostSession::new(HOST, &clients, secs(0));
    let line = bulky_line(crate::MAX_CLIENT_LINE_BYTES);
    let per_client = crate::CLIENT_PENDING_BYTES / line.len();
    for _ in 0..per_client {
        for seat in clients {
            host.receive(seat, &line, secs(0));
        }
    }
    let sent = per_client * clients.len();
    let mut sealed = 0;
    let mut tick = 0;
    while sealed < sent {
        let batch = step(&mut host, secs(0), 0).expect("no client lags yet");
        sealed += batch.len();
        tick += 1;
        for (_, out) in host.take_outgoing() {
            assert!(out.len() <= crate::MAX_LINE_BYTES, "{} bytes", out.len());
        }
        for seat in clients {
            ack_through(&mut host, seat, tick, tick, secs(0));
        }
    }
    assert_eq!(sealed, sent);
    assert!(tick > 1, "the flood spans several ticks");
}
