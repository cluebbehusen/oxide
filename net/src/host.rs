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
    /// client's progress timeout. Repeating the current state changes
    /// nothing.
    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
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

    /// Whether every live client has acknowledged every published batch, and
    /// so reported every hash the host is waiting on.
    pub fn caught_up(&self) -> bool {
        self.clients
            .iter()
            .filter(|client| client.live)
            .all(|client| client.acked == self.next_tick)
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
mod tests;
