//! The client side: the batch gate, acknowledgements, and host liveness.

use crate::message::{ClientMessage, HostMessage};
use crate::{HEARTBEAT_INTERVAL, SILENCE_TIMEOUT, reports_hash};
use oxide_sim::{Command, PlayerCommand, Tick};
use std::collections::VecDeque;
use std::time::Duration;

/// Why a client session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientEnd {
    /// Nothing arrived from the host for [`SILENCE_TIMEOUT`].
    HostSilent,
    /// The host halted the session on a hash mismatch at `tick`.
    Desync {
        /// The report tick that disagreed.
        tick: Tick,
    },
    /// The host sent a line that failed to decode or broke the protocol.
    Protocol,
}

/// The client's half of a lockstep session, starting at tick zero.
///
/// Each game-loop step, the client executes every batch
/// [`ClientSession::next_batch`] yields, calling
/// [`ClientSession::executed`] after each one.
#[derive(Debug)]
pub struct ClientSession {
    /// Received batches not yet handed out, oldest first.
    batches: VecDeque<Vec<PlayerCommand>>,
    /// The tick of the next batch the host may send.
    next_batch: Tick,
    /// The world tick after the last executed batch.
    executed: Tick,
    executing: bool,
    heard: Duration,
    sent: Duration,
    spoke: bool,
    end: Option<ClientEnd>,
    outgoing: Vec<String>,
}

impl ClientSession {
    /// A session whose world is at tick zero.
    pub fn new(now: Duration) -> Self {
        Self {
            batches: VecDeque::new(),
            next_batch: 0,
            executed: 0,
            executing: false,
            heard: now,
            sent: now,
            spoke: false,
            end: None,
            outgoing: Vec::new(),
        }
    }

    /// Handles one line received from the host.
    pub fn receive(&mut self, line: &str, now: Duration) -> Result<(), ClientEnd> {
        self.ended()?;
        self.heard = now;
        match HostMessage::decode(line) {
            Ok(HostMessage::Batch { tick, commands }) if tick == self.next_batch => {
                self.next_batch += 1;
                self.batches.push_back(commands);
                Ok(())
            }
            Ok(HostMessage::Heartbeat) => Ok(()),
            Ok(HostMessage::Desync { tick }) => self.end(ClientEnd::Desync { tick }),
            Ok(HostMessage::Batch { .. }) | Err(_) => self.end(ClientEnd::Protocol),
        }
    }

    /// Sends an order for this client's seat.
    pub fn send(&mut self, command: Command) {
        self.say(&ClientMessage::Command { command });
    }

    /// The next batch to execute, if it has arrived. Execute it on the
    /// world's current tick, then call [`ClientSession::executed`].
    pub fn next_batch(&mut self) -> Option<Vec<PlayerCommand>> {
        assert!(!self.executing, "report the previous batch as executed");
        if self.end.is_some() {
            return None;
        }
        let batch = self.batches.pop_front()?;
        self.executing = true;
        Some(batch)
    }

    /// Acknowledges the batch just executed. `hash` runs only when the
    /// resulting tick is a report tick.
    pub fn executed(&mut self, hash: impl FnOnce() -> u64) {
        assert!(
            std::mem::take(&mut self.executing),
            "execute a batch from next_batch first"
        );
        self.executed += 1;
        let tick = self.executed;
        self.say(&ClientMessage::Ack {
            tick,
            hash: reports_hash(tick).then(hash),
        });
    }

    /// Applies the host silence timeout and queues a heartbeat when the
    /// client has sent nothing recently.
    pub fn poll(&mut self, now: Duration) -> Result<(), ClientEnd> {
        self.ended()?;
        if now.saturating_sub(self.heard) >= SILENCE_TIMEOUT {
            return self.end(ClientEnd::HostSilent);
        }
        if std::mem::take(&mut self.spoke) {
            self.sent = now;
        } else if now.saturating_sub(self.sent) >= HEARTBEAT_INTERVAL {
            self.sent = now;
            self.outgoing.push(ClientMessage::Heartbeat.encode());
        }
        Ok(())
    }

    /// How many received batches are waiting to execute.
    pub fn backlog(&self) -> usize {
        self.batches.len()
    }

    /// Lines to send to the host.
    pub fn take_outgoing(&mut self) -> Vec<String> {
        std::mem::take(&mut self.outgoing)
    }

    fn say(&mut self, message: &ClientMessage) {
        if self.end.is_none() {
            self.spoke = true;
            self.outgoing.push(message.encode());
        }
    }

    fn ended(&self) -> Result<(), ClientEnd> {
        self.end.map_or(Ok(()), Err)
    }

    fn end(&mut self, end: ClientEnd) -> Result<(), ClientEnd> {
        self.end = Some(end);
        Err(end)
    }
}

#[cfg(test)]
mod tests;
