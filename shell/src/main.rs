//! The Oxide shell: a thin macroquad window over the deterministic sim.
//!
//! Frame order is fixed and matters:
//!
//! 1. drain debug-socket requests (screenshots defer to post-render),
//! 2. gather input events — polled hardware first, then injected — and route
//!    them to the active mode (menu or the one gameplay input mapper),
//! 3. advance the sim by wall clock (unless paused; `advance_ticks` from the
//!    socket bypasses the clock entirely — that's driven mode),
//! 4. render with interpolation,
//! 5. capture any requested screenshot from the finished frame.
//!
//! Nothing in this crate may affect game outcomes except by staging
//! tick-stamped commands. If a feature can't be expressed that way, it
//! belongs in the sim.

mod action;
mod assets;
mod autosave;
mod camera;
mod config;
mod debug_server;
mod game;
mod input;
mod layout;
mod menu;
mod panel;
mod paths;
mod render;
mod saves;
mod screens;
mod theme;
mod tutorial;

use anyhow::{Context, Result};
use clap::Parser;
use debug_server::IncomingRequest;
use game::{Game, GameReplay, SoundKind};
use macroquad::audio::{PlaySoundParams, play_sound};
use macroquad::prelude::*;
use menu::{Menu, PreviewCache};
use oxide_protocol::{
    AdvancedView, CameraView, HashView, Key, MouseButton, OverlayView, PresentedView, RawEvent,
    Reply, Request, ResponseEnvelope, SavedView, ScreenshotView, StateView, StatusView, UiView,
};
use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario};
use std::sync::mpsc::{Receiver, Sender};

#[derive(Parser)]
#[command(name = "oxide-shell", version, about = "Oxide, playable")]
struct Args {
    /// Scenario JSON path (skips the menu).
    #[arg(long, conflicts_with = "replay")]
    scenario: Option<String>,

    /// Resume a session from a replay JSON (skips the menu).
    #[arg(long)]
    replay: Option<String>,

    /// Open a replay in the read-only playback viewer (pause, speed,
    /// seek — no recorder, no commands).
    #[arg(long, conflicts_with_all = ["scenario", "replay", "automation"])]
    watch: Option<String>,

    /// Serve the debug protocol on --port (skips the menu unless automated).
    #[arg(long)]
    debug_server: bool,

    /// Deterministic UI-driving mode: start at the main menu and accept only
    /// injected input, never hardware input.
    #[arg(
        long,
        requires = "debug_server",
        conflicts_with_all = ["scenario", "replay"]
    )]
    automation: bool,

    /// Debug server port.
    #[arg(long, default_value_t = oxide_protocol::DEFAULT_PORT)]
    port: u16,

    /// Start with the sim clock stopped: time advances only via the debug
    /// socket (driven mode). Rendering still runs.
    #[arg(long)]
    paused: bool,

    /// Wall-clock speed multiplier.
    #[arg(long, default_value_t = 1.0, value_parser = parse_speed)]
    speed: f64,

    /// Window size as WIDTHxHEIGHT (e.g. 800x600) — the UX matrix boots
    /// the shell at every supported size.
    #[arg(long, value_parser = parse_window)]
    window: Option<(u32, u32)>,

    /// Render at logical (non-retina) pixel density, exercising the 1x
    /// layout path on high-DPI displays.
    #[arg(long)]
    no_high_dpi: bool,

    /// Print startup diagnostics to stderr: prologue milestones with
    /// ms-since-entry, then per-frame gap and hardware-event counts for
    /// the first frames. OXIDE_TRACE_STARTUP=1 enables it too (handy
    /// for the packaged .app, where flags are awkward).
    #[arg(long)]
    trace_startup: bool,
}

/// Wall-clock zero for the startup trace, pinned by the first caller —
/// `window_conf` runs before the window exists, so it lands close to
/// process entry.
static TRACE_ENTRY: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Frames the startup trace reports before going quiet.
const TRACE_FRAMES: u32 = 200;

/// The env var alone must not switch tracing on under `--automation`:
/// the shots and menu_ux harnesses capture stderr from spawned shells,
/// and an exported OXIDE_TRACE_STARTUP would leak into every one. The
/// explicit flag always wins.
fn trace_active(flag: bool, automation: bool, env_set: bool) -> bool {
    flag || (env_set && !automation)
}

fn trace_startup_enabled(args: &Args) -> bool {
    let env_set = std::env::var("OXIDE_TRACE_STARTUP").is_ok_and(|v| v == "1");
    trace_active(args.trace_startup, args.automation, env_set)
}

fn trace_mark(label: &str) {
    let entry = TRACE_ENTRY.get_or_init(std::time::Instant::now);
    eprintln!(
        "[trace-startup] {label} +{:.1}ms",
        entry.elapsed().as_secs_f64() * 1000.0
    );
}

/// Parses `WIDTHxHEIGHT` with sane floors — smaller than 640x400 and the
/// fixed chrome cannot physically fit.
fn parse_window(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once('x')
        .ok_or_else(|| "expected WIDTHxHEIGHT, e.g. 800x600".to_string())?;
    let (w, h): (u32, u32) = (
        w.parse().map_err(|err| format!("{err}"))?,
        h.parse().map_err(|err| format!("{err}"))?,
    );
    if w < 640 || h < 400 {
        return Err("window must be at least 640x400".to_string());
    }
    if w > 16_384 || h > 16_384 {
        // The native config takes i32; 4294967295x400 once reached the
        // backend as -1.
        return Err("window must be at most 16384x16384".to_string());
    }
    Ok((w, h))
}

/// Same envelope the debug socket enforces — the CLI shouldn't accept less
/// sane values than the protocol does.
fn parse_speed(s: &str) -> Result<f64, String> {
    let v: f64 = s.parse().map_err(|err| format!("{err}"))?;
    if v.is_finite() && (0.05..=64.0).contains(&v) {
        Ok(v)
    } else {
        Err("speed must be a finite value within 0.05..=64".to_string())
    }
}

fn window_conf() -> Conf {
    TRACE_ENTRY.get_or_init(std::time::Instant::now);
    // The window is created before `run()` ever sees clap's output, so
    // the size/DPI flags are parsed here too — clap is idempotent and
    // errors surface identically on the second parse in `run()`.
    let args = Args::parse();
    if trace_startup_enabled(&args) {
        trace_mark("window_conf enter");
    }
    let (width, height) = args.window.unwrap_or(config::Config::load().window);
    Conf {
        window_title: "Oxide".to_string(),
        window_width: width as i32,
        window_height: height as i32,
        // Render at native pixel density — pre-atlas this was too many
        // pixels to afford; post-atlas it's crisp text and art for free.
        high_dpi: !args.no_high_dpi,
        // Dock/taskbar face on every backend that takes one (miniquad
        // hands `big` to the macOS dock). Generated by tools/gen_icon.py;
        // the packaged .app carries the same mark as its icns.
        icon: Some(macroquad::miniquad::conf::Icon {
            small: *include_bytes!("../../assets/icon/oxide_16.rgba"),
            medium: *include_bytes!("../../assets/icon/oxide_32.rgba"),
            big: *include_bytes!("../../assets/icon/oxide_64.rgba"),
        }),
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("fatal: {err:#}");
        std::process::exit(1);
    }
}

/// Which screen owns input this frame. Screens carry no choices — the
/// [`NewMatchDraft`] does, which is what lets Back walk the flow
/// without losing anything.
enum Mode {
    /// The front door: play, settings, quit.
    Home,
    /// Settings and the Controls remap screen (state in the `settings`
    /// session local).
    Settings,
    /// The New Match wizard (map grid, then match setup); its state
    /// lives in the `wizard` session local.
    Wizard,
    /// The game proper.
    Playing,
    /// Read-only replay playback: the log is the match, seek included.
    Playback,
    /// The replay shelf (state in the `shelf` session local).
    Replays,
    /// Game visible but veiled; the pause screen owns input (state in
    /// the `pause` session local, confirmation included).
    Pause,
}

use screens::wizard::{NewMatchDraft, Out as WizardOut, Step as WizardStep, Wizard};

fn launch(draft: &NewMatchDraft) -> Result<Game> {
    let mut scenario = (**draft.scenario.as_ref().context("draft has a map")?).clone();
    // ONE consumer, one source: the per-seat vector the setup screen
    // filled. Every AI seat gets its own config here, and the vacated
    // human seat can never fall to the team-blind classic bot.
    anyhow::ensure!(
        draft.seats.len() == scenario.players.len(),
        "draft seats out of step with the map"
    );
    // Discovery lists every parseable JSON without building it, so a
    // zero-seat file can reach here — refuse it as a launch error
    // instead of underflowing the seat clamp below.
    anyhow::ensure!(!scenario.players.is_empty(), "the map has no player seats");
    let seat_choice = draft.seat_choice.min(scenario.players.len() - 1);
    for (i, player) in scenario.players.iter_mut().enumerate() {
        player.bot = i != seat_choice;
        player.bot_config = if player.bot {
            let plan = draft.seats[i];
            Some(oxide_sim::scenario::BotConfig {
                level: oxide_sim::bot::Level::LADDER[plan.level_choice.min(3)],
                aggression: screens::wizard::personality_knob(plan.personality_choice),
            })
        } else {
            None
        };
    }
    // Per-seat faction chips (the setup screen's): Auto keeps the
    // authored roster; an override retints the seat, starting units
    // remapped through their roles. Same-faction opponents are
    // readable now — the allegiance accents carry friend-or-foe, so
    // faction is a free choice, not a fairness rule.
    for (i, plan) in draft.seats.iter().enumerate() {
        if let Some(faction) = screens::wizard::faction_override(plan.faction_choice) {
            scenario.retint_seat(i, faction);
        }
    }
    // Duels run through the same per-seat chips as every other map:
    // Auto keeps the authored roster (even seats Ferrous, odd Cupric —
    // the classic matchup), and a chip override is how a mirror match
    // or a faction swap gets arranged. The old quick flow forced the
    // opponent onto the complementary roster; nothing forces anything
    // now.
    // Seat names must stay unique: the victory banner, the panel, and
    // the stats screen all address seats by name. Retints can land two
    // seats on one faction-derived label ("North West Ferrous" twice),
    // so duplicates take an ordinal instead of refusing to launch.
    let mut seen: Vec<String> = Vec::new();
    for player in scenario.players.iter_mut() {
        if seen.contains(&player.name) {
            let mut n = 2;
            while seen.contains(&format!("{} {n}", player.name)) {
                n += 1;
            }
            player.name = format!("{} {n}", player.name);
        }
        seen.push(player.name.clone());
    }
    let mut names: Vec<&str> = scenario.players.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    anyhow::ensure!(
        names.len() == scenario.players.len(),
        "seat names collide after setup"
    );
    Game::new(scenario)
}

/// A screenshot request parked until after this frame renders.
struct PendingScreenshot {
    id: u64,
    path: String,
    reply: Sender<ResponseEnvelope>,
}

/// Home rows; Continue appears only when a compatible autosave exists,
/// so the returned flag says whether row indices are shifted by one.
/// Plays queued clips with a per-kind rate limit, so twenty simultaneous
/// lasers read as battle, not noise.
#[derive(Default)]
struct Mixer {
    last_played: std::collections::HashMap<SoundKind, f64>,
    /// Alternates the basic zap between two clips so volleys read as
    /// many guns, not one sample looping.
    laser_flip: bool,
}

impl Mixer {
    /// Which settings bus a clip bills against.
    fn bus(volumes: &config::Volumes, kind: SoundKind) -> f32 {
        let bus = match kind {
            SoundKind::Click | SoundKind::Denied => volumes.ui,
            _ => volumes.effects,
        };
        volumes.master * bus
    }

    fn play(
        &mut self,
        sounds: &assets::Sounds,
        kind: SoundKind,
        volumes: &config::Volumes,
        attenuation: f32,
    ) {
        let now = get_time();
        let min_gap = match kind {
            SoundKind::Laser => 0.09,
            SoundKind::RailFire => 0.15,
            SoundKind::UnitDeath => 0.12,
            SoundKind::Flak => 0.12,
            SoundKind::Artillery => 0.2,
            SoundKind::ArtilleryLaunch => 0.2,
            SoundKind::Ack => 0.15,
            _ => 0.05,
        };
        if now - self.last_played.get(&kind).copied().unwrap_or(f64::MIN) < min_gap {
            return;
        }
        self.last_played.insert(kind, now);
        let (sound, volume) = match kind {
            SoundKind::Laser => {
                self.laser_flip = !self.laser_flip;
                (
                    if self.laser_flip {
                        &sounds.laser
                    } else {
                        &sounds.laser2
                    },
                    0.18,
                )
            }
            SoundKind::RailFire => (&sounds.rail_fire, 0.4),
            SoundKind::UnitDeath => (&sounds.unit_death, 0.35),
            SoundKind::BuildingBoom => (&sounds.building_boom, 0.6),
            SoundKind::Deposit => (&sounds.deposit, 0.25),
            SoundKind::TrainDone => (&sounds.train_done, 0.3),
            SoundKind::Click => (&sounds.click, 0.25),
            SoundKind::Denied => (&sounds.denied, 0.3),
            SoundKind::Victory => (&sounds.victory, 0.6),
            SoundKind::Defeat => (&sounds.defeat, 0.6),
            SoundKind::Flak => (&sounds.flak, 0.3),
            SoundKind::Artillery => (&sounds.artillery_boom, 0.5),
            SoundKind::ArtilleryLaunch => (&sounds.artillery_launch, 0.35),
            SoundKind::Ack => (&sounds.ack, 0.18),
        };
        let volume = volume * Self::bus(volumes, kind) * attenuation;
        if volume <= 0.0 {
            return;
        }
        play_sound(
            sound,
            PlaySoundParams {
                looped: false,
                volume,
            },
        );
    }
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let trace = trace_startup_enabled(&args);
    let mark = |label: &str| {
        if trace {
            trace_mark(label);
        }
    };
    mark("run enter (first frame)");
    // Subscribe to hardware input before the prologue: the whole load
    // below runs inside frame 1, and events dispatched before the
    // subscriber exists are discarded, not queued. Automation shells
    // stay unarmed — they never poll, so a queue would never drain.
    if !args.automation {
        input::arm_hardware();
    }
    let mut config = config::Config::load();
    render::set_user_scale(config.ui_scale);
    render::set_reduced_motion(config.reduced_motion);
    render::set_colorblind(config.colorblind);
    mark("config loaded");
    let sprites = assets::Sprites::load().await?;
    mark("sprites loaded");
    let sounds = assets::Sounds::load().await?;
    mark("sounds loaded");
    let mut mixer = Mixer::default();

    let mut game = if let Some(path) = &args.replay {
        let replay = GameReplay::load(path).with_context(|| format!("loading replay {path}"))?;
        Game::from_replay(replay)?
    } else {
        let scenario = match &args.scenario {
            Some(path) => Scenario::load(path).with_context(|| format!("loading {path}"))?,
            None => Scenario::skirmish(),
        };
        Game::new(scenario)?
    };
    game.paused = args.paused;
    game.speed = args.speed;
    mark("game built");

    // Launched for a purpose (a scenario, a resume, or an agent socket)?
    // Straight into the game. Everyone else — automation and humans
    // alike — starts cold at the Home front door.
    let mut home = screens::home::HomeScreen::open();
    mark("home screen open");
    let mut playback: Option<screens::playback::PlaybackSession> = None;
    let mut tutorial: Option<tutorial::Tutorial> = None;
    // Window-size persistence: written once the size has been stable
    // for a second — a live resize is a burst of intermediate sizes
    // nobody wants fsynced.
    let mut pending_size: Option<((u32, u32), f64)> = None;
    let mut shelf: Option<screens::shelf::Shelf> = None;
    // Modifier truth for chord capture: tracked globally from raw
    // events, every frame, whatever the mode — a Ctrl pressed on one
    // screen must still read held after a mode switch, or Controls
    // captures phantom bare chords.
    let (mut capture_ctrl, mut capture_shift) = (false, false);
    if let Some(path) = &args.watch {
        let mut session = screens::playback::PlaybackSession::open(path)?;
        // The clock flags drive whichever session is visible: a viewer
        // launch applies them to the transport, not the hidden match.
        session.paused = args.paused;
        session.speed = args.speed as f32;
        playback = Some(session);
    }
    let purposeful =
        (args.debug_server && !args.automation) || args.scenario.is_some() || args.replay.is_some();
    let mut mode = if playback.is_some() {
        Mode::Playback
    } else if purposeful {
        Mode::Playing
    } else {
        Mode::Home
    };
    let mut draft = NewMatchDraft::default();
    // A menu-context error line (message, wall-clock deadline): map
    // and launch failures report here and the menus stay up — the
    // in-game toast strip only draws with the HUD.
    let mut menu_notice: Option<(String, f64)> = None;
    let mut previews = PreviewCache::default();
    let mut wizard: Option<Wizard> = None;
    let mut pause: Option<screens::pause::PauseScreen> = None;
    let mut settings: Option<screens::settings::SettingsScreen> = None;

    // The title-bar close and Cmd-Q must reach the autosave path: left
    // to macroquad they exit the process before any save runs.
    prevent_quit();

    let debug_rx: Option<Receiver<IncomingRequest>> = if args.debug_server {
        let rx = debug_server::spawn(args.port)?;
        mark("debug server up");
        Some(rx)
    } else {
        None
    };
    // Per-frame trace state: (frame index, previous frame top).
    let mut trace_frames: Option<(u32, std::time::Instant)> =
        trace.then(|| (0, std::time::Instant::now()));
    let mut input = input::InputState::new();
    let mut injected: Vec<RawEvent> = Vec::new();
    let mut pending_shots: Vec<PendingScreenshot> = Vec::new();
    let mut ui_view = capture_ui(
        &mode, &home, &wizard, &draft, &shelf, &pause, &settings, &game,
    );

    loop {
        let dt = get_frame_time();
        // The camera never queries the window itself; feed it the viewport
        // once per frame (handles live resizes, keeps camera math pure),
        // then advance any zoom glide. Menus take the same injection —
        // their update logic runs headless in tests on the default size.
        game.camera
            .set_viewport(vec2(screen_width(), screen_height()));
        render::set_viewport(screen_width(), screen_height());
        game.camera.update(dt);

        if let Some(rx) = &debug_rx {
            while let Ok(incoming) = rx.try_recv() {
                // An injected event is consumed by the NEXT frame; any
                // query drained after it in the same burst would answer
                // from the pre-input frame. Hold the rest of the queue
                // until the event has actually been felt.
                let holds_queries = matches!(incoming.request, Request::InjectEvent { .. });
                handle_request(
                    incoming,
                    &mut game,
                    &mut mode,
                    &mut input,
                    &mut injected,
                    &mut pending_shots,
                    &ui_view,
                    &mut tutorial,
                    &mut playback,
                    &mut settings,
                );
                if holds_queries {
                    break;
                }
            }
        }

        let mut events = if args.automation {
            Vec::new()
        } else {
            input::poll_events()
        };
        if let Some((frame, last_top)) = trace_frames.as_mut() {
            *frame += 1;
            let now = std::time::Instant::now();
            let entry = TRACE_ENTRY.get_or_init(std::time::Instant::now);
            eprintln!(
                "[trace-startup] [F] {frame} +{:.1}ms gap={:.1}ms hw_events={}",
                now.duration_since(*entry).as_secs_f64() * 1000.0,
                now.duration_since(*last_top).as_secs_f64() * 1000.0,
                events.len()
            );
            *last_top = now;
        }
        if trace_frames.is_some_and(|(frame, _)| frame >= TRACE_FRAMES) {
            trace_frames = None;
        }
        events.append(&mut injected);
        // Start-of-frame modifier truth, saved before the fold below:
        // the Controls capture replays this frame's edges in order from
        // this baseline, so a chord fully pressed AND released inside
        // one frame still reads its modifiers as of the main key-down.
        let (ctrl_at_frame_start, shift_at_frame_start) = (capture_ctrl, capture_shift);
        for e in &events {
            match e {
                RawEvent::KeyDown { key: Key::Ctrl } => capture_ctrl = true,
                RawEvent::KeyUp { key: Key::Ctrl } => capture_ctrl = false,
                RawEvent::KeyDown { key: Key::Shift } => capture_shift = true,
                RawEvent::KeyUp { key: Key::Shift } => capture_shift = false,
                _ => {}
            }
        }

        let mode_before = std::mem::discriminant(&mode);
        match mode {
            Mode::Home => {
                // The title scene: a cold front door drifts its camera
                // slowly across the backdrop world instead of freezing
                // a frame — presentation only, and only while nothing
                // is at stake (a resumable match keeps its exact view).
                if game.state.current_tick() == 0 && !render::reduced_motion() {
                    game.camera.pan(vec2(dt * 0.55, dt * 0.22));
                    let (_, hi) = game.camera.world_rect();
                    if hi.x >= game.state.map().width() as f32 + 1.9 {
                        game.camera.center = vec2(0.0, 0.0);
                        game.camera.pan(vec2(0.0, 0.0)); // re-clamp home
                    }
                }
                match home.update(&events, &mut input.mouse, &mut game.sounds_pending) {
                    screens::home::Out::Stay => {}
                    screens::home::Out::Continue => {
                        // Resume the newest autosave — a replay load, so
                        // it cannot desync from its own history.
                        if let Some(fresh) =
                            autosave::latest_compatible().and_then(|path| resume(&path).ok())
                        {
                            tutorial = None;
                            game = keep_flags(fresh, &game);
                            game.paused = args.paused;
                            mode = Mode::Playing;
                            input.reset_session();
                        } else {
                            game.toast("that save no longer loads");
                        }
                    }
                    screens::home::Out::Play => {
                        wizard = Some(Wizard::open(&draft));
                        mode = Mode::Wizard;
                    }
                    screens::home::Out::Tutorial => {
                        // The tutorial is a gentle real match with the
                        // lesson cards riding on top.
                        let fresh = Game::new(tutorial::tutorial_scenario())?;
                        game = keep_flags(fresh, &game);
                        game.paused = args.paused;
                        tutorial = Some(tutorial::Tutorial::new());
                        input.reset_session();
                        mode = Mode::Playing;
                    }
                    screens::home::Out::Replays => {
                        shelf = Some(screens::shelf::Shelf::open());
                        mode = Mode::Replays;
                    }
                    screens::home::Out::Settings => {
                        settings = Some(screens::settings::SettingsScreen::open(&config));
                        mode = Mode::Settings;
                    }
                    screens::home::Out::Quit => match autosave::save(&mut game) {
                        Ok(_) => std::process::exit(0),
                        Err(err) => {
                            // Exiting anyway would be silent data loss:
                            // the failure dialog holds the door.
                            pause = Some(screens::pause::PauseScreen::open_save_failed(
                                err.player_line(),
                                screens::pause::LeaveVerb::Quit,
                                game.state.result().is_some(),
                                can_surrender(&game),
                                true,
                            ));
                            mode = Mode::Pause;
                        }
                    },
                }
                render::draw(&game, &sprites, &input);
                veil();
                home.menu.draw(home.subtitle());
            }
            Mode::Settings => {
                let Some(sc) = settings.as_mut() else {
                    mode = Mode::Home;
                    continue;
                };
                let up = sc.update(
                    &events,
                    &mut input.mouse,
                    &mut game.sounds_pending,
                    &mut config,
                    &mut input.bindings,
                    ctrl_at_frame_start,
                    shift_at_frame_start,
                );
                if up.dirty
                    && let Err(err) = config.save()
                {
                    menu_notice =
                        Some((format!("could not save settings: {err}"), get_time() + 5.0));
                }
                render::draw(&game, &sprites, &input);
                veil();
                let sc = settings.as_ref().expect("still open");
                sc.draw();
                if up.out == screens::settings::Out::Leave {
                    // Back to the origin: Home, or the untouched pause
                    // menu still waiting on its Settings row.
                    let origin = sc.origin;
                    settings = None;
                    mode = match origin {
                        screens::settings::Origin::Home => Mode::Home,
                        screens::settings::Origin::Pause => Mode::Pause,
                    };
                }
            }
            Mode::Wizard => {
                let Some(w) = wizard.as_mut() else {
                    mode = Mode::Home;
                    continue;
                };
                // Wizard trouble — an unreadable map file, a scenario
                // that fails validation — is a dialog problem, never a
                // process abort: report and stay on the menu.
                let out = match w.update(
                    &events,
                    &mut input.mouse,
                    &mut draft,
                    &mut game.sounds_pending,
                ) {
                    Ok(out) => out,
                    Err(err) => {
                        menu_notice =
                            Some((format!("can't open that map: {err:#}"), get_time() + 5.0));
                        WizardOut::Stay
                    }
                };
                match out {
                    WizardOut::Home => {
                        wizard = None;
                        mode = Mode::Home;
                        render::draw(&game, &sprites, &input);
                        veil();
                        home.menu.draw(home.subtitle());
                        continue;
                    }
                    WizardOut::Launch => match launch(&draft) {
                        Ok(fresh) => {
                            tutorial = None;
                            game = keep_flags(fresh, &game);
                            game.paused = args.paused;
                            input.reset_session();
                            wizard = None;
                            mode = Mode::Playing;
                            render::draw(&game, &sprites, &input);
                            continue;
                        }
                        Err(err) => {
                            menu_notice = Some((
                                format!("can't start that match: {err:#}"),
                                get_time() + 5.0,
                            ));
                        }
                    },
                    WizardOut::Stay => {}
                }
                render::draw(&game, &sprites, &input);
                veil();
                let w = wizard.as_ref().expect("still open on Stay");
                match w.step {
                    WizardStep::Map => {
                        w.browser.draw(&w.entries, &mut previews);
                    }
                    WizardStep::Setup => {
                        w.draw_setup(&draft, &mut previews);
                    }
                }
            }
            Mode::Playing => {
                // The tutorial card is chrome: a click on it (the
                // dismiss box included) must never reach the world —
                // it once deselected armies and even placed buildings.
                if let Some(t) = &tutorial {
                    let dismiss = render::tutorial_dismiss_rect();
                    let card = render::tutorial_card_rect(t);
                    if events.iter().any(|e| {
                        matches!(e, RawEvent::MouseDown { button: MouseButton::Left, x, y }
                            if dismiss.contains(vec2(*x, *y)))
                    }) {
                        tutorial = None;
                    }
                    // Card clicks (the dismiss press included) never
                    // reach the world — an armed placement once spent
                    // scrap under the X. Swallowing a RELEASE whose
                    // press began in the world must also end that drag,
                    // or drag_origin sticks and a later release fires a
                    // stale box-select.
                    let swallowed_up = events.iter().any(|e| {
                        matches!(e, RawEvent::MouseUp { x, y, .. }
                            if card.contains(vec2(*x, *y)))
                    });
                    events.retain(|e| {
                        !matches!(e,
                            RawEvent::MouseDown { x, y, .. } | RawEvent::MouseUp { x, y, .. }
                                if card.contains(vec2(*x, *y)))
                    });
                    if swallowed_up {
                        input.drag_origin = None;
                    }
                }
                let had_selection =
                    !game.selection.units.is_empty() || game.selection.building.is_some();
                let escape_pressed = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                input.ui = render::ui_scale();
                input.now = get_time();
                input.camera_prefs = config.camera;
                input.touch_prefs = config.touch;
                input::apply_events(&mut game, &mut input, &events);
                input::update_held(&mut game, &input, dt);
                input::update_touch(&mut game, &mut input);
                // The cursor telegraphs the verb: crosshair while
                // placing or plotting, pointer over chrome.
                macroquad::miniquad::window::set_mouse_cursor(input::desired_cursor(&game, &input));
                // Escape walks outward: deselect first, then the menu —
                // except over a decided match (or the concede overlay),
                // where the banner promises 'Press Esc to continue' and
                // must mean it even with a selection still alive.
                if escape_pressed
                    && (!had_selection || game.state.result().is_some() || game.conceded_banner)
                {
                    // Opening the menu dismisses the concede overlay for
                    // good — Resume from here is clean spectating.
                    game.conceded_banner = false;
                    game.paused = true;
                    game.demo.paused_menu = true;
                    pause = Some(screens::pause::PauseScreen::open(
                        game.state.result().is_some(),
                        can_surrender(&game),
                    ));
                    mode = Mode::Pause;
                }
                if let Some(t) = tutorial.as_mut() {
                    if !t.advance(&game.demo) {
                        tutorial = None;
                    } else {
                        // A click on the card's dismiss box ends school.
                        let dismiss = render::tutorial_dismiss_rect();
                        if events.iter().any(|e| {
                            matches!(e, RawEvent::MouseDown { button: MouseButton::Left, x, y }
                                if dismiss.contains(vec2(*x, *y)))
                        }) {
                            tutorial = None;
                        }
                    }
                }
                game.advance_wall_clock(dt);
                game.update_fx(dt);
                if game.state.result().is_some() && game.end_stats.is_none() {
                    // One re-execution of the record at match end; the
                    // sim replays thousands of ticks per second, so the
                    // hitch hides inside the banner's arrival.
                    let mut replay = game.recorder.clone();
                    let total = game.state.current_tick();
                    replay.meta.ticks = Some(total);
                    game.end_stats = oxide_kit::stats::compute(&replay, (total / 48).max(1)).ok();
                }
                render::draw(&game, &sprites, &input);
                if let Some(t) = &tutorial {
                    render::draw_tutorial(t, &game);
                }
            }
            Mode::Playback => {
                if let Some(pb) = playback.as_mut() {
                    let leave = pb.update(
                        &events,
                        dt,
                        vec2(screen_width(), screen_height()),
                        config.camera.zoom_inverted,
                        config.camera.pan_speed,
                        &mut input.mouse,
                    );
                    if leave {
                        let back_to_pause = pb.from_pause;
                        playback = None;
                        // Opened from a live pause? Return there; the
                        // match is still waiting. Cold --watch or the
                        // shelf goes back Home.
                        if back_to_pause {
                            pause = Some(screens::pause::PauseScreen::open(
                                game.state.result().is_some(),
                                can_surrender(&game),
                            ));
                            mode = Mode::Pause;
                        } else {
                            home = screens::home::HomeScreen::open();
                            mode = Mode::Home;
                        }
                        continue;
                    }
                    render::draw(&pb.game, &sprites, &input);
                    screens::playback::playback_hud(pb, vec2(screen_width(), screen_height()));
                } else {
                    mode = Mode::Home;
                }
            }
            Mode::Replays => {
                let Some(sh) = shelf.as_mut() else {
                    mode = Mode::Home;
                    continue;
                };
                match sh.update(&events, &mut input.mouse, &mut game.sounds_pending) {
                    screens::shelf::Out::Home => {
                        shelf = None;
                        mode = Mode::Home;
                        render::draw(&game, &sprites, &input);
                        veil();
                        home.menu.draw(home.subtitle());
                        continue;
                    }
                    screens::shelf::Out::Watch(path) => {
                        match screens::playback::PlaybackSession::open(&path.to_string_lossy()) {
                            Ok(session) => {
                                playback = Some(session);
                                shelf = None;
                                mode = Mode::Playback;
                                render::draw(&game, &sprites, &input);
                                continue;
                            }
                            Err(_) => {
                                game.sounds_pending.push((SoundKind::Denied, None));
                            }
                        }
                    }
                    screens::shelf::Out::Load(path) => match resume(&path) {
                        // The same loader Continue uses, so the two
                        // verbs cannot drift apart.
                        Ok(fresh) => {
                            tutorial = None;
                            game = keep_flags(fresh, &game);
                            game.paused = args.paused;
                            input.reset_session();
                            shelf = None;
                            mode = Mode::Playing;
                            render::draw(&game, &sprites, &input);
                            continue;
                        }
                        Err(_) => {
                            game.sounds_pending.push((SoundKind::Denied, None));
                        }
                    },
                    screens::shelf::Out::Deleted => {
                        *sh = screens::shelf::Shelf::open();
                        home = screens::home::HomeScreen::open();
                    }
                    screens::shelf::Out::Stay => {}
                }
                render::draw(&game, &sprites, &input);
                veil();
                let sh = shelf.as_ref().expect("still open");
                sh.menu.draw(&sh.subtitle());
            }
            Mode::Pause => {
                let Some(ps) = pause.as_mut() else {
                    mode = Mode::Playing;
                    continue;
                };
                let out = ps.update(&events, &mut input.mouse, &mut game.sounds_pending);
                render::draw(&game, &sprites, &input);
                veil();
                ps.menu.draw(ps.subtitle(&game.scenario.name));
                match out {
                    screens::pause::Out::Stay => {}
                    screens::pause::Out::Resume => {
                        game.paused = false;
                        pause = None;
                        mode = Mode::Playing;
                    }
                    screens::pause::Out::SaveGame => {
                        // Only the session knows its map and tick; the
                        // screen just edits the string.
                        let suggested =
                            format!("{} · t{}", game.scenario.name, game.state.current_tick());
                        if let Some(ps) = pause.as_mut() {
                            ps.begin_naming(suggested);
                        }
                    }
                    screens::pause::Out::Save(name) => {
                        // Stay paused either way: the player may want to
                        // save and then quit.
                        let verdict = match autosave::save_named(&game, &name) {
                            Ok(_) => format!("saved: {name}"),
                            Err(err) => err.player_line(),
                        };
                        if let Some(ps) = pause.as_mut() {
                            ps.end_naming(verdict);
                        }
                    }
                    screens::pause::Out::Settings => {
                        // The pause payload stays put: leaving Settings
                        // lands back on this exact menu, cursor still on
                        // the row that opened it. The sim stays frozen —
                        // neither mode ever advances the wall clock.
                        settings = Some(screens::settings::SettingsScreen::open_from(
                            &config,
                            screens::settings::Origin::Pause,
                        ));
                        mode = Mode::Settings;
                    }
                    screens::pause::Out::Surrender => {
                        // The command lands on the next tick like any
                        // other. A 1v1 decides on the spot and the
                        // normal result flow takes over; in a team game
                        // the concede overlay meets the player back in
                        // the match while the ally plays on.
                        game.issue(oxide_sim::Command::Surrender);
                        game.paused = false;
                        pause = None;
                        mode = Mode::Playing;
                    }
                    screens::pause::Out::WatchReplay => {
                        // The recorder IS the record — clone it, stamp
                        // its length, play it back. Non-destructive; the
                        // live match waits.
                        let mut replay = game.recorder.clone();
                        replay.meta.ticks = Some(game.state.current_tick());
                        match screens::playback::PlaybackSession::from_replay(replay) {
                            Ok(mut session) => {
                                session.from_pause = true;
                                playback = Some(session);
                                pause = None;
                                mode = Mode::Playback;
                            }
                            Err(err) => game.toast(format!("cannot open playback: {err}")),
                        }
                    }
                    screens::pause::Out::Restart => {
                        let fresh = Game::new(game.scenario.clone())?;
                        // Restarting a tutorial restarts the lessons —
                        // discarding them turned it into a plain match.
                        if tutorial.is_some() {
                            tutorial = Some(tutorial::Tutorial::new());
                        }
                        game = keep_flags(fresh, &game);
                        game.paused = args.paused;
                        input.reset_session();
                        pause = None;
                        mode = Mode::Playing;
                    }
                    screens::pause::Out::MainMenu => match autosave::save(&mut game) {
                        Ok(_) => {
                            home = screens::home::HomeScreen::open();
                            pause = None;
                            mode = Mode::Home;
                        }
                        Err(err) => {
                            pause = Some(screens::pause::PauseScreen::open_save_failed(
                                err.player_line(),
                                screens::pause::LeaveVerb::MainMenu,
                                game.state.result().is_some(),
                                can_surrender(&game),
                                false,
                            ));
                        }
                    },
                    screens::pause::Out::Quit => match autosave::save(&mut game) {
                        Ok(_) => std::process::exit(0),
                        Err(err) => {
                            pause = Some(screens::pause::PauseScreen::open_save_failed(
                                err.player_line(),
                                screens::pause::LeaveVerb::Quit,
                                game.state.result().is_some(),
                                can_surrender(&game),
                                false,
                            ));
                        }
                    },
                    screens::pause::Out::RetrySave(verb) => match autosave::save(&mut game) {
                        Ok(_) => match verb {
                            screens::pause::LeaveVerb::MainMenu => {
                                home = screens::home::HomeScreen::open();
                                pause = None;
                                mode = Mode::Home;
                            }
                            screens::pause::LeaveVerb::Quit => std::process::exit(0),
                        },
                        Err(err) => {
                            // The dialog stays up; only the reason may
                            // have changed.
                            if let Some(ps) = pause.as_mut() {
                                ps.set_save_failure_line(err.player_line());
                            }
                        }
                    },
                    screens::pause::Out::LeaveUnsaved(verb) => match verb {
                        screens::pause::LeaveVerb::MainMenu => {
                            home = screens::home::HomeScreen::open();
                            pause = None;
                            mode = Mode::Home;
                        }
                        screens::pause::LeaveVerb::Quit => std::process::exit(0),
                    },
                    screens::pause::Out::Home => {
                        home = screens::home::HomeScreen::open();
                        pause = None;
                        mode = Mode::Home;
                    }
                }
            }
        }

        // The menu-context error line draws over whichever menu is up;
        // the gameplay modes speak through the HUD's toast strip.
        if !matches!(mode, Mode::Playing | Mode::Playback)
            && let Some((msg, until)) = &menu_notice
        {
            if get_time() < *until {
                let s = render::ui_scale();
                let width = measure_text(msg, None, (16.0 * s) as u16, 1.0).width;
                draw_text(
                    msg,
                    (screen_width() - width) * 0.5,
                    screen_height() - 48.0 * s,
                    16.0 * s,
                    theme::TEXT_DANGER,
                );
            } else {
                menu_notice = None;
            }
        }

        if std::mem::discriminant(&mode) != mode_before {
            input.reset_transient();
        }
        ui_view = capture_ui(
            &mode, &home, &wizard, &draft, &shelf, &pause, &settings, &game,
        );

        // The mixer serves whichever session is on screen: a playback
        // viewer queues its own sounds on its own game, and draining the
        // hidden match instead left replays silent while its queue grew.
        let (queued, cam_center, cam_half_w): (Vec<(SoundKind, Option<Vec2>)>, Vec2, f32) =
            match (&mode, playback.as_mut()) {
                (Mode::Playback, Some(pb)) => (
                    pb.game.sounds_pending.drain(..).collect(),
                    pb.game.camera.center,
                    pb.game.camera.viewport().x / pb.game.camera.zoom * 0.5,
                ),
                _ => (
                    game.sounds_pending.drain(..).collect(),
                    game.camera.center,
                    game.camera.viewport().x / game.camera.zoom * 0.5,
                ),
            };
        for (kind, world) in queued {
            // Distance dims the battlefield: full volume on screen,
            // fading to a quarter around 1.5 viewports out. Unpositioned
            // sounds (UI, own milestones) play flat.
            let attenuation = world.map_or(1.0, |p| {
                let center = cam_center;
                let half_w = cam_half_w;
                let d = (p - center).length();
                if d <= half_w {
                    1.0
                } else {
                    (1.0 - (d - half_w) / (2.0 * half_w)).clamp(0.25, 1.0)
                }
            });
            mixer.play(&sounds, kind, &config.volumes, attenuation);
        }

        if !pending_shots.is_empty() {
            // One readback serves every request that arrived this frame.
            let image = get_screen_data();
            for shot in pending_shots.drain(..) {
                let response = match write_png(&image, &shot.path) {
                    Ok((width, height)) => ResponseEnvelope::ok(
                        shot.id,
                        Reply::Screenshot(ScreenshotView {
                            path: shot.path,
                            width,
                            height,
                        }),
                    ),
                    Err(err) => ResponseEnvelope::err(shot.id, format!("screenshot: {err:#}")),
                };
                shot.reply.send(response).ok();
            }
        }
        // Persist the window size once it has settled: the config
        // documents window persistence, and only settings writes ever
        // saved it before.
        let live = (screen_width() as u32, screen_height() as u32);
        // Explicit --window runs (the UX matrix) and automation must
        // not overwrite the human's remembered size.
        let persist_size = args.window.is_none() && !args.automation;
        if persist_size && live.0 >= 640 && live.1 >= 400 && live != config.window {
            match pending_size {
                Some((size, since)) if size == live => {
                    if get_time() - since > 1.0 {
                        config.window = live;
                        if let Err(err) = config.save() {
                            let line = format!("could not save settings: {err}");
                            if matches!(mode, Mode::Playing) {
                                game.toast(line);
                            } else {
                                menu_notice = Some((line, get_time() + 5.0));
                            }
                        }
                        pending_size = None;
                    }
                }
                _ => pending_size = Some((live, get_time())),
            }
        } else {
            pending_size = None;
        }

        if is_quit_requested() {
            // Everything a quit must not lose: any settled-but-unwritten
            // window size, and the live session as an autosave. A failed
            // autosave swallows the quit (prevent_quit is in force) and
            // raises the failure dialog instead of exiting over data loss.
            config.save().ok();
            match autosave::save(&mut game) {
                Ok(_) => std::process::exit(0),
                Err(err) => {
                    game.paused = true;
                    // The dialog's home-vs-match classification must
                    // see THROUGH screens opened from Pause: a quit
                    // while Settings or Playback sits over a paused
                    // match still has an unsaved match behind it, and
                    // a Home-classified Cancel would strand it with no
                    // route back.
                    let over_a_match = matches!(mode, Mode::Playing | Mode::Pause)
                        || settings
                            .as_ref()
                            .is_some_and(|s| s.origin == screens::settings::Origin::Pause)
                        || playback.as_ref().is_some_and(|p| p.from_pause);
                    pause = Some(screens::pause::PauseScreen::open_save_failed(
                        err.player_line(),
                        screens::pause::LeaveVerb::Quit,
                        game.state.result().is_some(),
                        can_surrender(&game),
                        !over_a_match,
                    ));
                    settings = None;
                    playback = None;
                    mode = Mode::Pause;
                }
            }
        }

        next_frame().await;
    }
}

/// Loads a record back into a live session — the one loader behind both
/// Home's Continue and the shelf's Load, so the two verbs cannot drift.
/// A resume IS a replay load; validation and the tick-count cap live in
/// [`Game::from_replay`].
fn resume(path: &std::path::Path) -> Result<Game> {
    let replay =
        GameReplay::load(path).with_context(|| format!("loading record {}", path.display()))?;
    Game::from_replay(replay)
}

/// Carries session-level toggles (pause/speed/overlay) onto a fresh game.
/// Whether the human's seat can still concede: it holds a Foundry and
/// has not already resigned — the Surrender row's gate, matching the
/// sim's own command gate so the menu never offers a verb the sim
/// would only reject.
fn can_surrender(game: &Game) -> bool {
    !game.state.player(game.human).resigned && game.home_foundry().is_some()
}

fn keep_flags(mut fresh: Game, old: &Game) -> Game {
    fresh.paused = old.paused;
    fresh.speed = old.speed;
    fresh.overlay = old.overlay;
    fresh
}

#[allow(clippy::too_many_arguments)]
fn capture_ui(
    mode: &Mode,
    home: &screens::home::HomeScreen,
    wizard: &Option<Wizard>,
    draft: &NewMatchDraft,
    shelf: &Option<screens::shelf::Shelf>,
    pause: &Option<screens::pause::PauseScreen>,
    settings: &Option<screens::settings::SettingsScreen>,
    game: &Game,
) -> UiView {
    let (mode_name, menu) = match mode {
        Mode::Home => ("home", Some(&home.menu)),
        Mode::Settings => (
            settings.as_ref().map_or("settings", |s| s.mode_name()),
            settings.as_ref().map(|s| &s.menu),
        ),
        // Both wizard screens are custom (grid, setup) and report
        // through ui_surface below — no row menu to borrow.
        Mode::Wizard => (wizard.as_ref().map_or("main_menu", |w| w.mode_name()), None),
        Mode::Playing => ("playing", None),
        Mode::Playback => ("playback", None),
        Mode::Replays => ("replays", shelf.as_ref().map(|s| &s.menu)),
        Mode::Pause => (
            if pause.as_ref().is_some_and(|p| p.saving_failed()) {
                "save_failed"
            } else if pause.as_ref().is_some_and(|p| p.naming()) {
                "save_name"
            } else if pause.as_ref().is_some_and(|p| p.confirming()) {
                "confirm_pause"
            } else {
                "pause_menu"
            },
            pause.as_ref().map(|p| &p.menu),
        ),
    };
    // The wizard's custom screens (grid, setup) speak the same
    // protocol surface the row menus do — automation keeps its
    // footing across redesigns.
    if let (Mode::Wizard, Some(w)) = (mode, wizard.as_ref()) {
        let (title, items, selected) = w.ui_surface(draft);
        // The frame injected this viewport before drawing, so the
        // range reports the grid window the player is actually seeing.
        let visible = w.ui_visible_range(draft, render::viewport(), render::ui_scale());
        return UiView {
            mode: w.mode_name().to_string(),
            title: Some(title),
            selected: Some(selected),
            items,
            visible_range: Some(visible),
            hover: w.ui_hover(),
            chrome: None,
        };
    }
    UiView {
        mode: mode_name.to_string(),
        title: menu.map(|menu| menu.title.clone()),
        selected: menu.map(|menu| menu.selected),
        items: menu.map_or_else(Vec::new, |menu| menu.items.clone()),
        visible_range: menu.map(Menu::visible_range),
        hover: menu.and_then(Menu::hover),
        chrome: matches!(mode, Mode::Playing).then(|| {
            let l = game.layout.get();
            let m = l.minimap;
            // JSON has no Infinity: an absent panel reports the window
            // bottom, a band no click can land in.
            let panel_top = if l.panel_top.is_finite() {
                l.panel_top
            } else {
                screen_height()
            };
            let o = l.orders;
            [
                l.top_bar_h,
                panel_top,
                m.x,
                m.y,
                m.w,
                m.h,
                l.panel_right,
                o.x,
                o.y,
                o.w,
                o.h,
            ]
        }),
    }
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

/// Writes a captured frame as PNG with real error handling — macroquad's
/// own `export_png` unwraps on failure, which would let one malformed
/// debug-socket path abort the entire session.
fn write_png(image: &Image, path: &str) -> Result<(u32, u32)> {
    if let Some(parent) = std::path::Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path).with_context(|| format!("creating {path}"))?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        u32::from(image.width),
        u32::from(image.height),
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().context("writing png header")?;
    // The GL framebuffer is bottom-up; PNG rows are top-down. Skipping this
    // flip shipped upside-down screenshots once already.
    let stride = usize::from(image.width) * 4;
    let mut flipped = Vec::with_capacity(image.bytes.len());
    for row in image.bytes.chunks_exact(stride).rev() {
        flipped.extend_from_slice(row);
    }
    writer
        .write_image_data(&flipped)
        .context("writing png data")?;
    Ok((u32::from(image.width), u32::from(image.height)))
}

/// Answers one debug request. Screenshots are parked; everything else
/// responds immediately, between frames, against a settled world.
#[allow(clippy::too_many_arguments)]
fn handle_request(
    incoming: IncomingRequest,
    game: &mut Game,
    mode: &mut Mode,
    input: &mut input::InputState,
    injected: &mut Vec<RawEvent>,
    pending_shots: &mut Vec<PendingScreenshot>,
    ui_view: &UiView,
    tutorial: &mut Option<tutorial::Tutorial>,
    playback: &mut Option<screens::playback::PlaybackSession>,
    settings: &mut Option<screens::settings::SettingsScreen>,
) {
    let IncomingRequest { id, request, reply } = incoming;
    // A viewer session owns the screen: state-shaped questions and the
    // driven clock answer for the REPLAYED world, not the hidden match
    // behind it.
    if let Some(pb) = playback {
        if pb.game.state.current_tick() != pb.engine.position() {
            pb.game.state = pb.engine.state.clone();
        }
        let handled: Option<Result<Reply, String>> = match &request {
            Request::Status => {
                // The clock the caller cares about is the transport's,
                // not the hidden Game's disconnected fields.
                let mut view = status_view(&pb.game);
                view.paused = pb.paused;
                view.speed = f64::from(pb.speed);
                Some(Ok(Reply::Status(view)))
            }
            Request::QueryState { filter } => Some(Ok(Reply::State(StateView::capture(
                &pb.game.state,
                *filter,
            )))),
            Request::StateHash => Some(Ok(Reply::Hash(HashView {
                tick: pb.game.state.current_tick(),
                hash: oxide_protocol::hash_hex(pb.game.state.hash()),
            }))),
            Request::AdvanceTicks { ticks } => {
                // Same cap as the live clock, and the reply reports what
                // actually ran — a replay near its end advances less
                // than asked. Seek, don't advance: advance collects the
                // interval's events for presentation, and a million-tick
                // battle's worth of them is memory nobody will hear.
                let requested = (*ticks).min(oxide_protocol::MAX_ADVANCE_TICKS);
                // An external transport op replaces any UI seek in
                // flight: left pending, the stale target resumes next
                // frame and rewinds the replay this reply just reported
                // as advanced.
                pb.seeking = None;
                let before = pb.engine.position();
                pb.engine.seek(before.saturating_add(requested));
                pb.game.drop_presentation();
                pb.game.state = pb.engine.state.clone();
                Some(Ok(Reply::Advanced(AdvancedView {
                    ticks: pb.engine.position() - before,
                    tick: pb.game.state.current_tick(),
                    hash: oxide_protocol::hash_hex(pb.game.state.hash()),
                })))
            }
            Request::PresentTicks { ticks } => {
                let requested = (*ticks).min(oxide_protocol::MAX_PRESENT_TICKS);
                pb.seeking = None;
                let before = pb.engine.position();
                let mut events = Vec::new();
                for _ in 0..requested {
                    if pb.engine.at_end() {
                        break;
                    }
                    // Mirror Game::present_ticks: the previous tick's
                    // transients age by one sim interval, while effects
                    // emitted by the newest tick stay fresh.
                    pb.game.update_fx(game::TICK_DT);
                    let tick_events = pb.engine.advance(1);
                    pb.game.playback_present(&pb.engine.state, &tick_events);
                    events.extend(tick_events);
                }
                Some(Ok(Reply::Presented(PresentedView {
                    ticks: pb.engine.position() - before,
                    tick: pb.game.state.current_tick(),
                    hash: oxide_protocol::hash_hex(pb.game.state.hash()),
                    events,
                })))
            }
            Request::Pause => {
                pb.paused = true;
                Some(Ok(Reply::Ok))
            }
            Request::Resume => {
                pb.paused = false;
                Some(Ok(Reply::Ok))
            }
            Request::SetSpeed { multiplier } => {
                if multiplier.is_finite() && (0.05..=64.0).contains(multiplier) {
                    pb.speed = *multiplier as f32;
                    Some(Ok(Reply::Ok))
                } else {
                    Some(Err(format!("speed multiplier {multiplier} out of range")))
                }
            }
            Request::QueryCamera => {
                let (lo, hi) = pb.game.camera.world_rect();
                Some(Ok(Reply::Camera(CameraView {
                    center: [
                        f64::from(pb.game.camera.center.x),
                        f64::from(pb.game.camera.center.y),
                    ],
                    zoom: f64::from(pb.game.camera.zoom),
                    viewport: [f64::from(screen_width()), f64::from(screen_height())],
                    world_rect: [
                        f64::from(lo.x),
                        f64::from(lo.y),
                        f64::from(hi.x),
                        f64::from(hi.y),
                    ],
                })))
            }
            Request::ToggleOverlay => {
                pb.game.overlay = !pb.game.overlay;
                Some(Ok(Reply::Overlay(OverlayView {
                    enabled: pb.game.overlay,
                })))
            }
            // The viewer is read-only: refusing beats acknowledging a
            // request that would silently mutate the hidden match.
            Request::SendCommand { .. }
            | Request::LoadScenario { .. }
            | Request::LoadReplay { .. }
            | Request::SaveReplay { .. } => Some(Err(
                "the viewer is read-only; leave playback first".to_string(),
            )),
            _ => None,
        };
        if let Some(outcome) = handled {
            let envelope = match outcome {
                Ok(inner) => ResponseEnvelope::ok(id, inner),
                Err(error) => ResponseEnvelope::err(id, error),
            };
            let _ = reply.send(envelope);
            return;
        }
    }
    let outcome: Result<Reply, String> = match request {
        Request::Status => Ok(Reply::Status(status_view(game))),
        Request::QueryState { filter } => Ok(Reply::State(StateView::capture(&game.state, filter))),
        Request::QueryCamera => {
            let (lo, hi) = game.camera.world_rect();
            Ok(Reply::Camera(CameraView {
                center: [
                    f64::from(game.camera.center.x),
                    f64::from(game.camera.center.y),
                ],
                zoom: f64::from(game.camera.zoom),
                viewport: [f64::from(screen_width()), f64::from(screen_height())],
                world_rect: [
                    f64::from(lo.x),
                    f64::from(lo.y),
                    f64::from(hi.x),
                    f64::from(hi.y),
                ],
            }))
        }
        Request::QueryUi => Ok(Reply::Ui(ui_view.clone())),
        Request::StateHash => Ok(Reply::Hash(HashView {
            tick: game.state.current_tick(),
            hash: game.hash_hex(),
        })),
        Request::AdvanceTicks { ticks } => {
            let ticks = ticks.min(oxide_protocol::MAX_ADVANCE_TICKS);
            game.advance_ticks(ticks);
            Ok(Reply::Advanced(AdvancedView {
                ticks,
                tick: game.state.current_tick(),
                hash: game.hash_hex(),
            }))
        }
        Request::PresentTicks { ticks } => {
            let ticks = ticks.min(oxide_protocol::MAX_PRESENT_TICKS);
            let events = game.present_ticks(ticks);
            Ok(Reply::Presented(PresentedView {
                ticks,
                tick: game.state.current_tick(),
                hash: game.hash_hex(),
                events,
            }))
        }
        Request::Pause => {
            game.paused = true;
            Ok(Reply::Ok)
        }
        Request::Resume => {
            game.paused = false;
            // Resuming implies gameplay: leave the pause menu too, or the
            // sim runs behind a menu that still claims it is paused —
            // Settings opened over a paused match included, or the sim
            // would run under a screen that never ticks it.
            let settings_over_match = matches!(mode, Mode::Settings)
                && settings
                    .as_ref()
                    .is_some_and(|s| s.origin == screens::settings::Origin::Pause);
            if matches!(mode, Mode::Pause) || settings_over_match {
                *settings = None;
                *mode = Mode::Playing;
                input.reset_transient();
            }
            Ok(Reply::Ok)
        }
        Request::SetSpeed { multiplier } => {
            if multiplier.is_finite() && (0.05..=64.0).contains(&multiplier) {
                game.speed = multiplier;
                Ok(Reply::Ok)
            } else {
                Err(format!("speed multiplier {multiplier} outside 0.05..=64"))
            }
        }
        Request::SendCommand { player, command } => {
            if (player.0 as usize) < game.state.players().len() {
                game.pending.push(PlayerCommand { player, command });
                Ok(Reply::Ok)
            } else {
                Err(format!("no such player {player}"))
            }
        }
        Request::InjectEvent { event } => {
            // The hardware funnel admits only printable ASCII into Text
            // (input.rs char_event); injected events walk the identical
            // path, so they honor the identical contract — a control
            // byte or non-ASCII char is refused, not persisted into a
            // save name the font cannot draw.
            if let oxide_protocol::RawEvent::Text { ch } = event
                && !('\u{20}'..='\u{7e}').contains(&ch)
            {
                Err(format!(
                    "text event {ch:?} outside printable ASCII — the funnel refuses it"
                ))
            } else {
                injected.push(event);
                Ok(Reply::Ok)
            }
        }
        Request::Screenshot { path } => {
            let path = path
                .unwrap_or_else(|| format!("screenshots/tick-{}.png", game.state.current_tick()));
            pending_shots.push(PendingScreenshot { id, path, reply });
            return; // responds after the frame renders
        }
        Request::ToggleOverlay => {
            game.overlay = !game.overlay;
            Ok(Reply::Overlay(OverlayView {
                enabled: game.overlay,
            }))
        }
        Request::LoadScenario { path } => Scenario::load(&path)
            .map_err(|err| format!("loading {path}: {err}"))
            .and_then(|scenario| {
                Game::new(scenario).map_err(|err| format!("building scenario: {err:#}"))
            })
            .map(|fresh| {
                *tutorial = None;
                *game = keep_flags(fresh, game);
                *mode = Mode::Playing;
                input.reset_session();
                Reply::Ok
            }),
        Request::LoadReplay { path } => GameReplay::load(&path)
            .map_err(|err| format!("loading replay {path}: {err}"))
            .and_then(|replay| {
                Game::from_replay(replay).map_err(|err| format!("resuming replay: {err:#}"))
            })
            .map(|fresh| {
                *tutorial = None;
                *game = keep_flags(fresh, game);
                *mode = Mode::Playing;
                input.reset_session();
                Reply::Status(status_view(game))
            }),
        Request::SaveReplay { path } => {
            game.recorder.meta.ticks = Some(game.state.current_tick());
            let parent = std::path::Path::new(&path).parent();
            if let Some(parent) = parent
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).ok();
            }
            match game.recorder.save(&path) {
                Ok(()) => Ok(Reply::Saved(SavedView {
                    path,
                    commands: game.recorder.commands.len(),
                })),
                Err(err) => Err(format!("saving replay: {err}")),
            }
        }
    };
    let response = match outcome {
        Ok(ok) => ResponseEnvelope::ok(id, ok),
        Err(err) => ResponseEnvelope::err(id, err),
    };
    reply.send(response).ok();
}

fn status_view(game: &Game) -> StatusView {
    StatusView {
        tick: game.state.current_tick(),
        paused: game.paused,
        speed: game.speed,
        scenario: game.scenario.name.clone(),
        sim_version: SIM_VERSION.to_string(),
        result: game.state.result(),
        recorded_commands: game.recorder.commands.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team_draft() -> NewMatchDraft {
        let mut draft = NewMatchDraft::default();
        let scenario = Scenario::load("../scenarios/trident-plateau.json").expect("shipped map");
        draft.set_scenario(scenario, None);
        draft
    }

    #[test]
    fn launch_reads_only_the_per_seat_vector() {
        let mut draft = team_draft();
        draft.seat_choice = 2;
        draft.seats[0].level_choice = 3; // Expert
        draft.seats[0].personality_choice = 1; // Turtle
        let game = launch(&draft).expect("launches");
        let players = &game.scenario.players;
        assert!(!players[2].bot, "the chosen chair is the human's");
        assert_eq!(game.human, oxide_sim::PlayerId(2));
        for (i, p) in players.iter().enumerate() {
            if i == 2 {
                assert!(p.bot_config.is_none());
                continue;
            }
            assert!(p.bot, "every other seat is a bot");
            let config = p.bot_config.expect("every bot seat has a config");
            if i == 0 {
                assert_eq!(config.level, oxide_sim::bot::Level::Expert);
                assert_eq!(config.aggression, Some(100), "the seat's OWN dials");
            } else {
                assert_eq!(config.level, oxide_sim::bot::Level::Medium);
            }
        }
        // Auto chips keep the seat's authored faction.
        assert_eq!(players[2].faction, oxide_sim::Faction::Ferrous);
    }

    #[test]
    fn a_zero_seat_map_refuses_to_launch_instead_of_panicking() {
        // Discovery lists any parseable JSON; a players: [] file used
        // to underflow the seat clamp.
        let mut scenario = Scenario::skirmish();
        scenario.players.clear();
        scenario.units.clear();
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(scenario, None);
        assert!(
            launch(&draft).is_err(),
            "an empty seat list is a launch error, not a crash"
        );
    }

    #[test]
    fn a_faction_chip_retints_the_seat_and_collided_names_take_ordinals() {
        let mut draft = NewMatchDraft::default();
        let scenario = Scenario::load("../scenarios/gatework-array.json").expect("shipped map");
        draft.set_scenario(scenario, None);
        // "North West Cupric" flips Ferrous — its retinted label
        // collides with seat 0's "North West Ferrous".
        draft.seats[1].faction_choice = 1;
        let game = launch(&draft).expect("a legitimate faction choice never refuses to launch");
        let players = &game.scenario.players;
        assert_eq!(players[1].faction, oxide_sim::Faction::Ferrous);
        assert_eq!(
            players[1].name, "North West Ferrous 2",
            "the duplicate label took an ordinal"
        );
        let mut names: Vec<&str> = players.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), players.len(), "every banner name stays unique");
    }

    #[test]
    fn a_duel_chip_override_retints_only_its_seat() {
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(Scenario::skirmish(), None);
        draft.seats[0].faction_choice = 2; // the human goes Cupric
        let game = launch(&draft).expect("launches");
        let players = &game.scenario.players;
        assert_eq!(players[0].faction, oxide_sim::Faction::Cupric);
        assert_eq!(
            players[1].faction,
            oxide_sim::Faction::Cupric,
            "Auto keeps the authored roster - the mirror the quick flow forbade"
        );
        assert_ne!(
            players[0].name, players[1].name,
            "ordinals keep names unique"
        );
    }

    #[test]
    fn a_stale_draft_fails_the_launch_instead_of_the_process() {
        // The caller shows launch errors on a menu notice; the fn's
        // contract is Err, never panic, on a draft out of step.
        let mut draft = team_draft();
        draft.seats.truncate(2);
        assert!(launch(&draft).is_err());
    }

    #[test]
    fn automation_requires_debug_server() {
        assert!(Args::try_parse_from(["oxide-shell", "--automation"]).is_err());
        let args = Args::try_parse_from(["oxide-shell", "--debug-server", "--automation"])
            .expect("automation with the debug server should parse");
        assert!(args.debug_server);
        assert!(args.automation);
    }

    #[test]
    fn trace_env_never_reaches_an_automation_shell() {
        // Harness-spawned shells inherit the developer's environment;
        // only the explicit flag may trace under --automation.
        assert!(!trace_active(false, true, true));
        assert!(trace_active(true, true, false));
        assert!(trace_active(false, false, true));
        assert!(!trace_active(false, false, false));
    }
}
