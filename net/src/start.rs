//! The start barrier: from a frozen roster to tick zero on every machine.

use crate::START_TIMEOUT;
use crate::host::HostSession;
use crate::message::{JoinMessage, LobbyMessage};
use oxide_sim::{PlayerId, Scenario};
use std::time::Duration;

/// Why a start was abandoned. The host closes every seated connection and
/// keeps listening; clients rejoin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFailed {
    /// Not every client was ready within [`START_TIMEOUT`].
    TimedOut,
    /// The client's world at tick zero hashed differently from the host's.
    Mismatch {
        /// The client's seat.
        seat: PlayerId,
    },
    /// The client sent something other than one decodable Ready.
    Protocol {
        /// The client's seat.
        seat: PlayerId,
    },
}

/// The host's half of starting a match once the roster is frozen.
///
/// The barrier queues each client's Start, collects every client's Ready,
/// and compares each tick-zero hash with the host's. Once all are in, it
/// queues Go and hands over the [`HostSession`], whose liveness clocks start
/// then. Send the Go lines before anything the session queues.
#[derive(Debug)]
pub struct StartBarrier {
    host: PlayerId,
    clients: Vec<PlayerId>,
    hash: u64,
    deadline: Duration,
    ready: Vec<bool>,
    started: bool,
    outgoing: Vec<(PlayerId, String)>,
}

impl StartBarrier {
    /// Starts `scenario` with the host on `host` and clients on `clients`,
    /// which together must hold exactly the scenario's human seats.
    /// `host_hash` is the host's `State::hash` at tick zero.
    pub fn new(
        scenario: &Scenario,
        host: PlayerId,
        clients: &[PlayerId],
        host_hash: u64,
        now: Duration,
    ) -> Self {
        let mut seated: Vec<PlayerId> = clients.iter().copied().chain([host]).collect();
        seated.sort_unstable();
        let humans: Vec<PlayerId> = scenario
            .players
            .iter()
            .zip(0..)
            .filter(|(player, _)| !player.bot)
            .map(|(_, seat)| PlayerId(seat))
            .collect();
        assert_eq!(
            seated, humans,
            "the host and clients must hold exactly the scenario's human seats"
        );
        let outgoing = clients
            .iter()
            .map(|&seat| {
                let start = LobbyMessage::Start {
                    seat,
                    scenario: Box::new(scenario.clone()),
                };
                (seat, start.encode())
            })
            .collect();
        Self {
            host,
            clients: clients.to_vec(),
            hash: host_hash,
            deadline: now + START_TIMEOUT,
            ready: vec![false; clients.len()],
            started: false,
            outgoing,
        }
    }

    /// Handles one line received from `seat`'s connection. `seat` must be one
    /// of the clients.
    pub fn receive(&mut self, seat: PlayerId, line: &str) -> Result<(), StartFailed> {
        let index = self
            .clients
            .iter()
            .position(|&client| client == seat)
            .unwrap_or_else(|| panic!("{seat:?} is not a client seat"));
        match JoinMessage::decode(line) {
            Ok(JoinMessage::Ready { hash }) if !self.ready[index] => {
                if hash != self.hash {
                    return Err(StartFailed::Mismatch { seat });
                }
                self.ready[index] = true;
                Ok(())
            }
            _ => Err(StartFailed::Protocol { seat }),
        }
    }

    /// The session once every client is ready, with Go queued for each;
    /// `None` while waiting.
    pub fn poll(&mut self, now: Duration) -> Result<Option<HostSession>, StartFailed> {
        assert!(!self.started, "the match already started");
        if self.ready.iter().all(|&ready| ready) {
            self.started = true;
            let go = LobbyMessage::Go.encode();
            self.outgoing
                .extend(self.clients.iter().map(|&seat| (seat, go.clone())));
            return Ok(Some(HostSession::new(self.host, &self.clients, now)));
        }
        if now >= self.deadline {
            return Err(StartFailed::TimedOut);
        }
        Ok(None)
    }

    /// Lines to send, each addressed to a client seat.
    pub fn take_outgoing(&mut self) -> Vec<(PlayerId, String)> {
        std::mem::take(&mut self.outgoing)
    }
}

#[cfg(test)]
mod tests;
