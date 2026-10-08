use super::*;
use macroquad::prelude::vec2;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use oxide_sim::{BuildingId, Command, UnitKind};
use std::time::Instant;

const COMMIT: &str = "lan-test";
const STEP: Duration = Duration::from_millis(50);
const HOST: PlayerId = PlayerId(0);
const CLIENT: PlayerId = PlayerId(1);

fn viewport() -> Vec2 {
    vec2(1280.0, 800.0)
}

/// Twin Forges with seats 0 and 1 human and seats 2 and 3 host bots.
fn with_bots() -> Scenario {
    let mut scenario = Scenario::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/twin-forges.json"
    ))
    .unwrap();
    for (seat, player) in scenario.players.iter_mut().enumerate() {
        player.bot = seat >= 2;
        player.bot_config = player
            .bot
            .then(|| BotConfig::new(BotDifficulty::Standard, BotStance::Balanced, seat as u64));
    }
    scenario
}

/// Skirmish with both seats human.
fn duel() -> Scenario {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }
    scenario
}

fn foundry(game: &Game, seat: PlayerId) -> BuildingId {
    game.state
        .buildings()
        .iter()
        .find(|building| building.player == seat)
        .expect("every seat starts with a Foundry")
        .id
}

fn train(game: &Game, seat: PlayerId) -> Command {
    Command::Train {
        building: foundry(game, seat),
        kind: UnitKind::Harvester,
    }
}

fn recorded(game: &Game) -> Vec<(u64, oxide_sim::PlayerCommand)> {
    game.recorder
        .commands
        .iter()
        .map(|timed| (timed.tick, timed.command.clone()))
        .collect()
}

fn address(lobby: &Lobby) -> String {
    match lobby {
        Lobby::Host(host) => host.address.clone(),
        Lobby::Client(_) => unreachable!("a host lobby"),
    }
}

/// Polls `poll` until it yields, failing after ten seconds.
fn wait<T>(mut poll: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = poll() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

/// One host and one client, stepped on a synthetic clock over real
/// localhost sockets.
struct Match {
    now: Duration,
    host: (Game, Link),
    client: (Game, Link),
}

impl Match {
    fn start(scenario: Scenario) -> Self {
        Self::start_as(scenario, HOST)
    }

    fn start_as(scenario: Scenario, seat: PlayerId) -> Self {
        let mut host = Lobby::Host(Box::new(
            HostLobby::new("127.0.0.1:0", scenario, seat, COMMIT).unwrap(),
        ));
        let mut client = Lobby::Client(ClientLobby::new(&address(&host), COMMIT));
        let (mut hosted, mut joined) = (None, None);
        wait(|| {
            if hosted.is_none() {
                hosted = host.poll(Duration::ZERO, viewport());
            }
            if joined.is_none() {
                joined = client.poll(Duration::ZERO, viewport());
            }
            (hosted.is_some() && joined.is_some()).then_some(())
        });
        let (Some(host), Some(client)) = (hosted, joined) else {
            unreachable!("both lobbies started");
        };
        Self {
            now: Duration::ZERO,
            host,
            client,
        }
    }

    /// Advances both machines by one tick of time.
    fn step(&mut self) -> (Option<End>, Option<End>) {
        self.now += STEP;
        thread::sleep(Duration::from_millis(1));
        let host = self.host.1.pump(&mut self.host.0, self.now);
        let client = self.client.1.pump(&mut self.client.0, self.now);
        (host, client)
    }

    /// Advances the host alone by one tick of time.
    fn step_host(&mut self) {
        self.now += STEP;
        assert_eq!(self.host.1.pump(&mut self.host.0, self.now), None);
    }

    /// Steps both machines until each has ended.
    fn ends(&mut self) -> (End, End) {
        let (mut host_end, mut client_end) = (None, None);
        wait(|| {
            let (host, client) = self.step();
            host_end = host_end.or(host);
            client_end = client_end.or(client);
            host_end.zip(client_end)
        })
    }

    fn run(&mut self, steps: usize) {
        for _ in 0..steps {
            assert_eq!(self.step(), (None, None));
        }
    }

    /// Pauses the host and lets the client execute what is in flight.
    fn drain(&mut self) {
        self.host.0.presentation.paused = true;
        wait(|| {
            assert_eq!(self.step(), (None, None));
            (self.client.0.state.current_tick() == self.host.0.state.current_tick()).then_some(())
        });
    }
}

#[test]
fn a_hosted_match_with_bots_stays_in_sync() {
    let mut lan = Match::start(with_bots());
    assert_eq!(lan.client.0.presentation.human, CLIENT);
    lan.run(10);
    let order = train(&lan.client.0, CLIENT);
    lan.client.0.issue(order.clone());
    let host_order = train(&lan.host.0, HOST);
    lan.host.0.issue(host_order);
    lan.run(70);
    lan.drain();
    assert!(lan.host.0.state.current_tick() >= 60);
    assert_eq!(lan.client.0.state.hash(), lan.host.0.state.hash());
    assert_eq!(recorded(&lan.client.0), recorded(&lan.host.0));
    assert_eq!(lan.client.0.demo, lan.host.0.demo);
    assert!(
        recorded(&lan.host.0)
            .iter()
            .any(|(_, command)| command.player == CLIENT && command.command == order),
        "the client's order crossed the wire"
    );
    assert!(
        lan.client.0.pending.is_empty(),
        "the executed order retired"
    );
}

#[test]
fn a_client_that_leaves_surrenders_its_seat() {
    let Match {
        mut now,
        host: (mut host, mut link),
        client,
    } = Match::start(with_bots());
    drop(client);
    wait(|| {
        now += STEP;
        assert_eq!(link.pump(&mut host, now), None);
        (!host.state.accepts_commands(CLIENT)).then_some(())
    });
    assert!(host.state.result().is_none(), "the ally fights on");
    assert!(
        host.presentation
            .toasts
            .iter()
            .any(|toast| toast.text.contains("left the match"))
    );
}

#[test]
fn a_client_loses_a_host_that_leaves_undecided() {
    let mut lan = Match::start(duel());
    lan.run(5);
    let Match {
        mut now,
        host,
        client: (mut client, mut link),
    } = lan;
    drop(host);
    let end = wait(|| {
        now += STEP;
        thread::sleep(Duration::from_millis(1));
        link.pump(&mut client, now)
    });
    assert_eq!(end, End::HostLost);
}

#[test]
fn a_decided_match_ends_cleanly_on_both_sides() {
    let mut lan = Match::start(duel());
    lan.run(3);
    lan.client.0.issue(Command::Surrender);
    wait(|| {
        assert_eq!(lan.step(), (None, None));
        matches!(
            &lan.client.1,
            Link::Client(ClientLink {
                connection: None,
                ..
            })
        )
        .then_some(())
    });
    assert!(lan.client.0.state.result().is_some());
    assert_eq!(lan.client.0.state.hash(), lan.host.0.state.hash());
    assert_eq!(recorded(&lan.client.0), recorded(&lan.host.0));
}

#[test]
fn a_host_leaving_a_decided_match_still_delivers_the_result() {
    let mut lan = Match::start(duel());
    lan.run(3);
    lan.host.0.issue(Command::Surrender);
    while lan.host.0.state.result().is_none() {
        lan.step_host();
    }
    let Match {
        mut now,
        host: (host, link),
        client: (mut client, mut client_link),
    } = lan;
    let mut lingering = link.leave(&host, now).expect("a decided host lingers");
    wait(|| {
        now += STEP;
        thread::sleep(Duration::from_millis(1));
        lingering.pump(now);
        assert_eq!(client_link.pump(&mut client, now), None, "no host loss");
        matches!(
            &client_link,
            Link::Client(ClientLink {
                connection: None,
                ..
            })
        )
        .then_some(())
    });
    assert_eq!(client.state.hash(), host.state.hash());
    wait(|| (!lingering.pump(now)).then_some(()));

    let mut lan = Match::start(duel());
    lan.run(3);
    let (host, link) = lan.host;
    assert!(
        link.leave(&host, lan.now).is_none(),
        "undecided: drop at once"
    );
    let (client, link) = lan.client;
    assert!(link.leave(&client, lan.now).is_none());
}

#[test]
fn an_injected_desync_halts_both_machines() {
    let mut lan = Match::start(duel());
    lan.run(5);
    lan.client.0.state.tick(&[]);
    let desync = End::Desync {
        tick: oxide_net::HASH_INTERVAL,
    };
    assert_eq!(lan.ends(), (desync, desync));
    assert!(desync.notice().contains("Desync at tick 20"));
    assert_eq!(End::HostLost.notice(), "Lost connection to the host.");
}

#[test]
fn a_desync_before_the_decisive_tick_still_halts_both_machines() {
    let mut lan = Match::start(duel());
    lan.run(5);
    lan.client.0.state.tick(&[]);
    // The host runs ahead and ends the match before the client
    // reports the diverged tick.
    while lan.host.0.state.current_tick() < oxide_net::HASH_INTERVAL {
        lan.step_host();
    }
    lan.host.0.issue(Command::Surrender);
    while lan.host.0.state.result().is_none() {
        lan.step_host();
    }
    let desync = End::Desync {
        tick: oxide_net::HASH_INTERVAL,
    };
    assert_eq!(lan.ends(), (desync, desync));
}

#[test]
fn a_mismatched_build_is_refused_and_its_seat_stays_open() {
    let mut host = Lobby::Host(Box::new(
        HostLobby::new("127.0.0.1:0", duel(), HOST, COMMIT).unwrap(),
    ));
    let mut client = Lobby::Client(ClientLobby::new(&address(&host), "other"));
    let reason = wait(|| {
        assert!(host.poll(Duration::ZERO, viewport()).is_none());
        assert!(client.poll(Duration::ZERO, viewport()).is_none());
        client.failure().map(|(_, reason)| reason.to_owned())
    });
    assert!(reason.contains("host runs build lan-test"));
    assert_eq!(client.status(), reason);
    assert!(host.status().ends_with("1 of 2 players here"));
    assert!(host.failure().is_none());
}

#[test]
fn a_host_needs_its_own_seat_and_another_human_seat() {
    assert!(HostLobby::new("127.0.0.1:0", Scenario::skirmish(), HOST, COMMIT).is_err());
    assert!(HostLobby::new("127.0.0.1:0", duel(), PlayerId(2), COMMIT).is_err());
}

#[test]
fn a_host_can_sit_in_any_human_seat() {
    let mut lan = Match::start_as(duel(), CLIENT);
    assert_eq!(lan.host.0.presentation.human, CLIENT);
    assert_eq!(lan.client.0.presentation.human, HOST);
    lan.run(5);
    lan.drain();
    assert_eq!(lan.client.0.state.hash(), lan.host.0.state.hash());
}

#[test]
fn a_typed_address_gets_the_default_port_when_it_names_none() {
    for (typed, full) in [
        ("192.168.1.20", "192.168.1.20:4200"),
        (" connor-mbp ", "connor-mbp:4200"),
        ("connor-mbp:5000", "connor-mbp:5000"),
        ("10.0.0.2:4201", "10.0.0.2:4201"),
        ("fd7a::1", "[fd7a::1]:4200"),
        ("[fd7a::1]:4201", "[fd7a::1]:4201"),
    ] {
        assert_eq!(with_default_port(typed), full, "{typed}");
    }
}

#[test]
fn a_wildcard_bind_shows_a_reachable_address() {
    let loopback: SocketAddr = "127.0.0.1:4300".parse().unwrap();
    assert_eq!(reachable(loopback), "127.0.0.1:4300");
    let shown = reachable("0.0.0.0:4300".parse().unwrap());
    assert!(
        shown.ends_with("4300") && !shown.starts_with("0.0.0.0"),
        "{shown}"
    );
}
