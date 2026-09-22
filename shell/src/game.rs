//! Live simulation, bot execution, command recording, and session bookkeeping.
//! Shared presentation borrows the active world and never advances it.

use anyhow::Result;
use chassis::replay::Replay;
use macroquad::prelude::{Vec2, vec2};
use oxide_protocol::hash_hex;
use oxide_sim::bot::{SeatBot, seat_bots};
use oxide_sim::{
    Building, BuildingId, Command, Event, PlayerCommand, PlayerId, SIM_VERSION, Scenario, State,
    TICKS_PER_SECOND, UnitId, UnitKind,
};
use std::ops::Deref;

pub(crate) fn finish_recording(writer: &oxide_kit::recovery::RecoveryWriter, tick: u64) {
    writer.finish(tick);
    // Only the existing explicit leave/save path waits. A failed or slow
    // writer leaves an interrupted record, even if the ordinary save landed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !writer.status().clean
        && writer.status().error.is_none()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Seconds per sim tick.
pub const TICK_DT: f32 = 1.0 / TICKS_PER_SECOND as f32;
/// Ticks a single frame may run before we let rendering catch up. Sized
/// so the advertised 64x speed cap is real at 60 fps (64 × 20 tps ÷ 60);
/// ticks are cheap enough that a full frame of them costs well under 1 ms.
const MAX_TICKS_PER_FRAME: u32 = 24;

pub use oxide_kit::GameReplay;

/// Immutable access to the authoritative simulation outside this module.
pub(crate) struct ReadOnlyState(State);

impl Deref for ReadOnlyState {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl std::ops::DerefMut for ReadOnlyState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Commands waiting for the next recorded tick.
#[derive(Debug, Default)]
pub(crate) struct PendingCommands(Vec<PlayerCommand>);

impl Deref for PendingCommands {
    type Target = Vec<PlayerCommand>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl std::ops::DerefMut for PendingCommands {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// What the player currently has selected.
#[derive(Default)]
pub struct Selection {
    /// Selected units — single-allegiance by construction (own for
    /// command, ally/enemy for read-only inspection).
    pub units: Vec<UnitId>,
    /// Selected buildings of one owner (mutually exclusive with units
    /// in practice), kept in id order. Commands validate ownership at
    /// their own gates.
    pub buildings: Vec<BuildingId>,
}

/// A transient visual effect (never sim-relevant).
mod fx;
mod presentation;
pub(crate) use presentation::{Presentation, Scene};
mod projectiles;
pub(crate) use projectiles::LaunchPose;

pub(crate) use fx::UnitBody;
pub use fx::{Effect, EffectKind, FlakYokeDelay, PingKind, ShotStyle, SoundKind};

/// A transient HUD message (rejected orders, stalled units).
pub struct Toast {
    /// What to say.
    pub text: String,
    /// Seconds since raised.
    pub age: f32,
}

/// One running session.
pub struct Game {
    /// The scenario this session started from.
    pub scenario: Scenario,
    /// The sim. Mutable access stays inside `Game` outside test fixtures.
    pub(crate) state: ReadOnlyState,
    /// Command sources for bot-flagged players.
    pub bots: Vec<SeatBot>,
    /// Every command of the session, tick-stamped — always recording.
    pub recorder: GameReplay,
    pub(crate) recovery_root: Option<std::path::PathBuf>,
    pub(crate) recovery: Option<std::sync::Arc<oxide_kit::recovery::RecoveryWriter>>,
    recovery_warned: bool,
    diagnostics_warned: bool,
    pub(crate) recovery_source: Option<std::path::PathBuf>,
    pub(crate) diagnostics: Option<oxide_kit::diagnostics::Recorder>,
    /// Commands staged for the next tick (human + debug socket).
    pub(crate) pending: PendingCommands,
    /// Whether the current session content is already autosaved; a new
    /// tick makes it stale again. Guards double-writes when Main Menu
    /// saves and the same game then quits as the Home backdrop.
    pub autosave_done: bool,
    /// End-of-match statistics, computed once from the recorder when
    /// the result lands from the bounded live tracker.
    pub end_stats: Option<oxide_kit::stats::MatchStats>,
    /// Tick-event totals and adaptively thinned graph samples. This is
    /// presentation bookkeeping and never feeds back into the sim.
    live_stats: oxide_kit::stats::LiveMatchStats,
    /// The match in numbers at the moment the human conceded an
    /// UNDECIDED team match — the exit offer's stats. Decided matches
    /// (a 1v1 surrender included) go through `end_stats` instead.
    pub concede_stats: Option<oxide_kit::stats::MatchStats>,
    /// What the player has demonstrably done — the tutorial's evidence.
    pub demo: crate::tutorial::Demo,
    /// True during bulk fast-forwards: presentation (fx, sounds, facing)
    /// is skipped entirely instead of accumulated-then-discarded — a
    /// million-tick advance must not buffer a million battles.
    suppress_presentation: bool,
    pub(crate) presentation: Presentation,
}

pub(crate) fn world_vec(pos: chassis::fx::Vec2Fx) -> Vec2 {
    vec2(pos.x.to_num::<f32>(), pos.y.to_num::<f32>())
}

impl Game {
    pub(crate) fn view(&self) -> Scene<'_> {
        Scene {
            state: &self.state,
            scenario: &self.scenario,
            pending: &self.pending,
            presentation: &self.presentation,
        }
    }
    pub fn selection_commandable(&self) -> bool {
        self.view().selection_commandable()
    }
    pub fn home_foundry(&self) -> Option<&Building> {
        self.view().home_foundry()
    }
    pub fn my_vision(&self) -> &oxide_sim::Vision {
        self.state.vision(self.presentation.human)
    }
    pub(crate) fn update_fx(&mut self, dt: f32) {
        self.presentation.update_fx(&self.state, dt);
    }
    pub(crate) fn update_wall_clock_fx(&mut self, dt: f32) {
        self.presentation.update_wall_clock_fx(&self.state, dt);
    }
    pub(crate) fn drop_presentation(&mut self) {
        self.presentation.drop_presentation(&self.state);
    }
    pub(crate) fn replace_state_after_jump(&mut self, state: &State) {
        self.state.0 = state.clone();
        self.presentation.reset_after_jump(&self.state);
    }

    /// Starts a session from a scenario, at the injected window size
    /// (headless callers get the default without a window).
    pub fn new(scenario: Scenario) -> Result<Self> {
        Self::with_viewport(scenario, crate::render::viewport())
    }

    /// `new` with the window injected — the only constructor tests use,
    /// because it never touches macroquad.
    pub fn with_viewport(scenario: Scenario, viewport: Vec2) -> Result<Self> {
        // The human is the scenario's single non-bot seat — seat choice
        // never permutes seats (parity carries factions, teams, and the
        // automation harness), it just moves which chair the human
        // takes. Anything but exactly one non-bot seat is a malformed
        // PLAYABLE session; the spectator constructor above is the
        // lenient door.
        let humans: Vec<PlayerId> = scenario
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.bot)
            .map(|(i, _)| PlayerId(i as u8))
            .collect();
        let human = match humans.as_slice() {
            [seat] => *seat,
            _ => anyhow::bail!(
                "a session wants exactly one non-bot seat, got {}",
                humans.len()
            ),
        };
        Self::assemble(scenario, viewport, human)
    }

    fn assemble(scenario: Scenario, viewport: Vec2, human: PlayerId) -> Result<Self> {
        let state = scenario.build()?;
        let live_stats = oxide_kit::stats::LiveMatchStats::new(&state);
        let bots = seat_bots(&scenario)?;
        let recorder = Replay::new(SIM_VERSION, scenario.clone());
        let presentation = Presentation::new(&state, human, viewport);
        Ok(Self {
            scenario,
            state: ReadOnlyState(state),
            bots,
            recorder,
            pending: PendingCommands(Vec::new()),
            autosave_done: false,
            recovery_root: None,
            recovery: None,
            recovery_warned: false,
            diagnostics_warned: false,
            recovery_source: None,
            diagnostics: None,
            end_stats: None,
            live_stats,
            concede_stats: None,
            demo: crate::tutorial::Demo::default(),
            suppress_presentation: false,
            presentation,
        })
    }

    /// Resumes a session from a recorded replay: rebuild its scenario,
    /// re-execute every recorded tick (headless-fast), and keep recording
    /// onto the same log. In a deterministic sim a replay *is* a save file
    /// — this is "load game".
    pub fn from_replay(replay: GameReplay) -> Result<Self> {
        Self::from_replay_observed(replay, None)
    }

    pub(crate) fn from_replay_observed(
        replay: GameReplay,
        diagnostics: Option<&oxide_kit::diagnostics::Recorder>,
    ) -> Result<Self> {
        let _load = diagnostics
            .and_then(|recorder| recorder.span(oxide_kit::diagnostics::Phase::ReplayLoad, 0));
        // Untrusted file: enforce the invariants recording guarantees, and
        // refuse cross-version saves outright — resuming one would keep
        // recording onto a log that can no longer reproduce.
        replay
            .validate(Some(SIM_VERSION))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let scenario = replay.setup.clone();
        let mut state = scenario.build()?;
        let total = replay.meta.ticks.unwrap_or_else(|| {
            replay
                .commands
                .last()
                .map_or(0, |c| c.tick.saturating_add(1))
        });
        // Loading replays synchronously on the frame loop: a structurally
        // valid file can still claim an absurd duration and freeze the UI
        // for minutes. ~28 game-hours is beyond any honest session.
        const MAX_LOAD_TICKS: u64 = oxide_kit::MAX_REPLAY_TICKS;
        anyhow::ensure!(
            total <= MAX_LOAD_TICKS,
            "replay spans {total} ticks, beyond the {MAX_LOAD_TICKS}-tick interactive load limit \
             (the headless driver replays without one)"
        );
        // The fast-forward lets bots observe every tick to rebuild controller
        // memory. Their generated commands are discarded because the record
        // remains authoritative.
        let mut bots = seat_bots(&scenario)?;
        let mut cursor = replay.cursor();
        let mut live_stats = oxide_kit::stats::LiveMatchStats::new(&state);
        let mut projectile_releases = projectiles::ProjectileReleases::default();
        let mut game = Self::new(scenario)?;
        let mut boundary_fog = game.presentation.boundary_fog.clone();
        for _ in 0..total {
            if let Some(recorder) = diagnostics {
                recorder.replay_progress(state.current_tick());
            }
            let _ = oxide_kit::bot_execution::commands_observed(&state, &mut bots, diagnostics);
            let commands: Vec<PlayerCommand> = cursor
                .take_tick(state.current_tick())
                .iter()
                .map(|t| t.command.clone())
                .collect();
            let report = state.tick(&commands);
            boundary_fog.observe(&state, game.presentation.human);
            projectile_releases.observe(&state, &report.events);
            live_stats.observe(&state, &report.events);
        }
        anyhow::ensure!(
            cursor.is_finished(),
            "replay duration metadata does not cover its own commands"
        );
        game.replace_state_after_jump(&state);
        game.presentation.boundary_fog = boundary_fog;
        game.presentation.projectile_releases = projectile_releases;
        game.bots = bots;
        game.recorder = replay;
        game.live_stats = live_stats;
        if game.state.result().is_some() {
            game.end_stats = Some(game.live_stats.snapshot(&game.state));
        }
        if let Some(focus) = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == game.presentation.human)
            .map(|b| world_vec(b.center()))
        {
            game.presentation.camera.center = focus;
            game.presentation.camera.pan(Vec2::ZERO); // re-clamp
        }
        Ok(game)
    }

    pub(crate) fn configure_diagnostics(&mut self, enabled: bool) {
        if !enabled {
            self.diagnostics_warned = false;
        }
        if enabled && self.diagnostics.is_none() && !self.diagnostics_warned {
            self.start_recovery();
            if let Some(recording) = &self.recovery {
                match oxide_kit::diagnostics::Recorder::start(recording.clone()) {
                    Ok(recorder) => {
                        recorder.install_panic_hook();
                        self.diagnostics = Some(recorder);
                    }
                    Err(error) => {
                        self.diagnostics_warned = true;
                        self.presentation
                            .toast(format!("Diagnostics unavailable: {error}"));
                    }
                }
            }
        }
        if let Some(recorder) = &self.diagnostics {
            recorder.set_enabled(enabled);
        }
    }
    pub(crate) fn diagnostic_span(
        &self,
        phase: oxide_kit::diagnostics::Phase,
    ) -> Option<oxide_kit::diagnostics::Span> {
        self.diagnostics
            .as_ref()
            .and_then(|recorder| recorder.span(phase, self.state.current_tick()))
    }

    pub(crate) fn start_recovery(&mut self) {
        if self.recovery.is_none()
            && !self.recovery_warned
            && let Some(root) = &self.recovery_root
        {
            match oxide_kit::recovery::RecoveryWriter::start_recovered(
                root.clone(),
                self.recorder.clone(),
                self.state.current_tick(),
                self.recovery_source.take(),
            ) {
                Ok(writer) => self.recovery = Some(std::sync::Arc::new(writer)),
                Err(error) => {
                    self.recovery_warned = true;
                    self.presentation
                        .toast(format!("Recovery unavailable: {error}"));
                }
            }
        }
    }

    pub(crate) fn poll_recovery(&mut self) {
        if !self.recovery_warned
            && let Some(writer) = &self.recovery
            && let Some(error) = writer.status().error
        {
            self.recovery_warned = true;
            self.presentation
                .toast(format!("Recovery stopped: {error}"));
        }
    }

    pub(crate) fn finish_recovery(&self) {
        if let Some(writer) = &self.recovery {
            finish_recording(writer, self.state.current_tick());
        }
    }

    /// Runs exactly one tick: bots think, staged commands drain, everything
    /// is recorded, presentation caches update. The only place `state.current_tick()`
    /// is called.
    pub fn do_tick(&mut self) -> oxide_sim::TickReport {
        self.start_recovery();
        // New ticks make any earlier autosave stale.
        self.autosave_done = false;
        // Interpolation cache; pointless during suppressed bulk advances
        // (advance_ticks rebuilds it once at the end).
        if !self.suppress_presentation {
            self.presentation.remember_previous_tick(&self.state);
        }

        let mut commands = std::mem::take(&mut self.pending.0);
        let human_commands: Vec<Command> = commands
            .iter()
            .filter(|pc| pc.player == self.presentation.human)
            .map(|pc| pc.command.clone())
            .collect();
        let bot_scope = self.diagnostic_span(oxide_kit::diagnostics::Phase::Bots);
        commands.extend(oxide_kit::bot_execution::commands_observed(
            &self.state,
            &mut self.bots,
            self.diagnostics
                .as_ref()
                .filter(|recorder| recorder.enabled()),
        ));
        drop(bot_scope);
        for command in &commands {
            self.recorder
                .record(self.state.current_tick(), command.clone());
        }
        if let Some(recovery) = &self.recovery {
            recovery.prepared(self.state.current_tick(), &commands);
        }
        let sim_scope = self.diagnostic_span(oxide_kit::diagnostics::Phase::Simulation);
        let report = self.state.0.tick(&commands);
        drop(sim_scope);
        if let Some(recovery) = &self.recovery {
            recovery.completed(self.state.current_tick());
        }
        let _presentation_scope = self.diagnostic_span(oxide_kit::diagnostics::Phase::Presentation);
        self.presentation
            .boundary_fog
            .observe(&self.state, self.presentation.human);
        self.live_stats.observe(&self.state, &report.events);
        if self.state.result().is_some() && self.end_stats.is_none() {
            self.end_stats = Some(self.live_stats.snapshot(&self.state));
        }

        // Income is evidence too: the mining lesson graduates on a
        // load actually landing, not on the accepted order — so it
        // rides the sim's event, outside the command gate below.
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::ScrapDeposited { player, .. } if *player == self.presentation.human))
        {
            self.demo.deposited = true;
        }

        // A concession that did NOT decide the match (a team game, the
        // ally fighting on) raises the surrender overlay: the human's
        // match in numbers so far, with Esc-to-menu as the exit. A
        // decisive surrender goes through the normal result flow, and a
        // bulk fast-forward (replay load) keeps only the resigned fact —
        // the banner is a fresh-concession moment, not standing state.
        if !self.suppress_presentation
            && self.state.result().is_none()
            && report
                .events
                .iter()
                .any(|e| matches!(e, Event::PlayerResigned { player } if *player == self.presentation.human))
        {
            self.concede_stats = Some(self.live_stats.snapshot(&self.state));
            self.presentation.conceded_banner = true;
        }

        // The tutorial's evidence: what the human actually asked for
        // AND the sim accepted. A tick carrying any rejection for the
        // human grades nothing — the deliberately-illegal placement
        // the building lesson invites must not graduate it.
        let human_rejected = report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { player, .. } if *player == self.presentation.human));
        if !human_rejected {
            for command in &human_commands {
                match command {
                    Command::Train { kind, .. } => {
                        self.demo.trained = true;
                        if kind.stats().can_fight() {
                            self.demo.trained_fighter = true;
                        }
                    }
                    Command::Harvest { .. } => self.demo.harvested = true,
                    Command::Build { .. } => self.demo.built = true,
                    // The march lesson teaches the default zero-chase advance;
                    // explicit attack-move is a different stance.
                    Command::Advance { .. } => self.demo.advanced = true,
                    _ => {}
                }
            }
        }

        if !self.suppress_presentation {
            self.presentation
                .observe_tick(&self.state, &report.events, &report.movement);
        } else {
            self.presentation
                .projectile_releases
                .observe(&self.state, &report.events);
        }
        // Dead units leave the selection — and so do HOSTILES whose
        // ground fog has re-covered: the panel reads live hp from the
        // selection, and an inspection must never become a tracking
        // beacon into the dark. (Allies stay: team sight is standing.)
        let human = self.presentation.human;
        let all_seeing = self.presentation.all_seeing();
        {
            let state = &self.state;
            self.presentation.selection.units.retain(|id| {
                state.unit(*id).is_some_and(|u| {
                    !state.hostile(human, u.player) || all_seeing || {
                        state.vision(human).visible(u.tile())
                    }
                })
            });
        }
        self.presentation.selection.buildings.retain(|id| {
            self.state.building(*id).is_some_and(|building| {
                !self.state.hostile(human, building.player)
                    || all_seeing
                    || (building
                        .tiles()
                        .any(|tile| self.state.vision(human).visible(tile))
                        && self.state.building_apparent(human, building))
            })
        });
        report
    }

    /// Advances the ordinary live path while optionally stopping on one exact
    /// tick. Native profiling supplies the bound so a multi-tick frame cannot
    /// overshoot its requested sample window; ordinary play passes `None`.
    pub fn advance_wall_clock(&mut self, dt: f32, stop_tick: Option<u64>) -> bool {
        if self.presentation.paused {
            return false;
        }
        self.presentation.accum += dt * self.presentation.speed as f32;
        let mut ran = 0;
        while self.presentation.accum >= TICK_DT
            && ran < MAX_TICKS_PER_FRAME
            && stop_tick.is_none_or(|tick| self.state.current_tick() < tick)
        {
            self.presentation.accum -= TICK_DT;
            self.do_tick();
            ran += 1;
        }
        if stop_tick.is_some_and(|tick| self.state.current_tick() >= tick) {
            self.presentation.accum = 0.0;
            return true;
        }
        // Behind by more than a frame's worth of ticks? Drop the debt
        // rather than spiraling.
        if ran == MAX_TICKS_PER_FRAME {
            self.presentation.accum = self.presentation.accum.min(TICK_DT);
        }
        false
    }

    /// Fast-forwards `n` ticks immediately, pause state notwithstanding
    /// (the debug socket's driven-clock mode).
    pub fn advance_ticks(&mut self, n: u64) {
        self.suppress_presentation = true;
        for _ in 0..n {
            self.do_tick();
        }
        self.suppress_presentation = false;
        // No cross-jump interpolation after a bulk advance — and whatever
        // presentation slipped in beforehand doesn't survive the jump.
        self.presentation.accum = 0.0;
        self.drop_presentation();
        self.presentation.remember_previous_tick(&self.state);
        self.presentation.facing.clear();
        self.presentation.refresh_facing(&self.state, &[]);
    }

    /// Advances a small number of ticks while retaining presentation
    /// effects and returning the sim events. This is the agent-facing
    /// counterpart to [`Game::advance_ticks`]: slower, but suitable for
    /// judging command acceptance, transient feedback, and combat frames.
    pub fn present_ticks(&mut self, n: u64) -> Vec<Event> {
        let mut events = Vec::new();
        for _ in 0..n {
            // A presented step stands in for one normal sim interval.
            // Age what the previous tick put on screen before spawning
            // this tick's effects, leaving the newest effects at age zero.
            self.update_fx(TICK_DT);
            events.extend(self.do_tick().events);
        }
        self.presentation.accum = 0.0;
        events
    }

    /// Stages a command from the local player for the next tick.
    pub fn issue(&mut self, command: Command) {
        self.stage(PlayerCommand {
            player: self.presentation.human,
            command,
        });
    }

    /// Stages an already attributed command from the debug input surface.
    pub(crate) fn stage(&mut self, command: PlayerCommand) {
        self.pending.0.push(command);
    }

    /// Current state fingerprint, protocol-formatted.
    pub fn hash_hex(&self) -> String {
        hash_hex(self.state.hash())
    }

    /// The transport's view of this session — also the live half of the
    /// debug protocol's shared surface.
    pub fn status_view(&self) -> oxide_protocol::StatusView {
        oxide_protocol::StatusView {
            tick: self.state.current_tick(),
            paused: self.presentation.paused,
            speed: self.presentation.speed,
            scenario: self.scenario.name.clone(),
            sim_version: SIM_VERSION.to_string(),
            result: self.state.result(),
            recorded_commands: self.recorder.commands.len(),
        }
    }
}

pub(crate) fn rotor_hull_turn_rate(kind: UnitKind) -> Option<f32> {
    match kind {
        UnitKind::Skyhook => Some(0.25),
        UnitKind::Buzzard | UnitKind::Wisp => Some(
            0.3 * kind.stats().speed.to_num::<f32>()
                / UnitKind::Buzzard.stats().speed.to_num::<f32>(),
        ),
        _ => None,
    }
}

fn angle_delta(from: f32, to: f32) -> f32 {
    (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// The live session's half of the debug protocol's shared surface: real
/// clock, real recorder, sim time driven through the same `do_tick`
/// funnel every other command source uses.
impl oxide_protocol::DebugSession for Game {
    fn status(&self) -> oxide_protocol::StatusView {
        self.status_view()
    }

    fn state(&self) -> &State {
        &self.state
    }

    fn advance(&mut self, ticks: u64) -> oxide_protocol::AdvancedView {
        self.advance_ticks(ticks);
        oxide_protocol::AdvancedView {
            ticks,
            tick: self.state.current_tick(),
            hash: self.hash_hex(),
        }
    }

    fn present(&mut self, ticks: u64) -> oxide_protocol::PresentedView {
        let events = self.present_ticks(ticks);
        oxide_protocol::PresentedView {
            ticks,
            tick: self.state.current_tick(),
            hash: self.hash_hex(),
            events,
        }
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        self.presentation.paused = paused;
        Ok(())
    }

    fn set_speed(&mut self, multiplier: f64) -> Result<(), String> {
        oxide_protocol::check_speed(multiplier)?;
        self.presentation.speed = multiplier;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_records_the_shell_boundary_and_resumes_the_same_future() {
        use std::time::{Duration, Instant};
        let root =
            std::env::temp_dir().join(format!("oxide-shell-recovery-{}", std::process::id()));
        let mut original = Game::new(Scenario::skirmish()).unwrap();
        original.recovery_root = Some(root.clone());
        original.configure_diagnostics(true);
        original.advance_ticks(180);
        let writer = original.recovery.as_ref().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while writer.status().durable_tick < 180 {
            assert!(Instant::now() < deadline, "{:?}", writer.status());
            std::thread::sleep(Duration::from_millis(10));
        }
        let replay = oxide_kit::recovery::inspect(writer.directory())
            .unwrap()
            .replay;
        let mut resumed =
            Game::from_replay_observed(replay, original.diagnostics.as_ref()).unwrap();
        assert_eq!(original.hash_hex(), resumed.hash_hex());
        original.advance_ticks(120);
        resumed.advance_ticks(120);
        assert_eq!(original.hash_hex(), resumed.hash_hex());
        original.finish_recovery();
        assert!(original.recovery.as_ref().unwrap().status().clean);
        let directory = original.recovery.as_ref().unwrap().directory().to_owned();
        drop(original);
        loop {
            let lease = std::fs::File::open(directory.join("lease")).unwrap();
            if lease.try_lock().is_ok() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The resume guarantee the hash check alone cannot see: the
    /// watch-back loop replays every re-executed tick through the
    /// seat bots so they rebuild their cross-tick memory (RNG streams,
    /// raid memory, blacklists). Deleting that loop keeps the resume
    /// hash identical — the recorded commands carry it — and only the
    /// FUTURE diverges, which is exactly what this pins.
    #[test]
    fn a_resumed_session_plays_the_same_future_as_an_unsaved_one() {
        // Seat 0 stays human (a session wants exactly one), seat 1
        // is the shipped bot whose memory the watch-back rebuilds.
        let mut scenario = oxide_sim::Scenario::skirmish();
        oxide_kit::bench::all_bots(&mut scenario);
        scenario.players[0].bot = false;
        scenario.players[0].bot_config = None;
        let mut original = Game::new(scenario).expect("game builds");
        original.advance_ticks(600);

        let mut snapshot = original.recorder.clone();
        snapshot.meta.ticks = Some(600);
        let mut resumed = Game::from_replay(snapshot).expect("the snapshot resumes");
        assert_eq!(
            original.hash_hex(),
            resumed.hash_hex(),
            "premise: the resume point itself matches"
        );

        original.advance_ticks(1_000);
        resumed.advance_ticks(1_000);
        assert_eq!(
            original.hash_hex(),
            resumed.hash_hex(),
            "a resumed session's future diverged from the unsaved one — \
             bot memory was not rebuilt by the watch-back"
        );
    }
    #[test]
    fn multiple_bot_seats_rebuild_the_same_history_on_resume() {
        let mut scenario =
            oxide_sim::Scenario::from_json(include_str!("../../scenarios/compass-grand.json"))
                .unwrap();
        oxide_kit::bench::all_bots(&mut scenario);
        scenario.players[0].bot = false;
        scenario.players[0].bot_config = None;
        let mut original = Game::new(scenario).unwrap();
        original.advance_ticks(180);
        let mut snapshot = original.recorder.clone();
        snapshot.meta.ticks = Some(180);
        let mut resumed = Game::from_replay(snapshot).unwrap();
        original.advance_ticks(120);
        resumed.advance_ticks(120);
        assert_eq!(
            serde_json::to_vec(&original.recorder.commands).unwrap(),
            serde_json::to_vec(&resumed.recorder.commands).unwrap()
        );
        assert_eq!(original.hash_hex(), resumed.hash_hex());
    }

    use oxide_sim::{Command, Scenario, Target, UnitKind};

    #[test]
    fn playable_sessions_require_exactly_one_human_but_spectators_do_not() {
        let viewport = macroquad::prelude::vec2(1280.0, 800.0);
        let mut all_bots = Scenario::skirmish();
        for player in &mut all_bots.players {
            player.bot = true;
            player.bot_config = Some(oxide_sim::scenario::BotConfig::default());
        }
        let err = Game::with_viewport(all_bots.clone(), viewport)
            .err()
            .expect("an all-bot match has no command seat");
        assert!(err.to_string().contains("got 0"));
        assert_eq!(
            crate::screens::playback::PlaybackSession::from_replay(Replay::new(
                SIM_VERSION,
                all_bots
            ))
            .expect("spectator")
            .presentation
            .human,
            PlayerId(0)
        );

        let mut two_humans = Scenario::skirmish();
        for player in &mut two_humans.players {
            player.bot = false;
            player.bot_config = None;
        }
        let err = Game::with_viewport(two_humans, viewport)
            .err()
            .expect("two command seats are ambiguous");
        assert!(err.to_string().contains("got 2"));
    }

    #[test]
    fn replay_without_duration_infers_its_tail_and_restores_finished_stats() {
        let scenario = Scenario::skirmish();
        let mut replay = GameReplay::new(SIM_VERSION, scenario);
        replay.record(
            0,
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Surrender,
            },
        );
        assert!(replay.meta.ticks.is_none());

        let game = Game::from_replay(replay).expect("legacy duration is inferred");
        assert_eq!(game.state.current_tick(), 1);
        assert!(game.state.result().is_some());
        assert_eq!(
            game.end_stats.as_ref().map(|stats| stats.final_tick),
            Some(1)
        );
    }

    #[test]
    fn wall_clock_pause_and_hitch_rules_bound_simulation_debt() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("game");
        game.presentation.paused = true;
        assert!(!game.advance_wall_clock(10.0, None));
        assert_eq!(game.state.current_tick(), 0);
        assert_eq!(game.presentation.render_alpha(), 1.0);

        game.presentation.paused = false;
        game.presentation.speed = 64.0;
        assert!(!game.advance_wall_clock(1.0, None));
        assert_eq!(game.state.current_tick(), u64::from(MAX_TICKS_PER_FRAME));
        assert!(
            game.presentation.tick_fraction() <= 1.0,
            "excess hitch debt is dropped"
        );
    }

    #[test]
    fn toast_history_keeps_only_the_three_newest_messages() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("game");
        for text in ["one", "two", "three", "four"] {
            game.presentation.toast(text);
        }
        assert_eq!(
            game.presentation
                .toasts
                .iter()
                .map(|toast| toast.text.as_str())
                .collect::<Vec<_>>(),
            ["two", "three", "four"]
        );
        game.presentation.toast("three");
        assert_eq!(
            game.presentation
                .toasts
                .iter()
                .map(|toast| toast.text.as_str())
                .collect::<Vec<_>>(),
            ["two", "four", "three"]
        );
        assert_eq!(game.presentation.toasts.last().unwrap().age, 0.0);
    }

    #[test]
    fn articulated_hull_interpolates_the_short_turn_and_drops_on_seek() {
        let mut game = Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
        let id = game.state.units()[0].id;
        game.presentation.hull_heading.insert(id.0, (6.2, 0.1));
        assert!(
            (game.view().draw_hull_heading(id, 0.5) - (6.2 + angle_delta(6.2, 0.1) * 0.5)).abs()
                < 1e-6
        );
        assert!(angle_delta(6.2, 0.1) > 0.0);
        game.drop_presentation();
        let heading = f32::from(game.state.unit(id).unwrap().heading) * std::f32::consts::TAU
            / 256.0
            + std::f32::consts::FRAC_PI_2;
        assert_eq!(game.view().draw_hull_heading(id, 0.5), heading);
    }

    #[test]
    fn articulated_hulls_keep_authoritative_bearings_when_paused_or_jumped() {
        for kind in [UnitKind::Sentinel, UnitKind::Warden, UnitKind::Lancer] {
            let mut scenario = Scenario::skirmish();
            scenario.units[0].kind = kind;
            let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
            let id = game.state.units()[0].id;
            let assert_bearing = |game: &Game| {
                let expected =
                    f32::from(game.state.unit(id).unwrap().heading) * std::f32::consts::TAU / 256.0
                        + std::f32::consts::FRAC_PI_2;
                for alpha in [0.0, 0.5, 1.0] {
                    assert_eq!(
                        game.view().draw_hull_heading(id, alpha),
                        expected,
                        "{kind:?}"
                    );
                }
            };
            assert_bearing(&game);
            game.advance_ticks(1);
            assert_bearing(&game);
            let mut snapshot = serde_json::to_value(&*game.state).unwrap();
            snapshot["units"][0]["heading"] = serde_json::json!(96);
            game.replace_state_after_jump(&serde_json::from_value(snapshot).unwrap());
            assert_bearing(&game);
            game.drop_presentation();
            assert_bearing(&game);
        }
    }

    #[test]
    fn cruising_aircraft_keep_authoritative_facing_across_timeline_jumps() {
        for kind in UnitKind::ALL
            .into_iter()
            .filter(|kind| kind.cruise_turn_rate() > 0)
        {
            let mut scenario = Scenario::skirmish();
            scenario.units[0].kind = kind;
            let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
            let id = game.state.units()[0].id;
            for heading in [0u8, 96, 254] {
                let mut snapshot = serde_json::to_value(&*game.state).unwrap();
                snapshot["units"][0]["heading"] = serde_json::json!(heading);
                game.replace_state_after_jump(&serde_json::from_value(snapshot).unwrap());
                let expected = f32::from(heading) * std::f32::consts::TAU / 256.0
                    + std::f32::consts::FRAC_PI_2;
                assert_eq!(
                    game.presentation.facing.get(&id.0),
                    Some(&expected),
                    "{kind:?}"
                );
                for alpha in [0.0, 0.5, 1.0] {
                    assert_eq!(game.presentation.draw_heading(id, heading, alpha), expected);
                }
            }
        }
    }

    fn head_on_pair() -> (Game, [UnitId; 2]) {
        let mut map = vec!["........................................"; 24];
        map[2] = "..1.....................................";
        let scenario = serde_json::from_value(serde_json::json!({
            "name": "Head-on pass", "seed": 1, "map": map,
            "players": [{"name": "You", "faction": "ferrous", "scrap": 0, "bot": false}],
            "units": [
                {"player": 0, "kind": "sentinel", "x": 12, "y": 12},
                {"player": 0, "kind": "sentinel", "x": 22, "y": 12}
            ],
            "buildings": []
        }))
        .unwrap();
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        let ids = [game.state.units()[0].id, game.state.units()[1].id];
        game.present_ticks(1);
        for (id, x) in [(ids[0], 22), (ids[1], 12)] {
            game.issue(Command::Move {
                units: vec![id],
                goal: chassis::grid::TilePos::new(x, 12),
                queue: false,
            });
        }
        (game, ids)
    }

    #[test]
    fn collision_slides_are_drawn_eased_and_leaned_then_settle_on_the_simulation_pose() {
        let (mut game, ids) = head_on_pair();
        let (mut widest_lag, mut widest_lean) = (0.0_f32, 0.0_f32);
        for _ in 0..200 {
            game.present_ticks(1);
            for id in ids {
                let unit = game.state.unit(id).unwrap();
                let lag = world_vec(unit.pos) - game.presentation.draw_pos(id, unit.pos, 1.0);
                widest_lag = widest_lag.max(lag.length());
                widest_lean = widest_lean.max(game.presentation.slide_yaw(id, 1.0, false).abs());
            }
        }
        assert!(widest_lag > 0.02 && widest_lag <= crate::slide_motion::MAX_LAG + 1e-6);
        assert!(widest_lean > 0.05 && widest_lean <= crate::slide_motion::MAX_YAW + 1e-6);
        assert!(game.presentation.slide_motion.is_empty());
        for id in ids {
            let unit = game.state.unit(id).unwrap();
            assert_eq!(
                game.presentation.draw_pos(id, unit.pos, 1.0),
                world_vec(unit.pos)
            );
            assert_eq!(unit.order, oxide_sim::Order::Idle);
        }
    }

    #[test]
    fn a_bulk_advance_drops_eased_slides() {
        let (mut game, _) = head_on_pair();
        while game.presentation.slide_motion.is_empty() {
            game.present_ticks(1);
            assert!(game.state.current_tick() < 200, "the pair never touched");
        }
        game.advance_ticks(1);
        assert!(game.presentation.slide_motion.is_empty());
    }

    fn rotor_game(kind: UnitKind) -> Game {
        let mut map = vec!["........................................"; 24];
        map[2] = "..1................................2....";
        let scenario = serde_json::from_value(serde_json::json!({
            "name": "Rotor turning", "seed": 1, "map": map,
            "players": [
                {"name": "You", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "Target", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "units": [{"player": 0, "kind": kind, "x": 12, "y": 12}],
            "buildings": []
        }))
        .unwrap();
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap()
    }

    #[test]
    fn rotor_hulls_ease_reversals_and_finish_turning_after_stopping() {
        let mut first_turns = Vec::new();
        for kind in [UnitKind::Buzzard, UnitKind::Skyhook, UnitKind::Wisp] {
            let mut game = rotor_game(kind);
            let id = game.state.units()[0].id;
            game.present_ticks(1);
            game.issue(Command::Move {
                units: vec![id],
                goal: chassis::grid::TilePos::new(28, 12),
                queue: false,
            });
            game.present_ticks(20);
            let before = game.view().draw_hull_heading(id, 1.0);
            let position = game.state.unit(id).unwrap().pos;
            game.issue(Command::Move {
                units: vec![id],
                goal: chassis::grid::TilePos::new(8, 12),
                queue: false,
            });
            game.present_ticks(1);
            assert!(game.state.unit(id).unwrap().pos.x < position.x, "{kind:?}");
            let turn = angle_delta(before, game.view().draw_hull_heading(id, 1.0)).abs();
            assert!(turn > 0.1 && turn < 0.7, "{kind:?}: {turn}");
            first_turns.push(turn);
            assert!((game.view().draw_hull_heading(id, 0.0) - before).abs() < 1e-6);
            game.issue(Command::Stop { units: vec![id] });
            game.present_ticks(1);
            let stopped = game.state.unit(id).unwrap().pos;
            game.present_ticks(20);
            assert_eq!(game.state.unit(id).unwrap().pos, stopped, "{kind:?}");
            assert!(
                angle_delta(
                    game.view().draw_hull_heading(id, 1.0),
                    -std::f32::consts::FRAC_PI_2
                )
                .abs()
                    < 1e-5
            );
        }
        assert!((first_turns[0] - 0.3).abs() < 1e-6);
        assert!((first_turns[1] - 0.25).abs() < 1e-6);
        assert!(first_turns[1] < first_turns[0] && first_turns[0] < first_turns[2]);
    }

    #[test]
    fn hovering_wisp_eases_firing_aim_and_discards_it_on_seek() {
        let mut game = rotor_game(UnitKind::Wisp);
        let id = game.state.units()[0].id;
        game.present_ticks(1);
        let before = game.view().draw_hull_heading(id, 1.0);
        let position = game.state.unit(id).unwrap().pos;
        game.presentation
            .aim_units
            .insert(id.0, (std::f32::consts::PI, game.presentation.fx_time()));
        game.present_ticks(1);
        let after = game.view().draw_hull_heading(id, 1.0);
        assert!((0.1..0.7).contains(&angle_delta(before, after).abs()));
        assert_eq!(game.state.unit(id).unwrap().pos, position);
        game.present_ticks(8);
        assert!(
            angle_delta(game.view().draw_hull_heading(id, 1.0), std::f32::consts::PI).abs() < 1e-5
        );
        game.replace_state_after_jump(&game.state.0.clone());
        assert_eq!(
            game.view().draw_hull_heading(id, 0.0),
            game.view().draw_hull_heading(id, 1.0)
        );
        assert_eq!(game.view().draw_hull_heading(id, 1.0), 0.0);
    }

    #[test]
    fn draw_positions_interpolate_known_units_and_fall_back_for_new_ones() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("game");
        let unit_id = game.state.units()[0].id;
        let current = game.state.units()[0].pos;
        let now = world_vec(current);
        game.presentation
            .prev_pos
            .insert(unit_id.0, now - macroquad::prelude::vec2(2.0, 4.0));
        assert_eq!(
            game.presentation.draw_pos(unit_id, current, 0.5),
            now - macroquad::prelude::vec2(1.0, 2.0)
        );
        assert_eq!(
            game.presentation.draw_pos(UnitId(u32::MAX), current, 0.5),
            now
        );
    }

    #[test]
    fn externally_driven_tick_fractions_are_clamped_to_one_frame() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("game");
        game.presentation.sync_external_tick_fraction(2.0);
        assert_eq!(game.presentation.tick_fraction(), 1.0);
        game.presentation.sync_external_tick_fraction(-1.0);
        assert_eq!(game.presentation.tick_fraction(), 0.0);
    }

    #[test]
    fn admitted_attack_alert_queues_one_protected_audio_cue() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        game.presentation.sounds_pending.clear();

        game.presentation
            .raise_alert(macroquad::prelude::vec2(10.0, 10.0));
        game.presentation
            .raise_alert(macroquad::prelude::vec2(11.0, 11.0));

        assert_eq!(
            game.presentation.sounds_pending,
            vec![(SoundKind::Alert, None)],
            "the region gate must admit one alert cue rather than one per hit"
        );
    }

    #[test]
    fn demo_flags_read_only_the_humans_commands() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        assert!(!game.demo.trained);
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == game.presentation.human)
            .unwrap()
            .id;
        game.issue(Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        });
        game.do_tick();
        assert!(game.demo.trained, "the human trained");
        assert!(!game.demo.trained_fighter, "a harvester is not a fighter");
        game.issue(Command::Train {
            building: foundry,
            kind: UnitKind::Sentinel,
        });
        game.do_tick();
        assert!(game.demo.trained_fighter);
        // Run a few more ticks with no human commands and check the
        // unrelated flags stay cold — only the human's own commands
        // may grade the tutorial.
        for _ in 0..20 {
            game.do_tick();
        }
        assert!(!game.demo.advanced);
        assert!(!game.demo.built);
    }

    #[test]
    fn presented_ticks_return_rejections_and_keep_their_toast() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        let foreign = game
            .state
            .units()
            .iter()
            .find(|unit| unit.player != game.presentation.human)
            .expect("skirmish has an opponent")
            .id;
        game.issue(Command::Stop {
            units: vec![foreign],
        });

        let events = game.present_ticks(1);

        assert!(events.iter().any(|event| matches!(
            event,
            Event::CommandRejected { player, .. } if *player == game.presentation.human
        )));
        assert!(
            game.presentation
                .toasts
                .iter()
                .any(|toast| toast.text == "nothing selected can do that"),
            "presentation-preserving steps must retain shell feedback"
        );
    }

    #[test]
    fn presented_ticks_age_old_effects_and_leave_new_feedback_fresh() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        game.presentation.fx.push(Effect {
            kind: EffectKind::Ping {
                at: macroquad::prelude::Vec2::ZERO,
                kind: PingKind::Move,
            },
            age: 0.0,
        });

        game.present_ticks(4);

        let age = game
            .presentation
            .fx
            .iter()
            .find_map(|effect| matches!(effect.kind, EffectKind::Ping { .. }).then_some(effect.age))
            .expect("the half-second ping remains after four ticks");
        assert!(
            (age - 4.0 * TICK_DT).abs() < f32::EPSILON * 8.0,
            "each represented interval must age existing presentation: {age}"
        );

        game.present_ticks(6);
        assert!(
            !game
                .presentation
                .fx
                .iter()
                .any(|effect| matches!(effect.kind, EffectKind::Ping { .. })),
            "an effect must expire after enough presented sim time"
        );
    }

    #[test]
    fn paused_wall_time_freezes_presentation() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        game.presentation.fx.push(Effect {
            kind: EffectKind::Ping {
                at: macroquad::prelude::Vec2::ZERO,
                kind: PingKind::Move,
            },
            age: 0.0,
        });

        game.presentation.paused = true;
        game.update_wall_clock_fx(0.25);
        assert_eq!(
            game.presentation.fx_time(),
            0.0,
            "decorative animation must hold"
        );
        assert_eq!(
            game.presentation.fx[0].age, 0.0,
            "transient effects must hold too"
        );

        game.presentation.paused = false;
        game.update_wall_clock_fx(0.25);
        assert_eq!(game.presentation.fx_time(), 0.25);
        assert_eq!(game.presentation.fx[0].age, 0.25);
    }

    #[test]
    fn wall_clock_profile_bound_cannot_overshoot_inside_a_multi_tick_frame() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        game.presentation.speed = 8.0;

        assert!(game.advance_wall_clock(1.0, Some(5)));
        assert_eq!(game.state.current_tick(), 5);
        assert_eq!(game.presentation.tick_fraction(), 0.0);
    }

    #[test]
    fn playback_and_seeks_face_a_parked_airframe_by_its_heading() {
        let mut scenario = Scenario::skirmish();
        scenario.units = vec![oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: oxide_sim::UnitKind::Condor,
            x: 20,
            y: 12,
        }];
        let mut game = Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
            .expect("the landing scenario builds");
        let condor = game.state.units()[0].id;
        for _ in 0..600 {
            game.advance_ticks(1);
            if game.state.unit(condor).is_some_and(|u| u.landed) {
                break;
            }
        }
        let parked = game.state.unit(condor).expect("the Condor survives");
        assert!(parked.landed, "premise: the idle Condor parks itself");
        let expected =
            f32::from(parked.heading) * std::f32::consts::TAU / 256.0 + std::f32::consts::FRAC_PI_2;
        let snapshot = game.state.0.clone();

        game.replace_state_after_jump(&snapshot);
        assert_eq!(
            game.presentation.facing.get(&condor.0).copied(),
            Some(expected),
            "a seek shows the parked heading, not the default rotation"
        );

        game.presentation.facing.clear();
        game.presentation.remember_previous_tick(&game.state);
        game.presentation.observe_tick(&snapshot, &[], &[]);
        assert_eq!(
            game.presentation.facing.get(&condor.0).copied(),
            Some(expected),
            "playback faces the airframe as live play does"
        );
    }

    #[test]
    fn bulk_advance_drops_old_timeline_aim_and_facing() {
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        let unit = game.state.units()[0].id.0;
        let building = game.state.buildings()[0].id.0;
        game.presentation.facing.insert(unit, 1.25);
        game.presentation
            .aim_units
            .insert(unit, (2.5, game.presentation.fx_time()));
        game.presentation
            .aim_buildings
            .insert(building, (0.75, game.presentation.fx_time()));
        game.presentation
            .aim_building_targets
            .insert(building, Target::Unit(UnitId(unit)));

        game.advance_ticks(1);

        let expected = f32::from(game.state.unit(UnitId(unit)).unwrap().heading)
            * std::f32::consts::TAU
            / 256.0
            + std::f32::consts::FRAC_PI_2;
        assert_eq!(game.presentation.facing.get(&unit), Some(&expected));
        assert!(game.presentation.aim_units.is_empty());
        assert!(game.presentation.aim_buildings.is_empty());
        assert!(game.presentation.aim_building_targets.is_empty());
    }

    #[test]
    fn a_decisive_concession_uses_the_result_flow_not_the_overlay() {
        // 1v1: the surrender decides the match on its own tick, so the
        // normal end-of-match banner takes over.
        let mut game = Game::with_viewport(
            Scenario::skirmish(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("skirmish builds");
        game.issue(Command::Surrender);
        game.do_tick();
        assert!(game.state.player(game.presentation.human).resigned);
        assert!(game.state.result().is_some(), "a 1v1 concession decides");
        assert!(
            !game.presentation.conceded_banner,
            "no concede overlay over a result"
        );
        assert!(game.concede_stats.is_none());
        assert_eq!(
            game.end_stats.as_ref().map(|stats| stats.final_tick),
            Some(1),
            "the deciding tick leaves its report ready without replaying"
        );
    }

    #[test]
    fn a_team_concession_raises_the_surrender_overlay() {
        use oxide_sim::scenario::PlayerSpec;
        let seat = |name: &str, faction, team, bot| PlayerSpec {
            name: name.into(),
            faction,
            team: Some(team),
            scrap: 100,
            bot,
            bot_config: bot.then_some(oxide_sim::scenario::BotConfig::default()),
        };
        let scenario = Scenario {
            name: "concede-arena".into(),
            seed: 42,
            map: vec![
                "####################".into(),
                "#1..............3..#".into(),
                "#..................#".into(),
                "#..................#".into(),
                "#..................#".into(),
                "#2..............4..#".into(),
                "#..................#".into(),
                "####################".into(),
            ],
            players: vec![
                seat("West Ferrous", oxide_sim::Faction::Ferrous, 0, false),
                seat("West Cupric", oxide_sim::Faction::Cupric, 0, true),
                seat("East Ferrous", oxide_sim::Faction::Ferrous, 1, true),
                seat("East Cupric", oxide_sim::Faction::Cupric, 1, true),
            ],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        };
        let mut game = Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
            .expect("the concede arena builds");
        game.issue(Command::Surrender);
        game.do_tick();
        assert!(game.state.player(game.presentation.human).resigned);
        assert!(
            game.state.result().is_none(),
            "the ally's Foundry keeps the match running"
        );
        assert!(
            game.presentation.conceded_banner,
            "the undecided concession raises the overlay"
        );
        assert!(
            game.concede_stats.is_some(),
            "the exit offer carries the match-so-far numbers"
        );
        assert_eq!(
            game.concede_stats.as_ref().map(|stats| stats.final_tick),
            Some(1)
        );
        // The banner is a one-shot moment: later ticks never re-raise
        // a dismissed overlay.
        game.presentation.conceded_banner = false;
        game.do_tick();
        assert!(
            !game.presentation.conceded_banner,
            "spectating stays unobstructed"
        );
    }

    #[test]
    fn angular_interpolation_crosses_zero_by_the_short_arc() {
        let mut game = Game::new(Scenario::skirmish()).unwrap();
        let id = game.state.units()[0].id;
        game.presentation.prev_heading.insert(id.0, 254);
        let expected = std::f32::consts::TAU + std::f32::consts::FRAC_PI_2;
        assert!((game.presentation.draw_heading(id, 2, 0.5) - expected).abs() < 1e-6);
        game.presentation.prev_heading.insert(id.0, 2);
        assert!(
            (game.presentation.draw_heading(id, 254, 0.5) - std::f32::consts::FRAC_PI_2).abs()
                < 1e-6
        );
    }
}
