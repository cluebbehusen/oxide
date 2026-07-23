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
mod render;
mod saves;
mod screens;
mod tutorial;

use anyhow::{Context, Result};
use clap::Parser;
use debug_server::IncomingRequest;
use game::{Game, GameReplay, SoundKind};
use macroquad::audio::{PlaySoundParams, play_sound};
use macroquad::prelude::*;
use menu::{Menu, PreviewCache};
use oxide_protocol::{
    AdvancedView, CameraView, HashView, Key, MouseButton, OverlayView, RawEvent, Reply, Request,
    ResponseEnvelope, SavedView, ScreenshotView, StateView, StatusView, UiView,
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

/// Everything a quit must not lose: the live session (as an autosave)
/// and any settled-but-unwritten window size.
fn save_on_quit(game: &mut Game, config: &config::Config) {
    autosave::save(game);
    config.save().ok();
}

fn window_conf() -> Conf {
    // The window is created before `run()` ever sees clap's output, so
    // the size/DPI flags are parsed here too — clap is idempotent and
    // errors surface identically on the second parse in `run()`.
    let args = Args::parse();
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
    /// The New Match wizard (map, difficulty, personality, faction);
    /// its state lives in the `wizard` session local.
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
    let level = oxide_sim::bot::Level::LADDER[draft.level_choice.min(3)];
    let aggression = screens::wizard::personality_knob(draft.personality_choice);
    let config = oxide_sim::scenario::BotConfig { level, aggression };
    for player in scenario.players.iter_mut().filter(|p| p.bot) {
        player.bot_config = Some(config);
    }
    // The human seat plays the chosen roster; "surprise" lets the
    // scenario seed pick.
    let faction = match draft.faction_choice {
        0 => oxide_sim::Faction::Ferrous,
        1 => oxide_sim::Faction::Cupric,
        _ => match scenario.seed % 2 {
            0 => oxide_sim::Faction::Ferrous,
            _ => oxide_sim::Faction::Cupric,
        },
    };
    let complement = match faction {
        oxide_sim::Faction::Ferrous => oxide_sim::Faction::Cupric,
        oxide_sim::Faction::Cupric => oxide_sim::Faction::Ferrous,
    };
    if let Some(human) = scenario.players.iter_mut().find(|p| !p.bot) {
        retint_seat(human, faction);
    }
    // In a duel, faction is also the only allegiance cue on screen —
    // the opponent takes the other roster, or two same-color armies
    // would fight an unreadable war. Team maps author their own mixed
    // factions and carry an explicit ally marker instead.
    if scenario.players.len() == 2
        && let Some(bot) = scenario.players.iter_mut().find(|p| p.bot)
    {
        retint_seat(bot, complement);
    }
    Game::new(scenario)
}

/// Moves a seat onto a roster, keeping any faction-derived name honest:
/// the shipped maps name seats after their faction ("Cupric", "West
/// Ferrous"), and a stale name makes the victory banner announce the
/// wrong side.
fn retint_seat(seat: &mut oxide_sim::scenario::PlayerSpec, faction: oxide_sim::Faction) {
    let label = |f: oxide_sim::Faction| match f {
        oxide_sim::Faction::Ferrous => "Ferrous",
        oxide_sim::Faction::Cupric => "Cupric",
    };
    if seat.faction != faction {
        seat.name = seat.name.replace(label(seat.faction), label(faction));
        seat.faction = faction;
    }
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
    let mut config = config::Config::load();
    render::set_user_scale(config.ui_scale);
    render::set_reduced_motion(config.reduced_motion);
    let sprites = assets::Sprites::load().await?;
    let sounds = assets::Sounds::load().await?;
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

    // Launched for a purpose (a scenario, a resume, or an agent socket)?
    // Straight into the game. Everyone else — automation and humans
    // alike — starts cold at the Home front door.
    let mut home = screens::home::HomeScreen::open();
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
    let mut previews = PreviewCache::default();
    let mut wizard: Option<Wizard> = None;
    let mut pause: Option<screens::pause::PauseScreen> = None;
    let mut settings: Option<screens::settings::SettingsScreen> = None;

    // The title-bar close and Cmd-Q must reach the autosave path: left
    // to macroquad they exit the process before any save runs.
    prevent_quit();

    let debug_rx: Option<Receiver<IncomingRequest>> = if args.debug_server {
        Some(debug_server::spawn(args.port)?)
    } else {
        None
    };
    let mut input = input::InputState::new();
    let mut injected: Vec<RawEvent> = Vec::new();
    let mut pending_shots: Vec<PendingScreenshot> = Vec::new();
    let mut ui_view = capture_ui(&mode, &home, &wizard, &shelf, &pause, &settings, &game);

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
                );
                if holds_queries {
                    break;
                }
            }
        }

        let mut events = if args.automation {
            Vec::new()
        } else {
            input::poll_events(&mut input)
        };
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
                match home.update(&events, &mut input.mouse, &mut game.sounds_pending) {
                    screens::home::Out::Stay => {}
                    screens::home::Out::Continue => {
                        // Resume the newest autosave — a replay load, so
                        // it cannot desync from its own history.
                        if let Some(path) = autosave::latest_compatible()
                            && let Ok(replay) = GameReplay::load(&path)
                            && let Ok(fresh) = Game::from_replay(replay)
                        {
                            tutorial = None;
                            game = keep_flags(fresh, &game);
                            game.paused = false;
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
                        let mut scenario = Scenario::skirmish();
                        for p in scenario.players.iter_mut().skip(1) {
                            p.bot_config = Some(oxide_sim::scenario::BotConfig {
                                level: oxide_sim::bot::Level::Easy,
                                aggression: Some(0),
                            });
                        }
                        let fresh = Game::new(scenario)?;
                        game = keep_flags(fresh, &game);
                        game.paused = false;
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
                    screens::home::Out::Quit => {
                        autosave::save(&mut game);
                        std::process::exit(0);
                    }
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
                if up.dirty {
                    config.save().ok();
                }
                if let Some(text) = up.toast {
                    game.toast(text);
                }
                render::draw(&game, &sprites, &input);
                veil();
                let sc = settings.as_ref().expect("still open");
                sc.menu.draw(sc.hint());
                if up.out == screens::settings::Out::Home {
                    settings = None;
                    mode = Mode::Home;
                }
            }
            Mode::Wizard => {
                let Some(w) = wizard.as_mut() else {
                    mode = Mode::Home;
                    continue;
                };
                match w.update(
                    &events,
                    &mut input.mouse,
                    &mut draft,
                    &mut game.sounds_pending,
                )? {
                    WizardOut::Home => {
                        wizard = None;
                        mode = Mode::Home;
                        render::draw(&game, &sprites, &input);
                        veil();
                        home.menu.draw(home.subtitle());
                        continue;
                    }
                    WizardOut::Launch => {
                        let fresh = launch(&draft)?;
                        tutorial = None;
                        game = keep_flags(fresh, &game);
                        game.paused = false;
                        input.reset_session();
                        wizard = None;
                        mode = Mode::Playing;
                        render::draw(&game, &sprites, &input);
                        continue;
                    }
                    WizardOut::Stay => {}
                }
                render::draw(&game, &sprites, &input);
                veil();
                let w = wizard.as_ref().expect("still open on Stay");
                if w.step == WizardStep::Map {
                    // The subtitle browses with the player: the
                    // highlighted map's hook and badges, the pointer's
                    // row winning over the keyboard cursor.
                    let focus = w.menu.hover().unwrap_or(w.menu.selected);
                    let subtitle = w
                        .entries
                        .get(focus)
                        .and_then(|e| e.blurb.as_deref())
                        .unwrap_or("machines eating a dead world");
                    w.menu.draw(subtitle);
                    // Fog-free preview of the highlighted map, softly
                    // panelled on the right.
                    if let Some(entry) = w.entries.get(focus)
                        && let Some(tex) = previews.get(focus, entry)
                    {
                        let s = render::ui_scale();
                        // Strictly right of the menu's own row rects —
                        // shared geometry, no independent arithmetic to
                        // drift out of sync. Too narrow? No panel.
                        let left_bound = w.menu.rows_right_edge() + 24.0 * s;
                        let avail = screen_width() - left_bound - 24.0 * s;
                        let max_w = avail.min(screen_width() * 0.26);
                        let max_h = screen_height() * 0.34;
                        if max_w >= 96.0 * s {
                            let scale = (max_w / tex.width()).min(max_h / tex.height());
                            let (pw, ph) = (tex.width() * scale, tex.height() * scale);
                            let x = screen_width() - pw - 24.0 * s;
                            let y = screen_height() * 0.5 - ph * 0.5;
                            draw_rectangle(
                                x - 8.0 * s,
                                y - 8.0 * s,
                                pw + 16.0 * s,
                                ph + 16.0 * s,
                                Color::from_rgba(20, 20, 24, 230),
                            );
                            draw_texture_ex(
                                tex,
                                x,
                                y,
                                render::theme_tint(&entry.theme),
                                DrawTextureParams {
                                    dest_size: Some(vec2(pw, ph)),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                } else {
                    w.menu.draw(w.subtitle(&draft));
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
                input::apply_events(&mut game, &mut input, &events);
                input::update_held(&mut game, &input, dt);
                // Escape walks outward: deselect first, then the menu —
                // except over a decided match, where the banner promises
                // 'Press Esc to continue' and must mean it even with a
                // selection still alive.
                if escape_pressed && (!had_selection || game.state.result().is_some()) {
                    game.paused = true;
                    game.demo.paused_menu = true;
                    pause = Some(screens::pause::PauseScreen::open(
                        game.state.result().is_some(),
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
                    render::draw_tutorial(t);
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
                            ));
                            mode = Mode::Pause;
                        } else {
                            home = screens::home::HomeScreen::open();
                            mode = Mode::Home;
                        }
                        continue;
                    }
                    render::draw(&pb.game, &sprites, &input);
                    screens::playback::playback_hud(pb);
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
                        game.paused = false;
                        input.reset_session();
                        pause = None;
                        mode = Mode::Playing;
                    }
                    screens::pause::Out::MainMenu => {
                        autosave::save(&mut game);
                        home = screens::home::HomeScreen::open();
                        pause = None;
                        mode = Mode::Home;
                    }
                    screens::pause::Out::Quit => {
                        autosave::save(&mut game);
                        std::process::exit(0);
                    }
                }
            }
        }

        if std::mem::discriminant(&mode) != mode_before {
            input.reset_transient();
        }
        ui_view = capture_ui(&mode, &home, &wizard, &shelf, &pause, &settings, &game);

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
                        config.save().ok();
                        pending_size = None;
                    }
                }
                _ => pending_size = Some((live, get_time())),
            }
        } else {
            pending_size = None;
        }

        if is_quit_requested() {
            save_on_quit(&mut game, &config);
            std::process::exit(0);
        }

        next_frame().await;
    }
}

/// Carries session-level toggles (pause/speed/overlay) onto a fresh game.
fn keep_flags(mut fresh: Game, old: &Game) -> Game {
    fresh.paused = old.paused;
    fresh.speed = old.speed;
    fresh.overlay = old.overlay;
    fresh
}

fn capture_ui(
    mode: &Mode,
    home: &screens::home::HomeScreen,
    wizard: &Option<Wizard>,
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
        Mode::Wizard => (
            wizard.as_ref().map_or("main_menu", |w| w.mode_name()),
            wizard.as_ref().map(|w| &w.menu),
        ),
        Mode::Playing => ("playing", None),
        Mode::Playback => ("playback", None),
        Mode::Replays => ("replays", shelf.as_ref().map(|s| &s.menu)),
        Mode::Pause => (
            if pause.as_ref().is_some_and(|p| p.confirming()) {
                "confirm_pause"
            } else {
                "pause_menu"
            },
            pause.as_ref().map(|p| &p.menu),
        ),
    };
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
                let requested = (*ticks).min(1_000_000);
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
            let ticks = ticks.min(1_000_000);
            game.advance_ticks(ticks);
            Ok(Reply::Advanced(AdvancedView {
                ticks,
                tick: game.state.current_tick(),
                hash: game.hash_hex(),
            }))
        }
        Request::Pause => {
            game.paused = true;
            Ok(Reply::Ok)
        }
        Request::Resume => {
            game.paused = false;
            // Resuming implies gameplay: leave the pause menu too, or the
            // sim runs behind a menu that still claims it is paused.
            if matches!(mode, Mode::Pause) {
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
            injected.push(event);
            Ok(Reply::Ok)
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

    #[test]
    fn automation_requires_debug_server() {
        assert!(Args::try_parse_from(["oxide-shell", "--automation"]).is_err());
        let args = Args::try_parse_from(["oxide-shell", "--debug-server", "--automation"])
            .expect("automation with the debug server should parse");
        assert!(args.debug_server);
        assert!(args.automation);
    }
}
