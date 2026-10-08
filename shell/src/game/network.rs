//! Lockstep sessions: a `Game` bound to one human seat that executes
//! batches assembled by the session host instead of draining its own
//! staged commands.

use super::{Game, MAX_TICKS_PER_FRAME, TICK_DT};
use anyhow::{Result, ensure};
use macroquad::prelude::Vec2;
use oxide_net::{ClientSession, HostSession};
use oxide_sim::{Command, PlayerCommand, PlayerId, Scenario, TickReport};
use std::time::Duration;

/// Batches a client keeps queued to absorb jitter while connected.
pub(crate) const CLIENT_BUFFER: usize = 2;

/// Which side of a lockstep session this machine plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetRole {
    /// Seals batches and runs every bot seat.
    Host,
    /// Executes the host's batches and runs no bots.
    Client,
}

impl Game {
    /// A lockstep session bound to `seat`, which must be a human seat. The
    /// host runs the scenario's bots; a client runs none.
    pub(crate) fn networked(
        scenario: Scenario,
        seat: PlayerId,
        role: NetRole,
        viewport: Vec2,
    ) -> Result<Self> {
        let player = scenario.players.get(usize::from(seat.0));
        ensure!(
            player.is_some_and(|player| !player.bot),
            "{seat:?} is not a human seat"
        );
        let mut game = Self::assemble(scenario, viewport, seat, role == NetRole::Host)?;
        game.net = Some(role);
        Ok(game)
    }

    /// This machine's side of a lockstep session, if any.
    pub(crate) fn net_role(&self) -> Option<NetRole> {
        self.net
    }

    /// Orders staged since the last call, to hand to the session host.
    pub(crate) fn take_outbox(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.outbox)
    }

    /// One frame of the host's match at 1x speed: mirrors the host's pause
    /// into the session, then seals, completes, publishes, and runs each due
    /// tick. The gate or a decided match declines a tick without building
    /// debt.
    pub(crate) fn host_frame(&mut self, session: &mut HostSession, dt: f32, now: Duration) {
        session.set_paused(self.presentation.paused);
        if self.presentation.paused {
            return;
        }
        self.pace(dt, |game| {
            if game.state.result().is_some() {
                return false;
            }
            let Some(mut batch) = session.seal(now) else {
                return false;
            };
            batch.extend(game.bot_commands());
            session.publish(&batch);
            game.run_batch(&batch);
            let state = &game.state;
            session.executed(|| state.hash());
            true
        });
        if self.presentation.accum < TICK_DT {
            self.prepare_bot_decision();
        }
    }

    /// One frame of a client's match at 1x speed: runs each due batch that
    /// has arrived, then catches up until at most `keep` batches wait. The
    /// host's pause reaches a client only as batches that stop arriving.
    pub(crate) fn client_frame(&mut self, session: &mut ClientSession, dt: f32, keep: usize) {
        self.pace(dt, |game| game.run_next(session));
        let mut extra = 0;
        while session.backlog() > keep && extra < MAX_TICKS_PER_FRAME && self.run_next(session) {
            extra += 1;
        }
    }

    fn run_next(&mut self, session: &mut ClientSession) -> bool {
        let Some(batch) = session.next_batch() else {
            return false;
        };
        self.run_batch(&batch);
        let state = &self.state;
        session.executed(|| state.hash());
        true
    }

    /// Executes one complete batch from the session host.
    ///
    /// A networked session's `pending` holds only its bound seat's orders,
    /// in send order, and the host keeps each seat's orders in arrival
    /// order. Executing the batch therefore retires as many orders from the
    /// front of `pending` as the batch carries for the bound seat; the rest
    /// stay projected until their own batch arrives.
    pub(crate) fn run_batch(&mut self, batch: &[PlayerCommand]) -> TickReport {
        assert!(self.net.is_some(), "local sessions tick through do_tick");
        assert!(
            self.bot_decision.is_none(),
            "collect bot commands before running a batch"
        );
        let seat = self.presentation.human;
        let executed = batch
            .iter()
            .filter(|command| command.player == seat)
            .count();
        self.pending.0.drain(..executed.min(self.pending.0.len()));
        self.execute(batch, batch.len())
    }
}

#[cfg(test)]
mod tests;
