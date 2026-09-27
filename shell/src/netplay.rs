//! LAN play: the lobby that gathers machines and starts a match, and the
//! link that carries a running match between them. The frame loop polls
//! both; sockets run on `oxide_net::tcp` threads.

use crate::game::Game;
use crate::game::network::{CLIENT_BUFFER, NetRole};
use anyhow::{Context, Result, ensure};
use macroquad::prelude::Vec2;
use oxide_net::{
    ClientEnd, ClientSession, Closed, Connection, DEFAULT_PORT, HostEvent, HostSession,
    JoinMessage, Listener, LobbyMessage, StartBarrier, same_build,
};
use oxide_sim::{PlayerId, Scenario, Tick};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// After this long without a new tick, the waiting player is told why.
const WAIT_NOTICE: Duration = Duration::from_millis(500);

/// How long a host keeps its connections open after halting on a desync,
/// so every client hears why.
const DESYNC_GRACE: Duration = Duration::from_secs(2);

/// Why a running match ended for this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// The host is gone and the match was not decided.
    HostLost,
    /// A hash report disagreed and every machine halted.
    Desync {
        /// The report tick that disagreed.
        tick: Tick,
    },
}

impl End {
    /// What to tell the player.
    pub(crate) fn notice(self) -> String {
        match self {
            End::HostLost => "Lost connection to the host.".to_owned(),
            End::Desync { tick } => {
                format!("Desync at tick {tick}: the match halted and its replay was saved.")
            }
        }
    }
}

/// A running match's connections and session.
pub(crate) enum Link {
    /// This machine hosts.
    Host(HostLink),
    /// This machine joined a host.
    Client(ClientLink),
}

/// The host's connections, one per client seat.
pub(crate) struct HostLink {
    session: HostSession,
    peers: Vec<(PlayerId, Connection)>,
    /// When the host finished every connection, once the match was decided
    /// and every client had acknowledged it, or on a desync.
    closing: Option<(Duration, Option<End>)>,
    clock: Pacing,
}

/// The client's connection to its host.
pub(crate) struct ClientLink {
    session: ClientSession,
    connection: Option<Connection>,
    closed: bool,
    clock: Pacing,
}

/// Frame timing and the last time the match advanced.
struct Pacing {
    last: Duration,
    tick: Tick,
    moved: Duration,
}

impl Pacing {
    fn new(now: Duration) -> Self {
        Self {
            last: now,
            tick: 0,
            moved: now,
        }
    }

    /// Seconds since the previous frame.
    fn step(&mut self, now: Duration) -> f32 {
        let dt = now.saturating_sub(self.last).as_secs_f32();
        self.last = now;
        dt
    }

    /// Whether the match has not advanced for a while.
    fn stalled(&mut self, game: &Game, now: Duration) -> bool {
        let tick = game.state.current_tick();
        if tick != self.tick {
            self.tick = tick;
            self.moved = now;
        }
        game.state.result().is_none()
            && !game.presentation.paused
            && now.saturating_sub(self.moved) >= WAIT_NOTICE
    }
}

impl Link {
    fn host(session: HostSession, peers: Vec<(PlayerId, Connection)>, now: Duration) -> Self {
        Self::Host(HostLink {
            session,
            peers,
            closing: None,
            clock: Pacing::new(now),
        })
    }

    fn client(session: ClientSession, connection: Connection, now: Duration) -> Self {
        Self::Client(ClientLink {
            session,
            connection: Some(connection),
            closed: false,
            clock: Pacing::new(now),
        })
    }

    /// Moves lines, runs due ticks, and reports how the match ended, once.
    pub(crate) fn pump(&mut self, game: &mut Game, now: Duration) -> Option<End> {
        match self {
            Link::Host(host) => host.pump(game, now),
            Link::Client(client) => client.pump(game, now),
        }
    }
}

impl HostLink {
    fn pump(&mut self, game: &mut Game, now: Duration) -> Option<End> {
        let dt = self.clock.step(now);
        if let Some((since, end)) = self.closing {
            self.peers
                .retain(|(_, connection)| drain(connection).is_ok());
            let done = self.peers.is_empty() || now.saturating_sub(since) >= DESYNC_GRACE;
            if done && end.is_some() {
                self.peers.clear();
                self.closing = Some((since, None));
                return end;
            }
            return None;
        }

        let mut closed = Vec::new();
        for (seat, connection) in &self.peers {
            loop {
                match connection.try_recv() {
                    Ok(Some(line)) => self.session.receive(*seat, &line, now),
                    Ok(None) => break,
                    Err(Closed) => {
                        self.session.disconnected(*seat);
                        closed.push(*seat);
                        break;
                    }
                }
            }
        }
        self.peers.retain(|(seat, _)| !closed.contains(seat));
        for order in game.take_outbox() {
            self.session.submit(order);
        }

        game.host_frame(&mut self.session, dt, now);

        let mut end = None;
        for event in self.session.poll(now) {
            match event {
                HostEvent::Dropped { seat, .. } => {
                    self.peers.retain(|(peer, _)| *peer != seat);
                    let name = &game.scenario.players[usize::from(seat.0)].name;
                    game.presentation.toast(format!("{name} left the match."));
                }
                HostEvent::Desync { tick, .. } => end = Some(End::Desync { tick }),
            }
        }
        for (seat, line) in self.session.take_outgoing() {
            if let Some((_, connection)) = self.peers.iter().find(|(peer, _)| *peer == seat) {
                connection.send(&line);
            }
        }
        // A decided match still waits for the final acknowledgements, so
        // a divergence in the last report ticks is caught, not dropped.
        let settled = game.state.result().is_some() && self.session.caught_up();
        if end.is_some() || settled {
            for (_, connection) in &mut self.peers {
                connection.finish();
            }
            self.closing = Some((now, end));
        } else if self.clock.stalled(game, now) {
            game.presentation.toast("Waiting for players...");
        }
        None
    }
}

impl ClientLink {
    fn pump(&mut self, game: &mut Game, now: Duration) -> Option<End> {
        let dt = self.clock.step(now);
        let connection = self.connection.as_ref()?;
        if !self.closed {
            loop {
                match connection.try_recv() {
                    Ok(Some(line)) => {
                        if let Err(end) = self.session.receive(&line, now) {
                            return self.end(end);
                        }
                    }
                    Ok(None) => break,
                    Err(Closed) => {
                        self.closed = true;
                        break;
                    }
                }
            }
        }
        for order in game.take_outbox() {
            self.session.send(order);
        }

        let keep = if self.closed { 0 } else { CLIENT_BUFFER };
        game.client_frame(&mut self.session, dt, keep);

        if self.closed {
            if self.session.backlog() > 0 {
                return None;
            }
            self.connection = None;
            return game.state.result().is_none().then_some(End::HostLost);
        }
        if let Err(end) = self.session.poll(now) {
            return self.end(end);
        }
        for line in self.session.take_outgoing() {
            connection.send(&line);
        }
        if self.clock.stalled(game, now) {
            game.presentation.toast("Waiting for the host...");
        }
        None
    }

    fn end(&mut self, end: ClientEnd) -> Option<End> {
        self.connection = None;
        Some(match end {
            ClientEnd::Desync { tick } => End::Desync { tick },
            ClientEnd::HostSilent | ClientEnd::Protocol => End::HostLost,
        })
    }
}

/// Reads and discards whatever arrived, reporting whether the connection
/// is still open.
fn drain(connection: &Connection) -> Result<(), Closed> {
    while connection.try_recv()?.is_some() {}
    Ok(())
}

/// Gathering machines until a match starts.
pub(crate) enum Lobby {
    /// This machine hosts.
    Host(Box<HostLobby>),
    /// This machine joins a host.
    Client(ClientLobby),
}

impl Lobby {
    /// Polls the lobby; returns the match once it starts.
    pub(crate) fn poll(&mut self, now: Duration, viewport: Vec2) -> Option<(Game, Link)> {
        match self {
            Lobby::Host(host) => host.poll(now, viewport),
            Lobby::Client(client) => client.poll(now, viewport),
        }
    }

    /// One line describing where the lobby stands.
    pub(crate) fn status(&self) -> String {
        match self {
            Lobby::Host(host) => host.status(),
            Lobby::Client(client) => client.status(),
        }
    }

    /// A client that gave up: the address it tried, and why it failed.
    pub(crate) fn failure(&self) -> Option<(&str, &str)> {
        match self {
            Lobby::Client(ClientLobby {
                address,
                state: Joining::Failed(reason),
                ..
            }) => Some((address, reason)),
            _ => None,
        }
    }
}

/// `address` with the default port when it names none.
pub(crate) fn with_default_port(address: &str) -> String {
    let address = address.trim();
    let has_port = address.parse::<SocketAddr>().is_ok()
        || address
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.contains(':') && port.parse::<u16>().is_ok());
    if has_port {
        address.to_owned()
    } else if address.parse::<Ipv6Addr>().is_ok() {
        format!("[{address}]:{DEFAULT_PORT}")
    } else {
        format!("{address}:{DEFAULT_PORT}")
    }
}

/// Where other machines reach a listener bound to `bound`. A wildcard bind
/// shows the address of the interface this machine routes through.
fn reachable(bound: SocketAddr) -> String {
    if !bound.ip().is_unspecified() {
        return bound.to_string();
    }
    // Connecting a UDP socket only picks a route; it sends nothing.
    let routed = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
            socket.local_addr()
        })
        .ok()
        .filter(|local| !local.ip().is_unspecified());
    match routed {
        Some(local) => SocketAddr::new(local.ip(), bound.port()).to_string(),
        None => format!("port {}", bound.port()),
    }
}

/// A host waiting for every human seat to fill.
pub(crate) struct HostLobby {
    listener: Listener,
    address: String,
    commit: String,
    scenario: Scenario,
    host: PlayerId,
    seats: Vec<PlayerId>,
    greeting: Vec<Connection>,
    joined: Vec<Connection>,
    starting: Option<Starting>,
    notice: Option<String>,
}

struct Starting {
    barrier: StartBarrier,
    game: Box<Game>,
    seated: Vec<(PlayerId, Connection)>,
}

impl HostLobby {
    /// Listens on `address` for the scenario's human seats other than
    /// `host`'s.
    pub(crate) fn new(
        address: &str,
        scenario: Scenario,
        host: PlayerId,
        commit: &str,
    ) -> Result<Self> {
        let humans: Vec<PlayerId> = scenario
            .players
            .iter()
            .zip(0..)
            .filter(|(player, _)| !player.bot)
            .map(|(_, seat)| PlayerId(seat))
            .collect();
        ensure!(
            humans.len() >= 2 && humans.contains(&host),
            "a hosted scenario needs the host's seat and another human seat (seats with \"bot\": false)"
        );
        let listener =
            Listener::bind(address).with_context(|| format!("cannot listen on {address}"))?;
        let bound = listener
            .local_addr()
            .with_context(|| format!("cannot listen on {address}"))?;
        Ok(Self {
            listener,
            address: reachable(bound),
            commit: commit.to_owned(),
            scenario,
            host,
            seats: humans.into_iter().filter(|seat| *seat != host).collect(),
            greeting: Vec::new(),
            joined: Vec::new(),
            starting: None,
            notice: None,
        })
    }

    fn status(&self) -> String {
        let place = if self.starting.is_some() {
            "Starting the match...".to_owned()
        } else {
            format!(
                "Hosting on {}: {} of {} players here",
                self.address,
                self.joined.len() + 1,
                self.seats.len() + 1
            )
        };
        match &self.notice {
            Some(notice) => format!("{place}. {notice}"),
            None => place,
        }
    }

    fn poll(&mut self, now: Duration, viewport: Vec2) -> Option<(Game, Link)> {
        if self.starting.is_some() {
            return self.poll_start(now);
        }
        while let Ok(Some(connection)) = self.listener.try_accept() {
            // The host speaks first, so a mismatched client can say why.
            connection.send(&LobbyMessage::hello(&self.commit).encode());
            self.greeting.push(connection);
        }
        for connection in std::mem::take(&mut self.greeting) {
            match connection.try_recv() {
                Ok(None) => self.greeting.push(connection),
                Ok(Some(line)) => {
                    let matching = matches!(
                        JoinMessage::decode(&line),
                        Ok(JoinMessage::Hello { protocol, commit })
                            if same_build(protocol, &commit, &self.commit)
                    );
                    if matching {
                        self.joined.push(connection);
                    }
                }
                Err(Closed) => {}
            }
        }
        self.admit(now, viewport)
    }

    fn admit(&mut self, now: Duration, viewport: Vec2) -> Option<(Game, Link)> {
        self.joined
            .retain(|connection| matches!(connection.try_recv(), Ok(None)));
        if self.joined.len() < self.seats.len() {
            return None;
        }
        let game = match Game::networked(self.scenario.clone(), self.host, NetRole::Host, viewport)
        {
            Ok(game) => game,
            Err(error) => {
                self.notice = Some(format!("Cannot start: {error}"));
                self.joined.clear();
                return None;
            }
        };
        let mut barrier = StartBarrier::new(
            &self.scenario,
            self.host,
            &self.seats,
            game.state.hash(),
            now,
        );
        let seated: Vec<(PlayerId, Connection)> = self
            .seats
            .iter()
            .copied()
            .zip(self.joined.drain(..))
            .collect();
        send_to(&seated, barrier.take_outgoing());
        self.starting = Some(Starting {
            barrier,
            game: Box::new(game),
            seated,
        });
        self.notice = None;
        None
    }

    fn poll_start(&mut self, now: Duration) -> Option<(Game, Link)> {
        let starting = self.starting.as_mut()?;
        let mut failed = false;
        for (seat, connection) in &starting.seated {
            loop {
                match connection.try_recv() {
                    Ok(Some(line)) => {
                        if starting.barrier.receive(*seat, &line).is_err() {
                            failed = true;
                        }
                    }
                    Ok(None) => break,
                    Err(Closed) => {
                        failed = true;
                        break;
                    }
                }
            }
        }
        let started = if failed {
            Err(())
        } else {
            starting.barrier.poll(now).map_err(|_| ())
        };
        match started {
            Ok(None) => None,
            Ok(Some(session)) => {
                let Starting {
                    mut barrier,
                    game,
                    seated,
                } = self.starting.take()?;
                // Go precedes everything the session sends.
                send_to(&seated, barrier.take_outgoing());
                Some((*game, Link::host(session, seated, now)))
            }
            Err(()) => {
                // Dropping the seated connections sends every client back to
                // rejoin, so nothing from this attempt reaches the next.
                self.starting = None;
                self.notice = Some("The start failed; waiting for players to rejoin".to_owned());
                None
            }
        }
    }
}

fn send_to(seated: &[(PlayerId, Connection)], lines: Vec<(PlayerId, String)>) {
    for (seat, line) in lines {
        if let Some((_, connection)) = seated.iter().find(|(peer, _)| *peer == seat) {
            connection.send(&line);
        }
    }
}

/// A client joining a host.
pub(crate) struct ClientLobby {
    address: String,
    commit: String,
    state: Joining,
}

enum Joining {
    Connecting(JoinHandle<io::Result<Connection>>),
    Greeting(Connection),
    Waiting(Connection),
    Ready(Connection, Box<Game>, PlayerId),
    Failed(String),
    Started,
}

impl Joining {
    fn connect(address: &str) -> Self {
        let address = address.to_owned();
        Joining::Connecting(thread::spawn(move || Connection::connect(address)))
    }
}

impl ClientLobby {
    /// Starts connecting to the host at `address`.
    pub(crate) fn new(address: &str, commit: &str) -> Self {
        Self {
            address: address.to_owned(),
            commit: commit.to_owned(),
            state: Joining::connect(address),
        }
    }

    fn status(&self) -> String {
        match &self.state {
            Joining::Connecting(_) => format!("Connecting to {}...", self.address),
            Joining::Greeting(_) | Joining::Waiting(_) => {
                "Waiting for the host to start the match".to_owned()
            }
            Joining::Ready(_, _, seat) => format!("Starting as seat {}...", seat.0 + 1),
            Joining::Failed(reason) => reason.clone(),
            Joining::Started => "Started".to_owned(),
        }
    }

    fn poll(&mut self, now: Duration, viewport: Vec2) -> Option<(Game, Link)> {
        let state = std::mem::replace(&mut self.state, Joining::Started);
        let (next, started) = self.advance(state, now, viewport);
        self.state = next;
        started
    }

    fn advance(
        &self,
        state: Joining,
        now: Duration,
        viewport: Vec2,
    ) -> (Joining, Option<(Game, Link)>) {
        let refused = |reason: &str| (Joining::Failed(reason.to_owned()), None);
        match state {
            Joining::Connecting(handle) if handle.is_finished() => match handle.join() {
                Ok(Ok(connection)) => {
                    connection.send(&JoinMessage::hello(&self.commit).encode());
                    (Joining::Greeting(connection), None)
                }
                Ok(Err(error)) => refused(&format!("Cannot reach {}: {error}", self.address)),
                Err(_) => refused("Connecting failed"),
            },
            Joining::Greeting(connection) => match connection.try_recv() {
                Ok(None) => (Joining::Greeting(connection), None),
                Ok(Some(line)) => match LobbyMessage::decode(&line) {
                    Ok(LobbyMessage::Hello { protocol, commit })
                        if same_build(protocol, &commit, &self.commit) =>
                    {
                        (Joining::Waiting(connection), None)
                    }
                    Ok(LobbyMessage::Hello { commit, .. }) => refused(&format!(
                        "The host runs build {commit}; this is build {}",
                        self.commit
                    )),
                    _ => refused("The host sent something unexpected"),
                },
                Err(Closed) => refused("The host closed the connection"),
            },
            Joining::Waiting(connection) => match connection.try_recv() {
                Ok(None) => (Joining::Waiting(connection), None),
                Ok(Some(line)) => match LobbyMessage::decode(&line) {
                    Ok(LobbyMessage::Start { seat, scenario }) => {
                        match Game::networked(*scenario, seat, NetRole::Client, viewport) {
                            Ok(game) => {
                                let ready = JoinMessage::Ready {
                                    hash: game.state.hash(),
                                };
                                connection.send(&ready.encode());
                                (Joining::Ready(connection, Box::new(game), seat), None)
                            }
                            Err(error) => refused(&format!("Cannot start: {error}")),
                        }
                    }
                    _ => refused("The host sent something unexpected"),
                },
                Err(Closed) => refused("The host closed the lobby"),
            },
            Joining::Ready(connection, game, seat) => match connection.try_recv() {
                Ok(None) => (Joining::Ready(connection, game, seat), None),
                // Stop reading at Go: the batches behind it belong to the link.
                Ok(Some(line)) => match LobbyMessage::decode(&line) {
                    Ok(LobbyMessage::Go) => {
                        let link = Link::client(ClientSession::new(now), connection, now);
                        (Joining::Started, Some((*game, link)))
                    }
                    _ => refused("The host sent something unexpected"),
                },
                Err(Closed) => refused("The start failed; try joining again"),
            },
            other => (other, None),
        }
    }
}

#[cfg(test)]
mod tests {
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
            player.bot_config = player.bot.then(|| {
                BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, seat as u64)
            });
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
                (self.client.0.state.current_tick() == self.host.0.state.current_tick())
                    .then_some(())
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
}
