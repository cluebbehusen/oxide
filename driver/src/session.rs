//! The windowless session server: the full debug protocol with no GPU,
//! no window, and no wall clock.
//!
//! One [`Session`] holds a match open the way the shell's `Game` does —
//! sim state, seat bots, an always-on recorder, and a staging vec for
//! socket commands — but its advance loop is the headless runner's
//! composition and its screenshots come from the CPU renderer. It serves
//! the same [`oxide_protocol`] vocabulary over the same framed transport
//! ([`oxide_protocol::framing`]), so every `driver live` verb works
//! against it unchanged; requests that need a window (camera, UI, input
//! injection, the overlay) or a wall clock (pause, resume, speed) are
//! refused in words rather than faked.
//!
//! Parity with the live shell is the product, not a hope:
//! `driver/tests/session_parity.rs` drives the same script through both
//! servers and asserts hash, event, and status identity, and the
//! headless half of that suite runs in CI.

use crate::runner::{self, GameReplay};
use anyhow::{Context, Result};
use chassis::replay::Replay;
use oxide_protocol::framing::{IncomingRequest, Limits, incoming};
use oxide_protocol::{
    AdvancedView, DebugSession, PresentedView, Reply, Request, ResponseEnvelope, SavedView,
    ScreenshotView, StatusView, dispatch_shared, hash_hex,
};
use oxide_sim::bot::{SeatBot, seat_bots};
use oxide_sim::{Event, PlayerCommand, SIM_VERSION, Scenario, State};
use std::net::TcpListener;
use std::time::Duration;

/// A headless match a debug client can drive: the same session shape as
/// the shell's `Game`, minus everything presentational.
pub struct Session {
    scenario: Scenario,
    state: State,
    bots: Vec<SeatBot>,
    recorder: GameReplay,
    /// Commands staged for the next tick — the socket's funnel, exactly
    /// like a paused shell staging for the *next* tick.
    pending: Vec<PlayerCommand>,
}

impl Session {
    /// Opens a session on a scenario.
    pub fn new(scenario: Scenario) -> Result<Self> {
        let state = scenario.build().context("building scenario")?;
        let bots = seat_bots(&scenario).context("building public bot map briefing")?;
        let recorder = Replay::new(SIM_VERSION, scenario.clone());
        Ok(Self {
            scenario,
            state,
            bots,
            recorder,
            pending: Vec::new(),
        })
    }

    /// Resumes a session from a recorded replay, exactly as the shell
    /// does: validate (cross-version saves refused — resuming one would
    /// keep recording onto a log that can no longer reproduce), rebuild
    /// the scenario, re-execute every recorded tick, keep recording.
    pub fn resume(replay: GameReplay) -> Result<Self> {
        replay
            .validate(Some(SIM_VERSION))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let scenario = replay.setup.clone();
        let mut state = scenario.build().context("building replay setup")?;
        let total = replay.meta.ticks.unwrap_or_else(|| {
            replay
                .commands
                .last()
                .map_or(0, |c| c.tick.saturating_add(1))
        });
        // Same load bound as the shell: a structurally valid file can
        // still claim an absurd duration, and parity means refusing it
        // at the same line the shell does.
        anyhow::ensure!(
            total <= runner::MAX_REPLAY_TICKS,
            "replay spans {total} ticks, beyond the {}-tick load limit",
            runner::MAX_REPLAY_TICKS
        );
        // Bots may carry memory across ticks, so the fast-forward lets
        // them *watch* the session back: act() runs against every tick
        // and its outputs are discarded — the recorded commands are the
        // truth. The resumed session then continues exactly as the
        // unsaved one would have.
        let mut bots = seat_bots(&scenario).context("building replay public bot map briefing")?;
        let mut cursor = replay.cursor();
        for _ in 0..total {
            for bot in &mut bots {
                let _ = bot.act(&state);
            }
            let commands: Vec<PlayerCommand> = cursor
                .take_tick(state.current_tick())
                .iter()
                .map(|t| t.command.clone())
                .collect();
            state.tick(&commands);
        }
        anyhow::ensure!(
            cursor.is_finished(),
            "replay duration metadata does not cover its own commands"
        );
        Ok(Self {
            scenario,
            state,
            bots,
            recorder: replay,
            pending: Vec::new(),
        })
    }

    /// The current world, read-only (tests compare hashes through this).
    pub fn state(&self) -> &State {
        &self.state
    }

    /// One tick, the shell's `Game::do_tick` composition exactly: staged
    /// commands first, then bot commands, everything recorded, then the
    /// sim steps. Any deviation here is a parity bug by definition.
    fn step(&mut self) -> Vec<Event> {
        let mut commands = std::mem::take(&mut self.pending);
        for bot in &mut self.bots {
            commands.extend(bot.act(&self.state));
        }
        for command in &commands {
            self.recorder
                .record(self.state.current_tick(), command.clone());
        }
        self.state.tick(&commands).events
    }

    /// Answers one request. The shared surface (state reads, the driven
    /// clock) goes through the one protocol dispatcher; every remaining
    /// method is either implemented here or refused with the reason —
    /// never silently acknowledged.
    pub fn handle(&mut self, request: Request) -> Result<Reply, String> {
        if let Some(outcome) = dispatch_shared(self, &request) {
            return outcome;
        }
        match request {
            Request::SendCommand { player, command } => {
                if (player.0 as usize) < self.state.players().len() {
                    self.pending.push(PlayerCommand { player, command });
                    Ok(Reply::Ok)
                } else {
                    Err(format!("no such player {player}"))
                }
            }
            Request::Screenshot { path } => {
                let path = path.unwrap_or_else(|| {
                    format!("screenshots/tick-{}.png", self.state.current_tick())
                });
                self.screenshot(&path)
                    .map_err(|err| format!("screenshot: {err:#}"))
            }
            Request::LoadScenario { path } => Scenario::load(&path)
                .map_err(|err| format!("loading {path}: {err}"))
                .and_then(|scenario| {
                    Session::new(scenario).map_err(|err| format!("building scenario: {err:#}"))
                })
                .map(|fresh| {
                    *self = fresh;
                    Reply::Ok
                }),
            Request::LoadReplay { path } => oxide_kit::load_replay(&path)
                .map_err(|err| format!("loading replay {path}: {err}"))
                .and_then(|replay| {
                    Session::resume(replay).map_err(|err| format!("resuming replay: {err:#}"))
                })
                .map(|fresh| {
                    *self = fresh;
                    Reply::Status(self.status())
                }),
            Request::SaveReplay { path } => {
                self.recorder.meta.ticks = Some(self.state.current_tick());
                if let Some(parent) = std::path::Path::new(&path).parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent).ok();
                }
                match self.recorder.save(&path) {
                    Ok(()) => Ok(Reply::Saved(SavedView {
                        path,
                        commands: self.recorder.commands.len(),
                    })),
                    Err(err) => Err(format!("saving replay: {err}")),
                }
            }
            // No window exists: refusing beats pretending. Each message
            // names what is missing and, where one exists, the headless
            // way to get the same information.
            Request::QueryCamera => Err(
                "the headless session has no window or camera; its screenshots \
                 render the whole map"
                    .to_string(),
            ),
            Request::QueryUi => {
                Err("the headless session has no window or menus to report".to_string())
            }
            Request::QueryPerformance { .. } | Request::BeginPerformanceWindow { .. } => Err(
                "the headless session has no native window or GPU frame loop to profile"
                    .to_string(),
            ),
            Request::InjectEvent { .. } => Err(
                "the headless session has no input funnel; issue game commands \
                 with send_command"
                    .to_string(),
            ),
            Request::ToggleOverlay => Err(
                "the headless session has no renderer overlay; query_state is \
                 already omniscient"
                    .to_string(),
            ),
            // The shared surface was answered above (the clock family
            // refused from inside the clock methods); listing it keeps
            // this match exhaustive, so a new protocol request forces a
            // decision about which side of the capability split it
            // lives on.
            Request::Status
            | Request::QueryState { .. }
            | Request::QueryFogView { .. }
            | Request::StateHash
            | Request::AdvanceTicks { .. }
            | Request::PresentTicks { .. }
            | Request::Pause
            | Request::Resume
            | Request::SetSpeed { .. } => {
                unreachable!("shared requests are answered by dispatch_shared")
            }
        }
    }

    fn screenshot(&self, path: &str) -> Result<Reply> {
        let pixmap = crate::render::render_state(&self.state);
        if let Some(parent) = std::path::Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        pixmap
            .save_png(path)
            .with_context(|| format!("writing {path}"))?;
        Ok(Reply::Screenshot(ScreenshotView {
            path: path.to_string(),
            width: pixmap.width(),
            height: pixmap.height(),
            // The schematic tiny-skia render, NOT what the shell draws —
            // said on the wire so an agent never judges visual polish
            // from the wrong renderer.
            renderer: "cpu".to_string(),
        }))
    }
}

const NO_CLOCK: &str = "the headless session has no wall clock; sim time moves only through \
     advance_ticks and present_ticks";

/// The clockless third of the debug-session family: always in driven
/// mode, so the pause family is refused from inside the clock methods.
/// With no shell there are no transient effects to age — `present`
/// degenerates to "advance and return the events", deliberately the
/// same sim result as the shell's presented step, which is what the
/// parity suite pins.
impl DebugSession for Session {
    fn status(&self) -> StatusView {
        StatusView {
            tick: self.state.current_tick(),
            // No wall clock exists here: sim time moves only on request,
            // which reads as permanently paused — the same stance a
            // driven-mode shell reports.
            paused: true,
            speed: 1.0,
            scenario: self.scenario.name.clone(),
            sim_version: SIM_VERSION.to_string(),
            result: self.state.result(),
            recorded_commands: self.recorder.commands.len(),
        }
    }

    fn state(&self) -> &State {
        &self.state
    }

    fn advance(&mut self, ticks: u64) -> AdvancedView {
        for _ in 0..ticks {
            self.step();
        }
        AdvancedView {
            ticks,
            tick: self.state.current_tick(),
            hash: hash_hex(self.state.hash()),
        }
    }

    fn present(&mut self, ticks: u64) -> PresentedView {
        let mut events = Vec::new();
        for _ in 0..ticks {
            events.extend(self.step());
        }
        PresentedView {
            ticks,
            tick: self.state.current_tick(),
            hash: hash_hex(self.state.hash()),
            events,
        }
    }

    fn set_paused(&mut self, _paused: bool) -> Result<(), String> {
        Err(NO_CLOCK.to_string())
    }

    fn set_speed(&mut self, _multiplier: f64) -> Result<(), String> {
        Err(NO_CLOCK.to_string())
    }
}

/// Binds, announces, and answers until the process ends — the
/// `oxide-driver session` subcommand.
pub fn serve(port: u16, scenario: &str, idle_timeout: Duration) -> Result<()> {
    let scenario = runner::load_scenario(scenario)?;
    let session = Session::new(scenario)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("binding session server to 127.0.0.1:{port}"))?;
    eprintln!(
        "session server listening on 127.0.0.1:{port} — windowless, \
         sim time moves only on request"
    );
    let limits = Limits {
        idle_timeout,
        ..Limits::default()
    };
    serve_listener(listener, limits, session);
    Ok(())
}

/// The answering loop over an already-bound listener — tests bind port 0
/// and drive this on a thread. Single-threaded on purpose: requests from
/// every connection funnel through one channel, so each response reflects
/// a settled world, exactly as the shell answers between frames.
pub fn serve_listener(listener: TcpListener, limits: Limits, mut session: Session) {
    let rx = incoming(listener, limits);
    for IncomingRequest { id, request, reply } in rx {
        let response = match session.handle(request) {
            Ok(ok) => ResponseEnvelope::ok(id, ok),
            Err(err) => ResponseEnvelope::err(id, err),
        };
        reply.send(response).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::{Command, PlayerId};

    fn human_scenario(name: &str) -> Scenario {
        let mut scenario = Scenario::skirmish();
        scenario.name = name.to_string();
        for player in &mut scenario.players {
            player.bot = false;
            player.bot_config = None;
        }
        scenario
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "oxide-driver-session-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn failed_replacements_leave_state_and_staged_commands_intact() {
        let dir = scratch("failed-replace");
        let mut session = Session::new(human_scenario("original")).unwrap();
        session.handle(Request::AdvanceTicks { ticks: 2 }).unwrap();
        session
            .handle(Request::SendCommand {
                player: PlayerId(0),
                command: Command::Stop { units: Vec::new() },
            })
            .unwrap();
        let before_tick = session.state().current_tick();
        let before_hash = session.state().hash();

        let missing = dir.join("missing-scenario.json");
        assert!(
            session
                .handle(Request::LoadScenario {
                    path: missing.to_string_lossy().into_owned(),
                })
                .is_err()
        );
        let malformed = dir.join("malformed-replay.json");
        std::fs::write(&malformed, b"not a replay").unwrap();
        assert!(
            session
                .handle(Request::LoadReplay {
                    path: malformed.to_string_lossy().into_owned(),
                })
                .is_err()
        );
        assert_eq!(session.state().current_tick(), before_tick);
        assert_eq!(session.state().hash(), before_hash);

        session.handle(Request::PresentTicks { ticks: 1 }).unwrap();
        assert_eq!(session.status().recorded_commands, 1);
        assert_eq!(session.state().current_tick(), before_tick + 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn successful_scenario_replacement_resets_the_entire_session() {
        let dir = scratch("successful-replace");
        let replacement_path = dir.join("replacement.json");
        let replacement = human_scenario("replacement");
        std::fs::write(
            &replacement_path,
            serde_json::to_vec_pretty(&replacement).unwrap(),
        )
        .unwrap();

        let mut session = Session::new(human_scenario("original")).unwrap();
        session.handle(Request::AdvanceTicks { ticks: 4 }).unwrap();
        session
            .handle(Request::SendCommand {
                player: PlayerId(0),
                command: Command::Stop { units: Vec::new() },
            })
            .unwrap();

        assert!(matches!(
            session
                .handle(Request::LoadScenario {
                    path: replacement_path.to_string_lossy().into_owned(),
                })
                .unwrap(),
            Reply::Ok
        ));
        let status = session.status();
        assert_eq!(status.scenario, "replacement");
        assert_eq!(status.tick, 0);
        assert_eq!(status.recorded_commands, 0);
        session.handle(Request::PresentTicks { ticks: 1 }).unwrap();
        assert_eq!(
            session.status().recorded_commands,
            0,
            "old pending work was discarded"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn replay_without_duration_resumes_through_its_last_command() {
        let mut replay = GameReplay::new(SIM_VERSION, human_scenario("legacy"));
        replay.record(
            3,
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Stop { units: Vec::new() },
            },
        );
        assert_eq!(replay.meta.ticks, None);
        let expected = runner::run_replay(&replay, None, false).unwrap();

        let session = Session::resume(replay).unwrap();
        assert_eq!(session.state().current_tick(), 4);
        assert_eq!(session.state().hash(), expected.hash());
        assert_eq!(session.status().recorded_commands, 1);
    }

    #[test]
    fn commands_from_unknown_players_are_not_staged() {
        let mut session = Session::new(human_scenario("players")).unwrap();
        let error = session
            .handle(Request::SendCommand {
                player: PlayerId(99),
                command: Command::Stop { units: Vec::new() },
            })
            .unwrap_err();
        assert!(error.contains("no such player"));
        session.handle(Request::PresentTicks { ticks: 1 }).unwrap();
        assert_eq!(session.status().recorded_commands, 0);
    }
}
