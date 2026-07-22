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

mod assets;
mod camera;
mod debug_server;
mod game;
mod input;
mod menu;
mod render;

use anyhow::{Context, Result};
use clap::Parser;
use debug_server::IncomingRequest;
use game::{Game, GameReplay, SoundKind};
use macroquad::audio::{PlaySoundParams, play_sound};
use macroquad::prelude::*;
use menu::{Menu, ScenarioEntry};
use oxide_protocol::{
    AdvancedView, CameraView, HashView, Key, OverlayView, RawEvent, Reply, Request,
    ResponseEnvelope, SavedView, ScreenshotView, StateView, StatusView,
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

    /// Serve the debug protocol on --port (skips the menu).
    #[arg(long)]
    debug_server: bool,

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
    Conf {
        window_title: "Oxide".to_string(),
        window_width: 1280,
        window_height: 800,
        // Render at native pixel density — pre-atlas this was too many
        // pixels to afford; post-atlas it's crisp text and art for free.
        high_dpi: true,
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

/// Which screen owns input this frame.
enum Mode {
    /// Scenario picker.
    MainMenu,
    /// Difficulty picker for the chosen scenario.
    DifficultyMenu {
        /// The scenario about to start.
        scenario: Box<Scenario>,
    },
    /// Personality picker (after difficulty).
    PersonalityMenu {
        /// The scenario about to start.
        scenario: Box<Scenario>,
        /// The chosen ladder level.
        level: oxide_sim::bot::Level,
    },
    /// Faction picker (after personality) — which roster the human plays.
    FactionMenu {
        /// The scenario about to start.
        scenario: Box<Scenario>,
        /// The chosen ladder level.
        level: oxide_sim::bot::Level,
        /// The chosen personality knob.
        aggression: Option<u32>,
    },
    /// The game proper.
    Playing,
    /// Game visible but veiled; the pause menu owns input.
    PauseMenu,
}

const DIFFICULTY_ITEMS: [&str; 4] = ["Easy", "Medium", "Hard", "Expert"];
const PERSONALITY_ITEMS: [&str; 4] = ["Surprise me", "Turtle", "Balanced", "Aggressive"];
const FACTION_ITEMS: [&str; 3] = ["Ferrous", "Cupric", "Surprise me"];

fn personality_knob(choice: usize) -> Option<u32> {
    match choice {
        1 => Some(100), // Turtle
        2 => Some(500), // Balanced
        3 => Some(900), // Aggressive
        _ => None,      // Surprise me: dealt from the scenario seed
    }
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

const PAUSE_ITEMS: [&str; 4] = ["Resume", "Restart", "Main Menu", "Quit"];

fn build_main_menu() -> (Menu, Vec<ScenarioEntry>) {
    let entries = menu::discover_scenarios();
    let mut items: Vec<String> = entries.iter().map(|e| e.label.clone()).collect();
    items.push("Quit".to_string());
    (Menu::new("OXIDE", items), entries)
}

/// Plays queued clips with a per-kind rate limit, so twenty simultaneous
/// lasers read as battle, not noise.
#[derive(Default)]
struct Mixer {
    last_played: std::collections::HashMap<SoundKind, f64>,
}

impl Mixer {
    fn play(&mut self, sounds: &assets::Sounds, kind: SoundKind) {
        let now = get_time();
        let min_gap = match kind {
            SoundKind::Laser => 0.09,
            SoundKind::RailFire => 0.15,
            SoundKind::UnitDeath => 0.12,
            SoundKind::Flak => 0.12,
            SoundKind::Artillery => 0.2,
            _ => 0.05,
        };
        if now - self.last_played.get(&kind).copied().unwrap_or(f64::MIN) < min_gap {
            return;
        }
        self.last_played.insert(kind, now);
        let (sound, volume) = match kind {
            SoundKind::Laser => (&sounds.laser, 0.18),
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
        };
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
    // Straight into the game; the menu is for humans starting cold.
    let mut mode = if args.debug_server || args.scenario.is_some() || args.replay.is_some() {
        Mode::Playing
    } else {
        Mode::MainMenu
    };
    let mut main_menu: Option<(Menu, Vec<ScenarioEntry>)> = None;
    let mut sub_menu = Menu::new("", Vec::new());
    let mut pause_menu = Menu::new(
        "PAUSED",
        PAUSE_ITEMS.iter().map(|s| s.to_string()).collect(),
    );

    let debug_rx: Option<Receiver<IncomingRequest>> = if args.debug_server {
        Some(debug_server::spawn(args.port)?)
    } else {
        None
    };
    let mut input = input::InputState::new();
    let mut injected: Vec<RawEvent> = Vec::new();
    let mut pending_shots: Vec<PendingScreenshot> = Vec::new();

    loop {
        let dt = get_frame_time();
        // The camera never queries the window itself; feed it the viewport
        // once per frame (handles live resizes, keeps camera math pure),
        // then advance any zoom glide.
        game.camera
            .set_viewport(vec2(screen_width(), screen_height()));
        game.camera.update(dt);

        if let Some(rx) = &debug_rx {
            while let Ok(incoming) = rx.try_recv() {
                handle_request(
                    incoming,
                    &mut game,
                    &mut mode,
                    &mut input,
                    &mut injected,
                    &mut pending_shots,
                );
            }
        }

        let mut events = input::poll_events(&input);
        events.append(&mut injected);

        let mode_before = std::mem::discriminant(&mode);
        match mode {
            Mode::MainMenu => {
                let (menu, entries) = main_menu.get_or_insert_with(build_main_menu);
                if let Some(choice) = menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    if choice >= entries.len() {
                        std::process::exit(0); // the appended Quit row
                    }
                    let scenario = match &entries[choice].path {
                        Some(path) => Scenario::load(path)
                            .with_context(|| format!("loading {}", path.display()))?,
                        None => Scenario::skirmish(),
                    };
                    sub_menu = Menu::new(
                        "DIFFICULTY",
                        DIFFICULTY_ITEMS.iter().map(|s| s.to_string()).collect(),
                    );
                    sub_menu.selected = 1; // Medium is the fair default
                    mode = Mode::DifficultyMenu {
                        scenario: Box::new(scenario),
                    };
                    main_menu = None;
                }
                render::draw(&game, &sprites, &input);
                veil();
                if let Some((menu, _)) = &main_menu {
                    menu.draw("machines eating a dead world");
                }
            }
            Mode::DifficultyMenu { ref scenario } => {
                let _ = scenario;
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    // Escape walks backward through the flow.
                    mode = Mode::MainMenu;
                } else if let Some(choice) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    let level = oxide_sim::bot::Level::LADDER[choice.min(3)];
                    let scenario = scenario.clone();
                    sub_menu = Menu::new(
                        "OPPONENT",
                        PERSONALITY_ITEMS.iter().map(|s| s.to_string()).collect(),
                    );
                    mode = Mode::PersonalityMenu { scenario, level };
                }
                render::draw(&game, &sprites, &input);
                veil();
                sub_menu.draw("how hard should it think?");
            }
            Mode::PersonalityMenu {
                ref scenario,
                level,
            } => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    let scenario = scenario.clone();
                    sub_menu = Menu::new(
                        "DIFFICULTY",
                        DIFFICULTY_ITEMS.iter().map(|s| s.to_string()).collect(),
                    );
                    sub_menu.selected = oxide_sim::bot::Level::LADDER
                        .iter()
                        .position(|l| *l == level)
                        .unwrap_or(1);
                    mode = Mode::DifficultyMenu { scenario };
                } else if let Some(choice) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    let scenario = scenario.clone();
                    let aggression = personality_knob(choice);
                    sub_menu = Menu::new(
                        "FACTION",
                        FACTION_ITEMS.iter().map(|s| s.to_string()).collect(),
                    );
                    mode = Mode::FactionMenu {
                        scenario,
                        level,
                        aggression,
                    };
                }
                render::draw(&game, &sprites, &input);
                veil();
                sub_menu.draw("every one is the same mind, dialed differently");
            }
            Mode::FactionMenu {
                ref scenario,
                level,
                aggression,
            } => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    let scenario = scenario.clone();
                    sub_menu = Menu::new(
                        "OPPONENT",
                        PERSONALITY_ITEMS.iter().map(|s| s.to_string()).collect(),
                    );
                    mode = Mode::PersonalityMenu { scenario, level };
                } else if let Some(choice) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    let mut scenario = (**scenario).clone();
                    let config = oxide_sim::scenario::BotConfig { level, aggression };
                    for player in scenario.players.iter_mut().filter(|p| p.bot) {
                        player.bot_config = Some(config);
                    }
                    // The human seat plays the chosen roster; "surprise"
                    // lets the scenario seed pick.
                    let faction = match choice {
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
                    // In a duel, faction is also the only allegiance cue
                    // on screen — the opponent takes the other roster, or
                    // two same-color armies would fight an unreadable
                    // war. Team maps author their own mixed factions and
                    // carry an explicit ally marker instead.
                    if scenario.players.len() == 2
                        && let Some(bot) = scenario.players.iter_mut().find(|p| p.bot)
                    {
                        retint_seat(bot, complement);
                    }
                    let fresh = Game::new(scenario)?;
                    game = keep_flags(fresh, &game);
                    game.paused = false;
                    mode = Mode::Playing;
                    input.reset_session();
                }
                render::draw(&game, &sprites, &input);
                veil();
                sub_menu.draw("which roster do your machines run?");
            }
            Mode::Playing => {
                let had_selection =
                    !game.selection.units.is_empty() || game.selection.building.is_some();
                let escape_pressed = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                input::apply_events(&mut game, &mut input, &events);
                input::update_held(&mut game, &input, dt);
                // Escape walks outward: deselect first, then the menu.
                if escape_pressed && !had_selection {
                    game.paused = true;
                    mode = Mode::PauseMenu;
                }
                game.advance_wall_clock(dt);
                game.update_fx(dt);
                render::draw(&game, &sprites, &input);
            }
            Mode::PauseMenu => {
                let escape_pressed = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                let choice = pause_menu.handle(&events, &mut input.mouse);
                if choice.is_some() {
                    game.sounds_pending.push(SoundKind::Click);
                }
                render::draw(&game, &sprites, &input);
                veil();
                pause_menu.draw(&game.scenario.name);
                match choice {
                    Some(0) => {
                        game.paused = false;
                        mode = Mode::Playing;
                    }
                    Some(1) => {
                        let fresh = Game::new(game.scenario.clone())?;
                        game = keep_flags(fresh, &game);
                        game.paused = false;
                        mode = Mode::Playing;
                        input.reset_session();
                    }
                    Some(2) => {
                        main_menu = None;
                        mode = Mode::MainMenu;
                    }
                    Some(_) => std::process::exit(0),
                    None if escape_pressed => {
                        game.paused = false;
                        mode = Mode::Playing;
                    }
                    None => {}
                }
            }
        }

        if std::mem::discriminant(&mode) != mode_before {
            input.reset_transient();
        }

        let queued: Vec<SoundKind> = game.sounds_pending.drain(..).collect();
        for kind in queued {
            mixer.play(&sounds, kind);
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
        Color::new(0.05, 0.05, 0.07, 0.92),
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
fn handle_request(
    incoming: IncomingRequest,
    game: &mut Game,
    mode: &mut Mode,
    input: &mut input::InputState,
    injected: &mut Vec<RawEvent>,
    pending_shots: &mut Vec<PendingScreenshot>,
) {
    let IncomingRequest { id, request, reply } = incoming;
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
            if matches!(mode, Mode::PauseMenu) {
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
