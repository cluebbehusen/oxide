#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the multiplayer shell modes are its first callers"
    )
)]
//! Lockstep sessions: a `Game` bound to one human seat that executes
//! batches assembled by the session host instead of draining its own
//! staged commands.

use super::Game;
use anyhow::{Result, ensure};
use macroquad::prelude::Vec2;
use oxide_sim::{PlayerCommand, PlayerId, Scenario, TickReport};

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
        game.networked = true;
        Ok(game)
    }

    /// Executes one complete batch from the session host.
    ///
    /// A networked session's `pending` holds only its bound seat's orders,
    /// in send order, and the host keeps each seat's orders in arrival
    /// order. Executing the batch therefore retires as many orders from the
    /// front of `pending` as the batch carries for the bound seat; the rest
    /// stay projected until their own batch arrives.
    pub(crate) fn run_batch(&mut self, batch: &[PlayerCommand]) -> TickReport {
        assert!(self.networked, "local sessions tick through do_tick");
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
mod tests {
    use super::*;
    use macroquad::prelude::vec2;
    use oxide_sim::{BuildingId, Command, UnitKind};

    fn viewport() -> Vec2 {
        vec2(1280.0, 800.0)
    }

    fn foundry(game: &Game, seat: PlayerId) -> BuildingId {
        game.state
            .buildings()
            .iter()
            .find(|building| building.player == seat)
            .expect("every seat starts with a Foundry")
            .id
    }

    fn train(game: &Game, seat: PlayerId, kind: UnitKind) -> Command {
        Command::Train {
            building: foundry(game, seat),
            kind,
        }
    }

    fn recorded(game: &Game) -> Vec<(u64, PlayerCommand)> {
        game.recorder
            .commands
            .iter()
            .map(|timed| (timed.tick, timed.command.clone()))
            .collect()
    }

    #[test]
    fn host_batches_play_out_exactly_like_do_tick() {
        let human = PlayerId(0);
        let mut local = Game::with_viewport(Scenario::skirmish(), viewport()).unwrap();
        let mut host =
            Game::networked(Scenario::skirmish(), human, NetRole::Host, viewport()).unwrap();
        assert_eq!(host.bots.len(), 1, "the host runs the scenario's bot seat");
        for tick in 0..240 {
            let order = match tick {
                5 => Some(UnitKind::Harvester),
                120 => Some(UnitKind::Sentinel),
                _ => None,
            };
            if let Some(kind) = order {
                local.issue(train(&local, human, kind));
                host.issue(train(&host, human, kind));
            }
            let mut batch = host.pending.to_vec();
            batch.extend(host.bot_commands());
            let local_report = local.do_tick();
            let host_report = host.run_batch(&batch);
            assert_eq!(host_report.events, local_report.events, "tick {tick}");
            assert_eq!(host.state.hash(), local.state.hash(), "tick {tick}");
        }
        assert_eq!(recorded(&host), recorded(&local));
        assert!(
            recorded(&host)
                .iter()
                .any(|(_, command)| command.player == PlayerId(1)),
            "the bot seat issued orders"
        );
        assert_eq!(host.demo, local.demo);
        assert!(host.demo.trained_fighter);
        assert!(host.pending.is_empty());
    }

    #[test]
    fn a_client_binds_its_seat_and_retires_orders_as_they_execute() {
        let (host_seat, client_seat) = (PlayerId(0), PlayerId(1));
        let mut duel = Scenario::skirmish();
        for player in &mut duel.players {
            player.bot = false;
            player.bot_config = None;
            player.scrap = 500;
        }
        assert!(
            Game::networked(
                Scenario::skirmish(),
                client_seat,
                NetRole::Client,
                viewport()
            )
            .is_err(),
            "a bot seat cannot be bound"
        );
        assert!(
            Game::networked(duel.clone(), PlayerId(9), NetRole::Client, viewport()).is_err(),
            "an absent seat cannot be bound"
        );
        let mut host = Game::networked(duel.clone(), host_seat, NetRole::Host, viewport()).unwrap();
        let mut client = Game::networked(duel, client_seat, NetRole::Client, viewport()).unwrap();
        assert_eq!(client.presentation.human, client_seat);
        assert!(client.bots.is_empty() && host.bots.is_empty());

        let a = train(&client, client_seat, UnitKind::Harvester);
        let b = train(&client, client_seat, UnitKind::Harvester);
        let x = train(&host, host_seat, UnitKind::Harvester);
        client.issue(a.clone());
        client.issue(b.clone());
        host.issue(x.clone());
        let scrap = |game: &Game| {
            game.state.inspect_command_phase(&game.pending, |state| {
                state.scrap(client_seat).expect("the client seat exists")
            })
        };
        let unstaged = client.state.player(client_seat).scrap;

        let first = [
            PlayerCommand {
                player: client_seat,
                command: a,
            },
            PlayerCommand {
                player: host_seat,
                command: x,
            },
        ];
        for game in [&mut host, &mut client] {
            game.run_batch(&first);
        }
        assert_eq!(
            client.pending.to_vec(),
            vec![PlayerCommand {
                player: client_seat,
                command: b.clone(),
            }]
        );
        assert!(host.pending.is_empty());
        assert!(
            scrap(&client) < client.state.player(client_seat).scrap,
            "the unexecuted order stays charged in the projection"
        );
        assert!(client.state.player(client_seat).scrap < unstaged);

        for game in [&mut host, &mut client] {
            game.run_batch(&[]);
        }
        assert_eq!(client.pending.len(), 1, "an empty batch retires nothing");
        let second = [PlayerCommand {
            player: client_seat,
            command: b,
        }];
        for game in [&mut host, &mut client] {
            game.run_batch(&second);
        }
        assert!(client.pending.is_empty());
        assert_eq!(client.state.hash(), host.state.hash());
        assert_eq!(recorded(&client), recorded(&host));
    }

    #[test]
    #[should_panic(expected = "networked sessions execute supplied batches")]
    fn a_networked_session_never_ticks_itself() {
        Game::networked(Scenario::skirmish(), PlayerId(0), NetRole::Host, viewport())
            .unwrap()
            .do_tick();
    }

    #[test]
    #[should_panic(expected = "local sessions tick through do_tick")]
    fn a_local_session_never_runs_supplied_batches() {
        Game::with_viewport(Scenario::skirmish(), viewport())
            .unwrap()
            .run_batch(&[]);
    }
}
