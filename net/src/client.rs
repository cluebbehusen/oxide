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
        self.say(ClientMessage::Command { command });
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
        self.say(ClientMessage::Ack {
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

    /// Lines to send to the host.
    pub fn take_outgoing(&mut self) -> Vec<String> {
        std::mem::take(&mut self.outgoing)
    }

    fn say(&mut self, message: ClientMessage) {
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
mod tests {
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
        let first = client.next_batch().unwrap();
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
        let heard = now - secs(1);
        client
            .poll(heard + SILENCE_TIMEOUT - Duration::from_millis(1))
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
}
