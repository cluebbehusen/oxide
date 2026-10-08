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
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// After this long without a new tick, the waiting player is told why.
const WAIT_NOTICE: Duration = Duration::from_millis(500);

/// How long a host keeps finished connections open so every client hears
/// the last lines: a desync halt, or the batches that decided the match.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

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

    /// Leaves the match. A host of a decided match finishes its connections
    /// and hands them back to flush, so a client still behind receives the
    /// deciding batches instead of losing its host. Anything else drops at
    /// once: a client's seat surrenders, and an undecided host ends the match.
    pub(crate) fn leave(self, game: &Game, now: Duration) -> Option<Lingering> {
        match self {
            Link::Host(host) if game.state.result().is_some() => Some(Lingering {
                peers: host
                    .peers
                    .into_iter()
                    .map(|(_, mut connection)| {
                        connection.finish();
                        connection
                    })
                    .collect(),
                since: now,
            }),
            _ => None,
        }
    }
}

/// A departed host's finished connections, flushing their last lines.
pub(crate) struct Lingering {
    peers: Vec<Connection>,
    since: Duration,
}

impl Lingering {
    /// Drains the connections; false once every one closed or the grace ran
    /// out.
    pub(crate) fn pump(&mut self, now: Duration) -> bool {
        self.peers.retain(|connection| drain(connection).is_ok());
        !self.peers.is_empty() && now.saturating_sub(self.since) < CLOSE_GRACE
    }
}

impl HostLink {
    fn pump(&mut self, game: &mut Game, now: Duration) -> Option<End> {
        let dt = self.clock.step(now);
        if let Some((since, end)) = self.closing {
            self.peers
                .retain(|(_, connection)| drain(connection).is_ok());
            let done = self.peers.is_empty() || now.saturating_sub(since) >= CLOSE_GRACE;
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
                    game.presentation.toast(format!("{name} left the match"));
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
                            return Some(self.end(end));
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
            return Some(self.end(end));
        }
        for line in self.session.take_outgoing() {
            connection.send(&line);
        }
        if self.clock.stalled(game, now) {
            game.presentation.toast("Waiting for the host...");
        }
        None
    }

    fn end(&mut self, end: ClientEnd) -> End {
        self.connection = None;
        match end {
            ClientEnd::Desync { tick } => End::Desync { tick },
            ClientEnd::HostSilent | ClientEnd::Protocol => End::HostLost,
        }
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
/// shows the addresses of the interfaces this machine routes through to the
/// internet and to Tailscale's service address, which differ only when a
/// tailnet is up.
fn reachable(bound: SocketAddr) -> String {
    if !bound.ip().is_unspecified() {
        return bound.to_string();
    }
    let mut shown: Vec<String> = Vec::new();
    for probe in [
        Ipv4Addr::new(192, 0, 2, 1),
        Ipv4Addr::new(100, 100, 100, 100),
    ] {
        if let Some(ip) = routed_from(probe) {
            let address = SocketAddr::new(ip, bound.port()).to_string();
            if !shown.contains(&address) {
                shown.push(address);
            }
        }
    }
    if shown.is_empty() {
        format!("port {}", bound.port())
    } else {
        shown.join(" or ")
    }
}

/// The local address this machine would send to `probe` from. Connecting a
/// UDP socket only picks a route; it sends nothing.
fn routed_from(probe: Ipv4Addr) -> Option<IpAddr> {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect((probe, 9))?;
            socket.local_addr()
        })
        .ok()
        .map(|local| local.ip())
        .filter(|ip| !ip.is_unspecified())
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
mod tests;
