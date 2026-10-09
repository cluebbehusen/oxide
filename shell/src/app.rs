//! The session coordinator: [`App`] owns everything that outlives a
//! screen, [`Screen`] owns everything that does not.
//!
//! Frame order is fixed and matters:
//!
//! 1. drain debug-socket requests (screenshots defer to post-render),
//! 2. gather input events — polled hardware first, then injected — and route
//!    them to the active screen (menu or the one gameplay input mapper),
//! 3. advance the sim by wall clock (unless paused; `advance_ticks` from the
//!    socket bypasses the clock entirely — that's driven mode),
//! 4. render with interpolation,
//! 5. capture any requested screenshot from the finished frame.
//!
//! Every screen variant carries that screen's complete state, so a mode
//! without its payload is unrepresentable.

mod audio;
mod debug;
mod persistence;
mod screen_flow;
mod screenshot;
mod ui_view;

use crate::debug_server::IncomingRequest;
use crate::frame_profile::{FrameObservation, FrameProfiler};
use crate::frame_time::FrameTime;
use crate::game::{Game, SoundKind};
use crate::menu::PreviewCache;
use crate::numeric;
use crate::screens::codex::CodexScreen;
use crate::screens::final_map::FinalMapScreen;
use crate::screens::home::HomeScreen;
use crate::screens::lobby::LobbyScreen;
use crate::screens::pause::PauseScreen;
use crate::screens::playback::{PlaybackSession, ReturnTo as PlaybackReturn};
use crate::screens::results::ResultsScreen;
use crate::screens::settings::SettingsScreen;
use crate::screens::shelf::Shelf;
use crate::screens::wizard::launch::{NewMatch, PersonalitySeedSource, start_new_match};
use crate::screens::wizard::{NewMatchDraft, Out as WizardOut, Step as WizardStep, Wizard};
use crate::{Args, assets, autosave, config, input, render, screens, theme, tutorial};
use anyhow::{Context, Result};
use macroquad::prelude::*;
use oxide_kit::diagnostics::{Stage, StageGuard};
use oxide_protocol::{Key, MouseButton, RawEvent, Request};
use oxide_sim::Scenario;
use std::sync::mpsc::Receiver;

/// Which screen owns input this frame, holding that screen's state.
/// Match choices live in the [`NewMatchDraft`] on [`App`], not in a
/// screen.
enum Screen {
    /// The front door: play, settings, quit.
    Home(HomeScreen),
    /// Settings and the Controls remap screen. `back` is where leaving
    /// returns to — Home, or the untouched pause menu whose payload
    /// waits here intact so the cursor comes back to the row that
    /// opened this screen.
    Settings {
        /// The screen itself.
        screen: SettingsScreen,
        /// The displaced screen, restored wholesale on leave.
        back: Box<Screen>,
    },
    /// The codex of machines and works, opened over Home or the paused
    /// match, which waits in `back` as it does under Settings.
    Codex {
        /// The screen itself.
        screen: CodexScreen,
        /// The displaced screen, restored wholesale on leave.
        back: Box<Screen>,
    },
    /// The New Match wizard (map grid, then match setup).
    Wizard(Wizard),
    /// A LAN match gathering its machines. Leaving drops every
    /// connection the lobby holds.
    Lobby {
        /// The screen itself.
        screen: Box<LobbyScreen>,
        /// Where leaving returns to: Home, or the match setup that hosted.
        back: Box<Screen>,
    },
    /// The game proper. The session lives in [`App::game`], which every
    /// screen needs as its backdrop.
    Playing,
    /// Read-only replay playback: the log is the match, seek included.
    /// Boxed: the session carries a whole presentation `Game`, and the
    /// enum should not make every other screen pay its size.
    Playback(Box<PlaybackSession>),
    /// The replay shelf.
    Replays(Shelf),
    /// Decided-match report and next steps.
    Results(ResultsScreen),
    /// Camera-only inspection of the already-final live state.
    FinalMap(FinalMapScreen),
    /// Game visible but veiled; the pause screen owns input,
    /// confirmation and save-naming state included.
    Pause(PauseScreen),
    Busy(Box<persistence::Busy>),
}

/// Everything that outlives a screen: the live session, the config,
/// the input funnel, and the presentation caches.
struct App {
    /// The parsed command line — session flags like `--paused` apply to
    /// every fresh match, not just the first.
    args: Args,
    /// Presentation config, persisted on change.
    config: config::Config,
    /// The live (or backdrop) game session.
    game: Game,
    /// The tutorial's lesson cards, while a tutorial runs. Lives here, not
    /// in a screen, so it survives Playing -> Pause -> Playing.
    tutorial: Option<tutorial::Tutorial>,
    /// The wizard's remembered choices. Lives here, not in the wizard, so
    /// Home -> Wizard -> Back -> Wizard keeps the map pick and dials.
    draft: NewMatchDraft,
    /// Shell-only entropy for New Match opponent identities. A base is
    /// consumed only when the wizard launches; the resulting exact seeds live
    /// in the scenario and are thereafter replay data.
    personality_seeds: PersonalitySeedSource,
    /// The one input funnel.
    input: input::InputState,
    /// Map preview textures for the browser and setup screens.
    previews: PreviewCache,
    /// A menu-context error line (message, wall-clock deadline): map
    /// and launch failures report here and the menus stay up — the
    /// in-game toast strip only draws with the HUD.
    menu_notice: Option<(String, f64)>,
    /// When the current screen's coaching text shows.
    hint_clock: crate::hints::HintClock,
    /// Window-size persistence: written once the size has been stable
    /// for a second, so a live resize does not save every intermediate
    /// size.
    pending_size: Option<((u32, u32), f64)>,
    /// Modifier truth for chord capture: tracked globally from raw
    /// events, every frame, whatever the screen — a Ctrl pressed on one
    /// screen must still read held after a switch, or Controls captures
    /// phantom bare chords.
    capture_ctrl: bool,
    /// See `capture_ctrl`.
    capture_shift: bool,
    /// Events injected over the debug socket, consumed next frame.
    injected: Vec<RawEvent>,
    /// Screenshot requests parked until after this frame renders.
    pending_shots: Vec<screenshot::PendingScreenshot>,
    /// The one texture atlas and its source rects.
    sprites: assets::Sprites,
    /// The generated clips.
    sounds: assets::Sounds,
    /// The rate-limiting clip player.
    mixer: crate::mixer::Mixer,
    /// Continuously running score beds and their pure crossfade state.
    ///
    /// Automation leaves this absent so a driven shell never starts
    /// long-lived audio sources merely to take screenshots.
    soundtrack: Option<crate::soundtrack::Soundtrack>,
    /// Opt-in bounded native-frame timing, queried over the debug socket.
    frame_profiler: FrameProfiler,
    report_job: crate::diagnostic_report::ReportJob,
    performance: crate::performance::Performance,
    persistence: persistence::Worker,
    persistence_result: Option<Result<persistence::Output>>,
    catalog_id: Option<u64>,
    catalog_delete: Option<std::path::PathBuf>,
    /// Presented frames in a row spent in live play; the suspension
    /// pause reads it to tell a stalled match from a heavy transition.
    live_streak: u8,
    /// Whether this app last asked for the on-screen keyboard.
    soft_keyboard: bool,
    /// The running LAN match's link, pumped every frame while a screen
    /// holds the match, so menus sit over a running match.
    net: Option<crate::netplay::Link>,
    /// A departed host's connections, still flushing the deciding batches.
    lingering: Option<crate::netplay::Lingering>,
    /// The session clock the lobby and the link read.
    clock: std::time::Instant,
}

fn result_playback(game: &Game) -> Result<PlaybackSession> {
    let mut replay = game.recorder.clone();
    replay.meta.ticks = Some(game.state.current_tick());
    let mut session = PlaybackSession::from_replay(replay)?;
    session.return_to = PlaybackReturn::Results;
    Ok(session)
}

#[expect(clippy::too_many_lines, reason = "startup and the native frame loop")]
pub(crate) async fn run(args: Args) -> Result<()> {
    let review_font = std::env::var_os("OXIDE_REVIEW_FONT")
        .map(std::fs::read)
        .transpose()
        .context("reading OXIDE_REVIEW_FONT")?;
    let mut font = load_ttf_font_from_bytes(
        review_font
            .as_deref()
            .unwrap_or(include_bytes!("../../assets/fonts/ChakraPetch-Medium.ttf")),
    )?;
    font.set_filter(FilterMode::Linear);
    macroquad::text::set_default_font(font.clone());
    crate::typography::install(font);
    let trace = crate::trace_startup_enabled(&args);
    let mark = |label: &str| {
        if trace {
            crate::trace_mark(label);
        }
    };
    mark("run enter (first frame)");
    match oxide_kit::diagnostics::Monitor::start(
        crate::paths::recovery_dir(),
        crate::build_identity(),
    ) {
        Ok(monitor) => drop(monitor.install()),
        Err(error) => eprintln!("Diagnostics unavailable: {error}"),
    }
    // Subscribe to hardware input before the prologue: the whole load
    // below runs inside frame 1, and events dispatched before the
    // subscriber exists are discarded, not queued. Automation shells
    // stay unarmed — they never poll, so a queue would never drain.
    if !args.automation {
        input::arm_hardware();
    }
    let config = config::Config::load();
    render::set_user_scale(config.ui_scale);
    render::set_reduced_motion(config.reduced_motion);
    render::set_colorblind(config.colorblind);
    render::set_control_groups(config.control_groups);
    crate::strategic_markers::set_prefs(config.markers);
    mark("config loaded");
    let sprites = assets::Sprites::load()?;
    mark("sprites loaded");
    let sounds = assets::Sounds::load().await?;
    mark("sounds loaded");
    let soundtrack = if args.automation {
        None
    } else {
        let mut soundtrack = crate::soundtrack::Soundtrack::default();
        soundtrack.start(&sounds);
        Some(soundtrack)
    };

    let mut game = if let Some(path) = &args.replay {
        let replay =
            oxide_kit::load_replay(path).with_context(|| format!("loading replay {path}"))?;
        Game::from_replay(replay)?
    } else {
        let scenario = match &args.scenario {
            Some(path) => Scenario::load(path).with_context(|| format!("loading {path}"))?,
            None => Scenario::skirmish(),
        };
        Game::new(scenario)?
    };
    game.recovery_root = crate::paths::recovery_dir();
    if game.recovery_root.is_none() {
        game.presentation
            .toast("Recovery unavailable: no writable data folder is configured");
    }
    game.presentation.paused = args.paused;
    game.presentation.speed = args.speed;
    mark("game built");

    // Launched for a purpose (a scenario, a resume, or an agent socket)?
    // Straight into the game. Everyone else — automation and humans
    // alike — starts cold at the Home front door.
    let purposeful =
        (args.debug_server && !args.automation) || args.scenario.is_some() || args.replay.is_some();
    let commit = crate::build_identity().revision;
    let mut screen = if let Some(address) = &args.host {
        let lobby = crate::netplay::HostLobby::new(
            address,
            game.scenario.clone(),
            game.presentation.human,
            &commit,
        )?;
        Screen::Lobby {
            screen: Box::new(LobbyScreen::waiting(crate::netplay::Lobby::Host(Box::new(
                lobby,
            )))),
            back: Box::new(Screen::Home(HomeScreen::open())),
        }
    } else if let Some(address) = &args.join {
        let address = crate::netplay::with_default_port(address);
        let lobby = crate::netplay::ClientLobby::new(&address, &commit);
        Screen::Lobby {
            screen: Box::new(LobbyScreen::waiting(crate::netplay::Lobby::Client(lobby))),
            back: Box::new(Screen::Home(HomeScreen::open())),
        }
    } else if let Some(path) = &args.watch {
        let mut session = PlaybackSession::open(path)?;
        // The clock flags drive whichever session is visible: a viewer
        // launch applies them to the transport, not the hidden match.
        session.paused = args.paused;
        session.speed = numeric::to_f32(args.speed);
        Screen::Playback(Box::new(session))
    } else if purposeful {
        Screen::Playing
    } else {
        let home = HomeScreen::open();
        mark("home screen open");
        Screen::Home(home)
    };

    // The title-bar close and Cmd-Q must reach the autosave path: left
    // to macroquad they exit the process before any save runs.
    prevent_quit();

    let debug_rx: Option<Receiver<IncomingRequest>> = if args.debug_server {
        let limits = oxide_protocol::framing::Limits {
            idle_timeout: std::time::Duration::from_secs(args.debug_idle_timeout),
            ..Default::default()
        };
        let rx = crate::debug_server::spawn(args.port, limits)?;
        mark("debug server up");
        Some(rx)
    } else {
        None
    };
    // Per-frame trace state: (frame index, previous frame top).
    let mut trace_frames: Option<(u32, std::time::Instant)> =
        trace.then(|| (0, std::time::Instant::now()));

    let profile_frames = args.profile_frames;
    let personality_seeds = PersonalitySeedSource::for_session(args.automation);
    let mut app = App {
        args,
        config,
        game,
        tutorial: None,
        draft: NewMatchDraft::default(),
        personality_seeds,
        input: input::InputState::new(),
        previews: PreviewCache::default(),
        menu_notice: None,
        hint_clock: crate::hints::HintClock::default(),
        pending_size: None,
        capture_ctrl: false,
        capture_shift: false,
        injected: Vec::new(),
        pending_shots: Vec::new(),
        sprites,
        sounds,
        mixer: crate::mixer::Mixer::default(),
        soundtrack,
        frame_profiler: FrameProfiler::new(profile_frames),
        report_job: crate::diagnostic_report::ReportJob::default(),
        persistence: persistence::Worker::new()?,
        persistence_result: None,
        catalog_id: None,
        catalog_delete: None,
        performance: crate::performance::Performance::default(),
        live_streak: 0,
        soft_keyboard: false,
        net: None,
        lingering: None,
        clock: std::time::Instant::now(),
    };
    let mut ui_view = ui_view::capture_ui(&screen, &app);
    // A rerun pass re-enters the loop inside the same presented frame;
    // the frame's starting screen is the one recorded before it.
    let mut rerun_pass = false;
    let mut began_playing = false;

    loop {
        if let Some(result) = app.report_job.poll() {
            let (text, danger) = match result {
                Ok(text) => (text, false),
                Err(error) => (format!("Diagnostic operation failed: {error}"), true),
            };
            if let Screen::Settings {
                screen: settings, ..
            } = &mut screen
            {
                settings.notice = Some(crate::screens::settings::Notice { text, danger });
            } else {
                app.menu_notice = Some((text, get_time() + 8.0));
            }
        }
        let (diagnostic_tick, diagnostic_units, diagnostic_buildings) =
            visible_profile_state(&screen, &app);
        oxide_kit::diagnostics::frame(oxide_kit::diagnostics::FrameContext {
            screen: visible_profile_mode(&screen),
            tick: diagnostic_tick,
            units: diagnostic_units,
            buildings: diagnostic_buildings,
            speed: visible_speed(&screen, &app.game),
            width: numeric::to_u32(screen_width()),
            height: numeric::to_u32(screen_height()),
            dpi: f64::from(macroquad::miniquad::window::dpi_scale()),
            paused: match &screen {
                Screen::Playback(playback) => playback.paused,
                Screen::Pause(_) => true,
                _ => app.game.presentation.paused,
            },
            minimized: input::reported_minimized(),
            recording: match &screen {
                Screen::Playback(_) => None,
                _ => app.game.recovery.as_ref(),
            },
        });
        let input_diagnostic_scope = visible_stage(&screen, &app, Stage::Input);
        app.game.poll_recovery();
        let time = FrameTime::measure(get_frame_time());
        let first_pass = !std::mem::take(&mut rerun_pass);
        if first_pass {
            began_playing = matches!(screen, Screen::Playing);
        }
        if let Some(rx) = &debug_rx {
            while let Ok(incoming) = rx.try_recv() {
                // An injected event is consumed by the NEXT frame; any
                // query drained after it in the same burst would answer
                // from the pre-input frame. Hold the rest of the queue
                // until the event has actually been felt.
                let holds_queries = matches!(incoming.request, Request::InjectEvent { .. });
                debug::handle_request(incoming, &mut app, &mut screen, &ui_view);
                if holds_queries {
                    break;
                }
            }
        }

        drop(input_diagnostic_scope);
        let screen_diagnostic_scope = visible_stage(&screen, &app, Stage::Draw);
        // Debug requests are control-plane work between presented frames, not
        // native frame work. Start timing after draining them so Resume and
        // status polling cannot become an artificial slow frame.
        let performance_started = app
            .performance
            .begin(app.config.performance_display, performance_context(&screen));
        let frame_started = if app.frame_profiler.enabled() {
            performance_started.or_else(|| Some(std::time::Instant::now()))
        } else {
            None
        };
        let frame_tick_start = visible_tick(&screen, &app);
        let frame_mode = visible_profile_mode(&screen);
        app.poll_persistence(&mut screen);
        // The camera never queries the window itself; feed it the viewport
        // once per frame (handles live resizes, keeps camera math pure),
        // then advance any zoom glide. Menus take the same injection —
        // their update logic runs headless in tests on the default size.
        app.game
            .presentation
            .camera
            .set_viewport(vec2(screen_width(), screen_height()));
        render::set_viewport(screen_width(), screen_height());
        app.game.presentation.camera.update(time.presentation);

        let input_diagnostic_scope = visible_stage(&screen, &app, Stage::Input);
        let mut events = hardware_events(first_pass && !app.args.automation, || {
            input::poll_events(text_entry(&screen))
        });
        if let Some((frame, last_top)) = trace_frames.as_mut() {
            *frame += 1;
            let now = std::time::Instant::now();
            let entry = crate::TRACE_ENTRY.get_or_init(std::time::Instant::now);
            eprintln!(
                "[trace-startup] [F] {frame} +{:.1}ms gap={:.1}ms hw_events={}",
                now.duration_since(*entry).as_secs_f64() * 1000.0,
                now.duration_since(*last_top).as_secs_f64() * 1000.0,
                events.len()
            );
            *last_top = now;
        }
        if trace_frames.is_some_and(|(frame, _)| frame >= crate::TRACE_FRAMES) {
            trace_frames = None;
        }
        events.append(&mut app.injected);
        // Start-of-frame modifier truth, saved before the fold below:
        // the Controls capture replays this frame's edges in order from
        // this baseline, so a chord fully pressed AND released inside
        // one frame still reads its modifiers as of the main key-down.
        let (ctrl_at_frame_start, shift_at_frame_start) = (app.capture_ctrl, app.capture_shift);
        for e in &events {
            track_pointer_position(&mut app.input.mouse, e);
            match e {
                RawEvent::KeyDown { key: Key::Ctrl } => app.capture_ctrl = true,
                RawEvent::KeyUp { key: Key::Ctrl } => app.capture_ctrl = false,
                RawEvent::KeyDown { key: Key::Shift } => app.capture_shift = true,
                RawEvent::KeyUp { key: Key::Shift } => app.capture_shift = false,
                _ => {}
            }
        }

        drop(input_diagnostic_scope);
        let now = app.clock.elapsed();
        if let Some(link) = &mut app.net
            && let Some(end) = link.pump(&mut app.game, now)
        {
            screen = app.end_network_match(end, screen);
        }
        if app
            .lingering
            .as_mut()
            .is_some_and(|lingering| !lingering.pump(now))
        {
            app.lingering = None;
        }
        screen = report_under_menu(&app.game, screen);
        let screen_before = std::mem::discriminant(&screen);
        let screen_frame = screen_flow::update_and_draw(
            &mut app,
            screen,
            events,
            time,
            ctrl_at_frame_start,
            shift_at_frame_start,
        )?;
        gl_use_default_material();
        screen = screen_frame.screen;
        // Before any rerun: the next pass belongs to the new screen.
        if std::mem::discriminant(&screen) != screen_before {
            app.input.reset_transient();
            macroquad::miniquad::window::set_mouse_cursor(macroquad::miniquad::CursorIcon::Default);
        }
        if !screen_holds_live_match(&screen)
            && let Some(link) = app.net.take()
        {
            app.lingering = link.leave(&app.game, app.clock.elapsed());
        }
        let rerun = screen_frame.rerun;
        let profile_frame_active = screen_frame.profile_frame_active;
        if rerun {
            app.performance.reset();
            record_profile_frame(
                &mut app,
                &screen,
                frame_mode,
                profile_frame_active,
                frame_tick_start,
                frame_started,
            );
            rerun_pass = true;
            continue;
        }

        // The menu-context error line draws over whichever menu is up;
        // the gameplay screens speak through the HUD's toast strip.
        if !matches!(
            screen,
            Screen::Playing | Screen::Playback(_) | Screen::FinalMap(_)
        ) && let Some((msg, until)) = &app.menu_notice
        {
            if get_time() < *until {
                let s = render::ui_scale();
                let width = measure_text(msg, None, numeric::font_size(16.0 * s), 1.0).width;
                let y = if matches!(screen, Screen::Results(_)) {
                    screens::results::action_rects(vec2(screen_width(), screen_height()), s)[0].y
                        - 10.0 * s
                } else {
                    screen_height() - 48.0 * s
                };
                draw_text(
                    msg,
                    (screen_width() - width) * 0.5,
                    y,
                    16.0 * s,
                    theme::TEXT_DANGER,
                );
            } else {
                app.menu_notice = None;
            }
        }

        // A touch-only player types through the on-screen keyboard, which
        // follows the name field: every way out of naming hides it, and a
        // tap on the field brings back one the player dismissed.
        let refocus = match &mut screen {
            Screen::Pause(pause) => pause.take_keyboard_request(),
            Screen::Lobby { screen, .. } => screen.take_keyboard_request(),
            _ => false,
        };
        let wanted = text_entry(&screen);
        if crate::platform::TOUCH_ONLY && (wanted != app.soft_keyboard || (wanted && refocus)) {
            macroquad::miniquad::window::show_keyboard(wanted);
            app.soft_keyboard = wanted;
        }
        ui_view = ui_view::capture_ui(&screen, &app);
        audio::frame(&mut app, &mut screen, time.presentation);

        screenshot::serve(&mut app);
        // Persist the window size once it has settled.
        let live = (
            numeric::to_u32(screen_width()),
            numeric::to_u32(screen_height()),
        );
        // Explicit --window runs (the UX matrix) and automation must
        // not overwrite the human's remembered size.
        let persist_size = app.args.window.is_none() && !app.args.automation;
        if persist_size && live.0 >= 640 && live.1 >= 400 && live != app.config.window {
            match app.pending_size {
                Some((size, since)) if size == live => {
                    if get_time() - since > 1.0 {
                        app.config.window = live;
                        if let Err(err) = app.config.save() {
                            let line = format!("could not save settings: {err}");
                            if matches!(screen, Screen::Playing) {
                                app.game.presentation.toast(line);
                            } else {
                                app.menu_notice = Some((line, get_time() + 5.0));
                            }
                        }
                        app.pending_size = None;
                    }
                }
                _ => app.pending_size = Some((live, get_time())),
            }
        } else {
            app.pending_size = None;
        }

        if is_quit_requested() {
            // Everything a quit must not lose: any settled-but-unwritten
            // window size, and the live session as an autosave. A failed
            // autosave swallows the quit (prevent_quit is in force) and
            // raises the failure dialog instead of exiting over data loss.
            app.config.save().ok();
            // The dialog's home-vs-match classification must see through
            // screens opened from Pause: a quit while Settings or Playback
            // sits over a paused match still has an unsaved match behind
            // it, and a Home-classified Cancel would strand it.
            let over_a_match = screen_holds_live_match(&screen);
            if let Screen::Busy(busy) = &mut screen {
                busy.request_quit();
            } else {
                screen = app.persistence_screen(
                    persistence::Intent::Leave(screens::pause::LeaveVerb::Quit, !over_a_match),
                    screen,
                );
            }
        }

        app.performance.finish(performance_started);
        record_profile_frame(
            &mut app,
            &screen,
            frame_mode,
            profile_frame_active,
            frame_tick_start,
            frame_started,
        );

        drop(screen_diagnostic_scope);
        app.live_streak = next_live_streak(
            app.live_streak,
            began_playing,
            matches!(screen, Screen::Playing),
        );
        let wait_diagnostic_scope = visible_stage(&screen, &app, Stage::Present);
        next_frame().await;
        drop(wait_diagnostic_scope);
    }
}

/// A LAN match runs under its menu and can end there; the report replaces a
/// menu built for the running match.
fn report_under_menu(game: &Game, screen: Screen) -> Screen {
    let stale =
        matches!(&screen, Screen::Pause(pause) if !pause.decided() && !pause.saving_failed());
    if stale
        && game.net_role().is_some()
        && game.state.result().is_some()
        && game.end_stats.is_some()
    {
        Screen::Results(ResultsScreen::open())
    } else {
        screen
    }
}

/// The Surrender row shares the simulation's seat command gate.
fn can_surrender(game: &Game) -> bool {
    game.state.accepts_commands(game.presentation.human)
}

/// The pause rows for the live match; a LAN match keeps running under
/// its menu and offers no save or restart.
fn pause_menu(game: &Game) -> PauseScreen {
    let pause = PauseScreen::open(game.state.result().is_some(), can_surrender(game));
    if game.net_role().is_some() {
        pause.for_lan_match()
    } else {
        pause
    }
}

fn visible_stage(screen: &Screen, app: &App, stage: Stage) -> Option<StageGuard<'static>> {
    oxide_kit::diagnostics::stage(stage, visible_tick(screen, app))
}

fn visible_tick(screen: &Screen, app: &App) -> u64 {
    match screen {
        Screen::Playback(playback) => playback.engine.position(),
        _ => app.game.state.current_tick(),
    }
}

fn performance_context(screen: &Screen) -> u8 {
    match screen {
        Screen::Playing => 1,
        Screen::Playback(_) => 2,
        Screen::FinalMap(_) => 3,
        _ => 0,
    }
}

fn visible_speed(screen: &Screen, live: &Game) -> f64 {
    match screen {
        Screen::Playback(playback) => f64::from(playback.speed),
        _ => live.presentation.speed,
    }
}

fn visible_profile_mode(screen: &Screen) -> &'static str {
    match screen {
        Screen::Home(_) => "home",
        Screen::Settings { .. } => "settings",
        Screen::Codex { .. } => "codex",
        Screen::Wizard(_) => "wizard",
        Screen::Lobby { .. } => "lobby",
        Screen::Playing => "playing",
        Screen::Playback(_) => "playback",
        Screen::FinalMap(_) => "final_map",
        Screen::Results(_) => "results",
        Screen::Replays(_) => "replays",
        Screen::Pause(_) => "pause",
        Screen::Busy(busy) => busy.mode(),
    }
}

fn record_profile_frame(
    app: &mut App,
    screen: &Screen,
    mode: &str,
    active_playing: bool,
    tick_start: u64,
    started: Option<std::time::Instant>,
) {
    let Some(started) = started else {
        return;
    };
    let (tick_end, units, buildings) = visible_profile_state(screen, app);
    app.frame_profiler.record(FrameObservation {
        mode,
        active_playing,
        tick_start,
        tick_end,
        work_ms: started.elapsed().as_secs_f64() * 1000.0,
        units,
        buildings,
    });
}

fn visible_profile_state(screen: &Screen, app: &App) -> (u64, usize, usize) {
    let state = match screen {
        Screen::Playback(playback) => &playback.engine.state,
        _ => &app.game.state,
    };
    (
        state.current_tick(),
        state.units().len(),
        state.buildings().len(),
    )
}

/// Whether closing the window would abandon a live match hidden behind the
/// current screen. This decides whether Cancel on a failed quit-save can
/// return to that match or must return to the front door.
fn screen_holds_live_match(screen: &Screen) -> bool {
    match screen {
        Screen::Playing | Screen::Results(_) | Screen::FinalMap(_) | Screen::Pause(_) => true,
        Screen::Settings { back, .. } | Screen::Codex { back, .. } => {
            matches!(**back, Screen::Pause(_))
        }
        Screen::Playback(playback) => matches!(
            playback.return_to,
            PlaybackReturn::Pause | PlaybackReturn::Results
        ),
        Screen::Home(_) | Screen::Wizard(_) | Screen::Replays(_) | Screen::Lobby { .. } => false,
        Screen::Busy(_) => true,
    }
}

/// Keeps the cross-screen cursor position current even when a click arrives
/// without a preceding move event, as injected and some native clicks do.
fn track_pointer_position(mouse: &mut Vec2, event: &RawEvent) {
    match *event {
        RawEvent::MouseMove { x, y }
        | RawEvent::MouseDown { x, y, .. }
        | RawEvent::MouseUp { x, y, .. } => *mouse = vec2(x, y),
        _ => {}
    }
}

impl App {
    /// Swaps in a freshly built match: session toggles carry over from the
    /// old game, the frame profile and transient input start clean, and the
    /// tutorial is whatever this session calls for.
    fn install_session(&mut self, fresh: Game, paused: bool, tutorial: Option<tutorial::Tutorial>) {
        self.tutorial = tutorial;
        self.game = keep_flags(fresh, &self.game);
        self.game.presentation.paused = paused;
        self.performance.reset();
        self.input.reset_session();
    }

    /// Swaps in a LAN match from the lobby with the link that carries it:
    /// real time, no overlay, no tutorial.
    fn install_networked(&mut self, game: Game, link: crate::netplay::Link) {
        self.install_session(game, false, None);
        self.game.presentation.speed = 1.0;
        self.game.presentation.overlay = false;
        self.net = Some(link);
    }

    /// Reports why a LAN match ended and leaves it through the ordinary
    /// leave save, which keeps its replay.
    fn end_network_match(&mut self, end: crate::netplay::End, screen: Screen) -> Screen {
        self.menu_notice = Some((end.notice(), get_time() + 10.0));
        if matches!(screen, Screen::Busy(_)) {
            return screen;
        }
        self.persistence_screen(
            persistence::Intent::Leave(screens::pause::LeaveVerb::MainMenu, false),
            screen,
        )
    }
}

/// Carries session-level toggles (pause/speed/overlay) onto a fresh game.
fn keep_flags(mut fresh: Game, old: &Game) -> Game {
    if let Some(writer) = &old.recovery {
        writer.finish(old.state.current_tick());
    }
    fresh.presentation.paused = old.presentation.paused;
    fresh.presentation.speed = old.presentation.speed;
    fresh.presentation.overlay = old.presentation.overlay;
    fresh.recovery_root.clone_from(&old.recovery_root);
    fresh
}

/// Whether a text field owns input: the save-name field or the host
/// address.
fn text_entry(screen: &Screen) -> bool {
    match screen {
        Screen::Pause(pause) => pause.naming(),
        Screen::Lobby { screen, .. } => screen.text_entry(),
        _ => false,
    }
}

/// Consecutive presented frames that began and ended in live play.
fn next_live_streak(streak: u8, began_playing: bool, ended_playing: bool) -> u8 {
    if began_playing && ended_playing {
        streak.saturating_add(1)
    } else {
        0
    }
}

/// This pass's hardware input. Macroquad reports a frame's key presses until
/// the frame is presented, so a rerun pass inside the same frame must not read
/// them again: the screen a key opened would receive that key too.
fn hardware_events(poll_hardware: bool, poll: impl FnOnce() -> Vec<RawEvent>) -> Vec<RawEvent> {
    if poll_hardware { poll() } else { Vec::new() }
}

/// Dark translucent layer between the world and a menu.
fn veil() {
    // Dark enough that the game behind reads as backdrop texture, not
    // as competing UI — the HUD's own text lines must not fight the
    // menu's.
    draw_rectangle(
        0.0,
        0.0,
        screen_width(),
        screen_height(),
        Color::new(0.04, 0.04, 0.06, 0.96),
    );
}

#[cfg(test)]
mod tests;
