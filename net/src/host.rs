//! The host side: stamping, sealing order, the progress gate, desync
//! detection, and client liveness.

use crate::message::{ClientMessage, HostMessage};
use crate::{HEARTBEAT_INTERVAL, LEAD_CAP, PROGRESS_TIMEOUT, SILENCE_TIMEOUT, reports_hash};
use oxide_sim::{Command, PlayerCommand, PlayerId, Tick};
use std::collections::VecDeque;
use std::time::Duration;

/// Why the host stopped serving a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// The transport reported the connection closed.
    Closed,
    /// Nothing arrived for [`SILENCE_TIMEOUT`].
    Silent,
    /// The host was blocked on the client for [`PROGRESS_TIMEOUT`].
    Stalled,
    /// The client sent a line that failed to decode or broke the protocol.
    Protocol,
}

/// Something the host's caller must act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    /// The client left the progress gate; its `Surrender` is pending.
    Dropped {
        /// The client's seat.
        seat: PlayerId,
        /// Why it was dropped.
        reason: DropReason,
    },
    /// The client's hash report disagreed with the host; the session has
    /// halted and every live client has been told.
    Desync {
        /// The reporting client's seat.
        seat: PlayerId,
        /// The report tick that disagreed.
        tick: Tick,
    },
}

#[derive(Debug)]
struct Client {
    seat: PlayerId,
    live: bool,
    /// World tick after the last batch the client acknowledged.
    acked: Tick,
    /// When the host last received a line from the client.
    heard: Duration,
    /// When the host last sent the client a line, as seen by `poll`.
    sent: Duration,
    /// Whether a line was queued for the client since the last `poll`.
    spoke: bool,
    /// When a failed seal started waiting on this client.
    blocked_since: Option<Duration>,
}

/// The host's half of a lockstep session.
///
/// Each game-loop step, the host calls [`HostSession::seal`]; if it yields
/// human commands, the host appends its bot commands, calls
/// [`HostSession::publish`] with the complete batch, executes that batch, and
/// calls [`HostSession::executed`].
#[derive(Debug)]
pub struct HostSession {
    host: PlayerId,
    /// Human seats in rotation order, fixed at creation.
    humans: Vec<PlayerId>,
    clients: Vec<Client>,
    pending: Vec<PlayerCommand>,
    /// The tick the next sealed batch executes on.
    next_tick: Tick,
    sealed: bool,
    paused: bool,
    halted: bool,
    /// The host's own hashes at report ticks some live client has yet to
    /// acknowledge.
    hashes: VecDeque<(Tick, u64)>,
    events: Vec<HostEvent>,
    outgoing: Vec<(PlayerId, String)>,
}

impl HostSession {
    /// A session at tick zero for the host's own seat and its clients' seats.
    pub fn new(host: PlayerId, clients: &[PlayerId], now: Duration) -> Self {
        let mut humans: Vec<PlayerId> = clients.iter().copied().chain([host]).collect();
        humans.sort_unstable();
        let seats = humans.len();
        humans.dedup();
        assert_eq!(humans.len(), seats, "human seats must be distinct");
        Self {
            host,
            humans,
            clients: clients
                .iter()
                .map(|&seat| Client {
                    seat,
                    live: true,
                    acked: 0,
                    heard: now,
                    sent: now,
                    spoke: false,
                    blocked_since: None,
                })
                .collect(),
            pending: Vec::new(),
            next_tick: 0,
            sealed: false,
            paused: false,
            halted: false,
            hashes: VecDeque::new(),
            events: Vec::new(),
            outgoing: Vec::new(),
        }
    }

    /// Queues an order from the host's own seat for the next sealed tick.
    pub fn submit(&mut self, command: Command) {
        self.pending.push(PlayerCommand {
            player: self.host,
            command,
        });
    }

    /// Handles one line received from `seat`'s connection. Lines from a
    /// dropped client are ignored. `seat` must be one of this session's
    /// clients.
    pub fn receive(&mut self, seat: PlayerId, line: &str, now: Duration) {
        let index = self.client_index(seat);
        if !self.clients[index].live {
            return;
        }
        self.clients[index].heard = now;
        match ClientMessage::decode(line) {
            Ok(ClientMessage::Command { command }) => self.pending.push(PlayerCommand {
                player: seat,
                command,
            }),
            Ok(ClientMessage::Ack { tick, hash }) => self.acknowledge(index, tick, hash),
            Ok(ClientMessage::Heartbeat) => {}
            Err(_) => self.drop_client(index, DropReason::Protocol),
        }
    }

    /// The transport reported `seat`'s connection closed.
    pub fn disconnected(&mut self, seat: PlayerId) {
        let index = self.client_index(seat);
        if self.clients[index].live {
            self.drop_client(index, DropReason::Closed);
        }
    }

    /// Pausing stops sealing. Time spent paused never counts toward a
    /// client's progress timeout.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        for client in &mut self.clients {
            client.blocked_since = None;
        }
    }

    /// The human commands for the next tick in sealed order, or `None` while
    /// paused, halted, or waiting on a client at the lead cap. A sealed batch
    /// must be published before the next seal.
    pub fn seal(&mut self, now: Duration) -> Option<Vec<PlayerCommand>> {
        assert!(
            !self.sealed,
            "publish the sealed batch before sealing again"
        );
        if self.paused || self.halted {
            return None;
        }
        let tick = self.next_tick;
        let mut blocked = false;
        for client in &mut self.clients {
            if client.live && tick >= client.acked + LEAD_CAP {
                blocked = true;
                client.blocked_since.get_or_insert(now);
            } else {
                client.blocked_since = None;
            }
        }
        if blocked {
            return None;
        }
        for client in &mut self.clients {
            client.blocked_since = None;
        }
        self.sealed = true;
        let seats = self.humans.len() as Tick;
        let first = tick % seats;
        let mut batch = std::mem::take(&mut self.pending);
        batch.sort_by_key(|command| {
            let index = self
                .humans
                .binary_search(&command.player)
                .expect("pending commands come from human seats") as Tick;
            (index + seats - first) % seats
        });
        Some(batch)
    }

    /// Sends the complete sealed batch (sealed humans, then bots) to every
    /// live client.
    pub fn publish(&mut self, batch: &[PlayerCommand]) {
        assert!(
            std::mem::take(&mut self.sealed),
            "seal a batch before publishing"
        );
        let line = HostMessage::Batch {
            tick: self.next_tick,
            commands: batch.to_vec(),
        }
        .encode();
        self.next_tick += 1;
        self.broadcast(&line);
    }

    /// Records the host's own hash once it has executed the published batch.
    /// `hash` runs only when the resulting tick is a report tick.
    pub fn executed(&mut self, hash: impl FnOnce() -> u64) {
        let tick = self.next_tick;
        if reports_hash(tick) && self.clients.iter().any(|client| client.live) {
            self.hashes.push_back((tick, hash()));
        }
    }

    /// Applies timeouts, queues heartbeats, and returns what happened since
    /// the last poll. Call it from the game loop, never from a network thread,
    /// so a hung host stops heartbeating.
    pub fn poll(&mut self, now: Duration) -> Vec<HostEvent> {
        for index in 0..self.clients.len() {
            let client = &mut self.clients[index];
            if !client.live {
                continue;
            }
            if now.saturating_sub(client.heard) >= SILENCE_TIMEOUT {
                self.drop_client(index, DropReason::Silent);
                continue;
            }
            if client
                .blocked_since
                .is_some_and(|since| now.saturating_sub(since) >= PROGRESS_TIMEOUT)
            {
                self.drop_client(index, DropReason::Stalled);
                continue;
            }
            if std::mem::take(&mut client.spoke) {
                client.sent = now;
            } else if now.saturating_sub(client.sent) >= HEARTBEAT_INTERVAL {
                client.sent = now;
                self.outgoing
                    .push((client.seat, HostMessage::Heartbeat.encode()));
            }
        }
        std::mem::take(&mut self.events)
    }

    /// Lines to send, each addressed to a client seat.
    pub fn take_outgoing(&mut self) -> Vec<(PlayerId, String)> {
        std::mem::take(&mut self.outgoing)
    }

    fn client_index(&self, seat: PlayerId) -> usize {
        self.clients
            .iter()
            .position(|client| client.seat == seat)
            .unwrap_or_else(|| panic!("{seat:?} is not a client seat"))
    }

    fn acknowledge(&mut self, index: usize, tick: Tick, hash: Option<u64>) {
        let client = &self.clients[index];
        if tick != client.acked + 1 || tick > self.next_tick || hash.is_some() != reports_hash(tick)
        {
            self.drop_client(index, DropReason::Protocol);
            return;
        }
        self.clients[index].acked = tick;
        if let Some(hash) = hash {
            let expected = self
                .hashes
                .iter()
                .find(|(report, _)| *report == tick)
                .map(|(_, hash)| *hash)
                .expect("the host records its hash before a client can report that tick");
            if hash != expected {
                self.halt(self.clients[index].seat, tick);
            }
        }
        self.prune_hashes();
    }

    fn drop_client(&mut self, index: usize, reason: DropReason) {
        let client = &mut self.clients[index];
        client.live = false;
        client.blocked_since = None;
        let seat = client.seat;
        self.pending.push(PlayerCommand {
            player: seat,
            command: Command::Surrender,
        });
        self.events.push(HostEvent::Dropped { seat, reason });
        self.prune_hashes();
    }

    fn halt(&mut self, seat: PlayerId, tick: Tick) {
        self.halted = true;
        self.events.push(HostEvent::Desync { seat, tick });
        self.broadcast(&HostMessage::Desync { tick }.encode());
    }

    fn broadcast(&mut self, line: &str) {
        for client in self.clients.iter_mut().filter(|client| client.live) {
            client.spoke = true;
            self.outgoing.push((client.seat, line.to_owned()));
        }
    }

    fn prune_hashes(&mut self) {
        let oldest = self
            .clients
            .iter()
            .filter(|client| client.live)
            .map(|client| client.acked)
            .min();
        match oldest {
            Some(oldest) => self.hashes.retain(|(tick, _)| *tick > oldest),
            None => self.hashes.clear(),
        }
    }
}

#[cfg(test)]
mod tests {
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
        let later = PROGRESS_TIMEOUT;
        type DropA = fn(&mut HostSession, Duration);
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
}
