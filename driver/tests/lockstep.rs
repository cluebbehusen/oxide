//! In-process lockstep: a host and two clients exchange `oxide-net` lines
//! through delayed links on a virtual clock. Every seat's "player" is a
//! scripted controller running on that seat's own machine, so real orders
//! cross the wire. Every machine must execute identical batches.

use chassis::rng::Pcg32;
use oxide_bot::{PublicMapBriefing, SeatBot};
use oxide_kit::{GameReplay, bot_execution, runner};
use oxide_net::{
    ClientEnd, ClientSession, DropReason, HostEvent, HostSession, PROGRESS_TIMEOUT,
    SILENCE_TIMEOUT, reports_hash,
};
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use oxide_sim::{Command, PlayerCommand, PlayerId, SIM_VERSION, Scenario, State, Tick};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const STEP: Duration = Duration::from_millis(10);
const TICK: Duration = Duration::from_millis(50);
const HOST: PlayerId = PlayerId(0);
const CLIENTS: [PlayerId; 2] = [PlayerId(1), PlayerId(2)];

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn player_config() -> BotConfig {
    BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 0)
}

/// Twin Forges with seats 0 to 2 played by people and seat 3 a host bot.
fn scenario() -> Scenario {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios/twin-forges.json");
    let mut scenario = Scenario::load(&path).unwrap();
    for (seat, player) in scenario.players.iter_mut().enumerate() {
        player.bot = seat == 3;
        player.bot_config = Some(player_config());
    }
    scenario
}

/// One direction of a connection: in-order delivery after a jittered delay.
struct Link {
    latency: Duration,
    jitter: u32,
    rng: Pcg32,
    queue: VecDeque<(Duration, String)>,
    last: Duration,
    closed: bool,
}

impl Link {
    fn new(latency: Duration, jitter_ms: u32, stream: u64) -> Self {
        assert!(latency >= ms(u64::from(jitter_ms)));
        Self {
            latency,
            jitter: jitter_ms,
            rng: Pcg32::new(57, stream),
            queue: VecDeque::new(),
            last: Duration::ZERO,
            closed: false,
        }
    }

    fn send(&mut self, now: Duration, line: String) {
        if self.closed {
            return;
        }
        let spread = u64::from(self.rng.next_below(2 * self.jitter + 1));
        let at = (now + self.latency + ms(spread) - ms(u64::from(self.jitter))).max(self.last);
        self.last = at;
        self.queue.push_back((at, line));
    }

    fn deliver(&mut self, now: Duration) -> Vec<String> {
        let mut lines = Vec::new();
        while self.queue.front().is_some_and(|(at, _)| *at <= now) {
            lines.push(self.queue.pop_front().unwrap().1);
        }
        lines
    }

    fn close(&mut self) {
        self.closed = true;
        self.queue.clear();
    }
}

/// A machine's world, its record of executed batches, and its seat's player.
struct Machine {
    seat: PlayerId,
    state: State,
    replay: GameReplay,
    player: SeatBot,
    acted: Option<Tick>,
    hashes: Vec<(Tick, u64)>,
}

impl Machine {
    fn new(scenario: &Scenario, seat: PlayerId) -> Self {
        let briefing = Arc::new(PublicMapBriefing::from_scenario(scenario).unwrap());
        Self {
            seat,
            state: scenario.build().unwrap(),
            replay: GameReplay::new(SIM_VERSION, scenario.clone()),
            player: SeatBot::scripted(seat, player_config(), briefing),
            acted: None,
            hashes: Vec::new(),
        }
    }

    /// The player's orders from this machine's current world, once per tick.
    fn orders(&mut self) -> Vec<Command> {
        let tick = self.state.current_tick();
        if self.acted.replace(tick) == Some(tick) {
            return Vec::new();
        }
        self.player
            .act(&self.state)
            .into_iter()
            .map(|order| {
                assert_eq!(order.player, self.seat);
                order.command
            })
            .collect()
    }

    fn execute(&mut self, batch: Vec<PlayerCommand>) {
        runner::record_and_tick(&mut self.state, batch, Some(&mut self.replay));
        let tick = self.state.current_tick();
        if reports_hash(tick) {
            self.hashes.push((tick, self.state.hash()));
        }
    }

    fn commands(&self) -> Vec<(Tick, PlayerCommand)> {
        self.replay
            .commands
            .iter()
            .map(|timed| (timed.tick, timed.command.clone()))
            .collect()
    }
}

struct Host {
    session: HostSession,
    machine: Machine,
    bots: Vec<SeatBot>,
    next_due: Duration,
    frozen: bool,
    blocked: usize,
    events: Vec<(Duration, HostEvent)>,
}

struct Client {
    session: ClientSession,
    machine: Machine,
    up: Link,
    down: Link,
    stuck: Option<(Duration, Duration)>,
    doctor_at: Option<Tick>,
    end: Option<(Duration, ClientEnd)>,
}

impl Client {
    fn stuck(&self, now: Duration) -> bool {
        self.stuck
            .is_some_and(|(from, until)| from <= now && now < until)
    }

    fn step(&mut self, now: Duration) {
        if self.end.is_some() || self.down.closed {
            return;
        }
        for line in self.down.deliver(now) {
            if let Err(end) = self.session.receive(&line, now) {
                self.end = Some((now, end));
                return;
            }
        }
        if !self.stuck(now) {
            while let Some(mut batch) = self.session.next_batch() {
                for order in self.machine.orders() {
                    self.session.send(order);
                }
                if self.doctor_at == Some(self.machine.state.current_tick()) {
                    batch.push(PlayerCommand {
                        player: self.machine.seat,
                        command: Command::Surrender,
                    });
                }
                self.machine.execute(batch);
                let state = &self.machine.state;
                self.session.executed(|| state.hash());
            }
        }
        if let Err(end) = self.session.poll(now) {
            self.end = Some((now, end));
            return;
        }
        for line in self.session.take_outgoing() {
            self.up.send(now, line);
        }
    }
}

struct Net {
    now: Duration,
    host: Host,
    clients: Vec<Client>,
}

impl Net {
    /// Clients 1 and 2 with the given one-way latency and jitter per link.
    fn new(links: [(u64, u32); 2]) -> Self {
        let scenario = scenario();
        let host = Host {
            session: HostSession::new(HOST, &CLIENTS, Duration::ZERO),
            machine: Machine::new(&scenario, HOST),
            bots: oxide_bot::seat_bots(&scenario).unwrap(),
            next_due: Duration::ZERO,
            frozen: false,
            blocked: 0,
            events: Vec::new(),
        };
        assert_eq!(host.bots.len(), 1, "only seat 3 is a host bot");
        let clients = CLIENTS
            .iter()
            .zip(links)
            .zip(0u64..)
            .map(|((&seat, (latency, jitter)), stream)| Client {
                session: ClientSession::new(Duration::ZERO),
                machine: Machine::new(&scenario, seat),
                up: Link::new(ms(latency), jitter, 2 * stream),
                down: Link::new(ms(latency), jitter, 2 * stream + 1),
                stuck: None,
                doctor_at: None,
                end: None,
            })
            .collect();
        Self {
            now: Duration::ZERO,
            host,
            clients,
        }
    }

    fn client(&mut self, seat: PlayerId) -> &mut Client {
        &mut self.clients[usize::from(seat.0) - 1]
    }

    fn run_until(&mut self, until: Duration) {
        while self.now < until {
            self.step();
            self.now += STEP;
        }
    }

    /// Runs until the host has executed `ticks` ticks.
    fn run_ticks(&mut self, ticks: Tick) {
        while self.host.machine.state.current_tick() < ticks {
            self.step();
            self.now += STEP;
            assert!(
                self.now < Duration::from_secs(120),
                "the host never got there"
            );
        }
    }

    /// Stops sealing and lets every live client execute what is in flight.
    fn drain(&mut self) {
        self.host.session.set_paused(true);
        self.run_until(self.now + Duration::from_secs(2));
    }

    fn step(&mut self) {
        let now = self.now;
        if !self.host.frozen {
            self.host_step(now);
        }
        for client in &mut self.clients {
            client.step(now);
        }
    }

    fn host_step(&mut self, now: Duration) {
        let host = &mut self.host;
        for (client, &seat) in self.clients.iter_mut().zip(&CLIENTS) {
            for line in client.up.deliver(now) {
                host.session.receive(seat, &line, now);
            }
        }
        for event in host.session.poll(now) {
            if let HostEvent::Dropped { seat, .. } = event {
                let client = &mut self.clients[usize::from(seat.0) - 1];
                client.up.close();
                client.down.close();
            }
            host.events.push((now, event));
        }
        if now >= host.next_due {
            for order in host.machine.orders() {
                host.session.submit(order);
            }
            match host.session.seal(now) {
                Some(mut batch) => {
                    batch.extend(bot_execution::commands(&host.machine.state, &mut host.bots));
                    host.session.publish(&batch);
                    host.machine.execute(batch);
                    let state = &host.machine.state;
                    host.session.executed(|| state.hash());
                    host.next_due = now + TICK;
                }
                None => host.blocked += 1,
            }
        }
        for (seat, line) in host.session.take_outgoing() {
            self.clients[usize::from(seat.0) - 1].down.send(now, line);
        }
    }

    /// The host and the named clients executed identical batches and agree
    /// on every report-tick hash.
    fn assert_in_sync(&self, seats: &[PlayerId]) {
        let host = &self.host.machine;
        for &seat in seats {
            let client = &self.clients[usize::from(seat.0) - 1].machine;
            assert_eq!(
                client.state.current_tick(),
                host.state.current_tick(),
                "{seat:?}"
            );
            assert_eq!(client.commands(), host.commands(), "{seat:?}");
            assert_eq!(client.hashes, host.hashes, "{seat:?}");
            assert_eq!(client.state.hash(), host.state.hash(), "{seat:?}");
        }
    }

    fn surrender_tick(&self, seat: PlayerId) -> Option<Tick> {
        self.host
            .machine
            .commands()
            .into_iter()
            .find(|(_, order)| order.player == seat && order.command == Command::Surrender)
            .map(|(tick, _)| tick)
    }
}

#[test]
fn healthy_links_never_stall_and_every_machine_agrees() {
    let mut net = Net::new([(40, 20), (90, 20)]);
    net.run_ticks(400);
    assert_eq!(net.host.blocked, 0, "the lead cap absorbed normal latency");
    net.drain();
    assert!(net.host.events.is_empty(), "{:?}", net.host.events);
    assert!(net.clients.iter().all(|client| client.end.is_none()));
    assert!(
        net.host
            .machine
            .commands()
            .iter()
            .any(|(_, order)| CLIENTS.contains(&order.player)),
        "client orders crossed the wire"
    );
    net.assert_in_sync(&CLIENTS);
}

#[test]
fn a_short_client_stall_makes_the_host_wait_without_dropping_anyone() {
    let mut net = Net::new([(40, 20), (40, 20)]);
    net.client(CLIENTS[0]).stuck = Some((Duration::from_secs(2), Duration::from_secs(6)));
    net.run_ticks(300);
    assert!(net.host.blocked > 0, "the host waited at the lead cap");
    net.drain();
    assert!(net.host.events.is_empty(), "{:?}", net.host.events);
    net.assert_in_sync(&CLIENTS);
}

#[test]
fn a_stuck_client_with_a_live_network_is_dropped_and_surrenders() {
    let mut net = Net::new([(40, 20), (40, 20)]);
    let stuck = CLIENTS[1];
    net.client(stuck).stuck = Some((Duration::from_secs(2), Duration::MAX));
    net.run_until(Duration::from_secs(20));
    let (dropped_at, event) = net.host.events[0];
    assert_eq!(
        event,
        HostEvent::Dropped {
            seat: stuck,
            reason: DropReason::Stalled
        }
    );
    assert_eq!(net.host.events.len(), 1);
    assert!(dropped_at >= Duration::from_secs(4) + PROGRESS_TIMEOUT - ms(500));
    assert!(
        net.clients[0].end.is_none(),
        "the waiting host kept heartbeating"
    );
    let stuck_tick = net.clients[1].machine.state.current_tick();
    assert_eq!(
        net.surrender_tick(stuck),
        Some(stuck_tick + oxide_net::LEAD_CAP),
        "the first tick past the lead cap carries the Surrender, without the stuck ack"
    );
    assert!(!net.host.machine.state.accepts_commands(stuck));
    net.run_until(net.now + Duration::from_secs(2));
    net.drain();
    net.assert_in_sync(&CLIENTS[..1]);
    let stuck_commands = net.clients[1].machine.commands();
    assert!(net.host.machine.commands().starts_with(&stuck_commands));
}

#[test]
fn clients_end_the_session_when_the_host_falls_silent() {
    let mut net = Net::new([(40, 20), (90, 20)]);
    net.run_until(Duration::from_secs(2));
    net.host.frozen = true;
    let frozen_at = net.now;
    net.run_until(frozen_at + SILENCE_TIMEOUT + Duration::from_secs(1));
    for client in &net.clients {
        let (ended_at, end) = client.end.expect("the client noticed");
        assert_eq!(end, ClientEnd::HostSilent);
        assert!(ended_at >= frozen_at + SILENCE_TIMEOUT, "ended early");
        assert!(
            ended_at <= frozen_at + SILENCE_TIMEOUT + ms(200),
            "ended late"
        );
    }
}

#[test]
fn a_closed_connection_surrenders_the_seat_and_the_match_continues() {
    let mut net = Net::new([(40, 20), (40, 20)]);
    let gone = CLIENTS[1];
    net.run_until(Duration::from_secs(3));
    net.client(gone).up.close();
    net.client(gone).down.close();
    net.host.session.disconnected(gone);
    net.run_until(Duration::from_secs(8));
    assert_eq!(
        net.host
            .events
            .iter()
            .map(|(_, event)| *event)
            .collect::<Vec<_>>(),
        vec![HostEvent::Dropped {
            seat: gone,
            reason: DropReason::Closed
        }]
    );
    assert!(net.surrender_tick(gone).is_some());
    assert!(!net.host.machine.state.accepts_commands(gone));
    net.drain();
    net.assert_in_sync(&CLIENTS[..1]);
}

#[test]
fn a_diverged_client_halts_every_machine_at_the_next_report() {
    let mut net = Net::new([(40, 20), (40, 20)]);
    let report = 2 * oxide_net::HASH_INTERVAL;
    net.client(CLIENTS[0]).doctor_at = Some(report - 5);
    net.run_until(Duration::from_secs(6));
    assert_eq!(
        net.host
            .events
            .iter()
            .map(|(_, event)| *event)
            .collect::<Vec<_>>(),
        vec![HostEvent::Desync {
            seat: CLIENTS[0],
            tick: report
        }]
    );
    for client in &net.clients {
        assert_eq!(
            client.end.map(|(_, end)| end),
            Some(ClientEnd::Desync { tick: report })
        );
    }
    let halted = net.host.machine.state.current_tick();
    net.run_until(net.now + Duration::from_secs(1));
    assert_eq!(
        net.host.machine.state.current_tick(),
        halted,
        "the host stopped sealing"
    );
}
