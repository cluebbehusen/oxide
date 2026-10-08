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
    let mut host = Game::networked(Scenario::skirmish(), human, NetRole::Host, viewport()).unwrap();
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

/// Skirmish with both seats human, for a host on seat 0 and a client on
/// seat 1.
fn duel() -> Scenario {
    let mut duel = Scenario::skirmish();
    for player in &mut duel.players {
        player.bot = false;
        player.bot_config = None;
        player.scrap = 500;
    }
    duel
}

fn resting(game: &Game) -> bool {
    (game.presentation.accum - TICK_DT).abs() < 1e-6
}

#[test]
fn a_networked_session_never_journals() {
    let mut host = Game::networked(duel(), PlayerId(0), NetRole::Host, viewport()).unwrap();
    host.recovery_root = Some(std::env::temp_dir().join("oxide-lan-never-journals"));
    host.start_recovery();
    assert!(host.recovery.is_none());
}

#[test]
fn a_networked_session_queues_staged_orders_for_the_host() {
    let mut local = Game::with_viewport(duel(), viewport()).unwrap();
    local.issue(train(&local, PlayerId(0), UnitKind::Harvester));
    assert!(
        local.take_outbox().is_empty(),
        "local sessions send nothing"
    );

    let seat = PlayerId(1);
    let mut client = Game::networked(duel(), seat, NetRole::Client, viewport()).unwrap();
    assert_eq!(client.net_role(), Some(NetRole::Client));
    let order = train(&client, seat, UnitKind::Harvester);
    client.issue(order.clone());
    assert_eq!(client.take_outbox(), vec![order]);
    assert!(client.take_outbox().is_empty());
    assert_eq!(client.pending.len(), 1, "the order stays projected");
}

#[test]
#[should_panic(expected = "stages only its bound seat's orders")]
fn a_networked_session_refuses_another_seats_order() {
    let mut client = Game::networked(duel(), PlayerId(1), NetRole::Client, viewport()).unwrap();
    client.stage(PlayerCommand {
        player: PlayerId(0),
        command: Command::Surrender,
    });
}

#[test]
fn the_host_frame_paces_ticks_and_rests_at_the_lead_cap() {
    let (host_seat, client_seat) = (PlayerId(0), PlayerId(1));
    let mut host = Game::networked(duel(), host_seat, NetRole::Host, viewport()).unwrap();
    let mut session = HostSession::new(host_seat, &[client_seat], Duration::ZERO);
    let now = Duration::ZERO;

    host.host_frame(&mut session, TICK_DT * 0.5, now);
    assert_eq!(host.state.current_tick(), 0, "half a tick is not due");
    host.host_frame(&mut session, TICK_DT * 0.75, now);
    assert_eq!(host.state.current_tick(), 1);
    assert!(
        session.take_outgoing().len() == 1,
        "the batch went to the client"
    );

    host.presentation.paused = true;
    host.host_frame(&mut session, TICK_DT * 4.0, now);
    assert_eq!(host.state.current_tick(), 1, "a paused host seals nothing");
    host.presentation.paused = false;

    for _ in 0..10 {
        host.host_frame(&mut session, TICK_DT * 10.0, now);
    }
    assert_eq!(
        host.state.current_tick(),
        oxide_net::LEAD_CAP,
        "the silent client holds the host at the lead cap"
    );
    assert!(resting(&host), "no debt builds while blocked");
    assert_eq!(host.presentation.render_alpha(), 1.0);
}

#[test]
fn the_host_frame_stops_once_the_match_is_decided() {
    let host_seat = PlayerId(0);
    let mut host = Game::networked(duel(), host_seat, NetRole::Host, viewport()).unwrap();
    let mut session = HostSession::new(host_seat, &[], Duration::ZERO);
    for order in {
        host.issue(Command::Surrender);
        host.take_outbox()
    } {
        session.submit(order);
    }
    host.host_frame(&mut session, TICK_DT * 5.0, Duration::ZERO);
    assert!(
        host.state.result().is_some(),
        "the concession decided the duel"
    );
    let decided = host.state.current_tick();
    host.host_frame(&mut session, TICK_DT * 5.0, Duration::ZERO);
    assert_eq!(host.state.current_tick(), decided);
    assert!(host.pending.is_empty(), "the executed surrender retired");
}

#[test]
fn the_client_frame_paces_waits_and_catches_up() {
    let mut client = Game::networked(duel(), PlayerId(1), NetRole::Client, viewport()).unwrap();
    let mut session = ClientSession::new(Duration::ZERO);
    let batch = |tick| {
        oxide_net::HostMessage::Batch {
            tick,
            commands: Vec::new(),
        }
        .encode()
    };

    client.client_frame(&mut session, TICK_DT * 3.0, CLIENT_BUFFER);
    assert_eq!(client.state.current_tick(), 0, "nothing has arrived");
    assert!(resting(&client), "waiting builds no debt");

    for tick in 0..10 {
        session.receive(&batch(tick), Duration::ZERO).unwrap();
    }
    client.client_frame(&mut session, TICK_DT, CLIENT_BUFFER);
    assert_eq!(session.backlog(), CLIENT_BUFFER, "caught up to the buffer");
    assert_eq!(client.state.current_tick(), 8);

    client.client_frame(&mut session, 0.0, CLIENT_BUFFER);
    assert_eq!(
        client.state.current_tick(),
        8,
        "the buffer waits for its time"
    );
    client.client_frame(&mut session, 0.0, 0);
    assert_eq!(
        client.state.current_tick(),
        10,
        "a closed link drains everything"
    );
    assert_eq!(
        session.take_outgoing().len(),
        10,
        "every batch was acknowledged"
    );
}
