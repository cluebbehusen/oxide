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
mod camera;
mod config;
mod debug_server;
mod game;
mod input;
mod layout;
mod menu;
mod render;

use anyhow::{Context, Result};
use clap::Parser;
use debug_server::IncomingRequest;
use game::{Game, GameReplay, SoundKind};
use macroquad::audio::{PlaySoundParams, play_sound};
use macroquad::prelude::*;
use menu::{Menu, PreviewCache, ScenarioEntry};
use oxide_protocol::{
    AdvancedView, CameraView, HashView, Key, OverlayView, RawEvent, Reply, Request,
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
    /// Settings: Enter cycles a row's value; changes apply live and
    /// persist on the spot.
    Settings,
    /// Key remapping: Enter arms a row, the next key becomes its chord.
    Controls {
        /// The action row armed for rebinding, if any.
        rebinding: Option<usize>,
    },
    /// Scenario picker (the first New Match screen).
    MainMenu,
    /// Difficulty picker for the chosen scenario.
    DifficultyMenu,
    /// Personality picker (after difficulty).
    PersonalityMenu,
    /// Faction picker (after personality) — which roster the human plays.
    FactionMenu,
    /// The game proper.
    Playing,
    /// Game visible but veiled; the pause menu owns input.
    PauseMenu,
    /// A destructive pause choice awaiting explicit confirmation.
    ConfirmPause {
        /// The pause-menu row being confirmed (restart / main menu /
        /// quit).
        choice: usize,
    },
}

/// Everything New Match has chosen so far. The draft outlives every
/// screen transition: backing from Faction to Difficulty to the map
/// list and forward again re-offers each earlier answer instead of
/// forgetting it.
struct NewMatchDraft {
    /// The loaded map, once picked.
    scenario: Option<Box<Scenario>>,
    /// Map-list row, for re-preselection.
    scenario_choice: usize,
    /// Difficulty row (indexes `Level::LADDER`).
    level_choice: usize,
    /// Personality row (feeds `personality_knob`).
    personality_choice: usize,
    /// Faction row (Ferrous / Cupric / surprise).
    faction_choice: usize,
}

impl Default for NewMatchDraft {
    fn default() -> Self {
        Self {
            scenario: None,
            scenario_choice: 0,
            level_choice: 1, // Medium is the fair default
            personality_choice: 0,
            faction_choice: 0,
        }
    }
}

/// The wizard's menus, each preselected from the draft.
fn difficulty_menu(draft: &NewMatchDraft) -> Menu {
    let mut menu = Menu::new(
        "DIFFICULTY",
        DIFFICULTY_ITEMS.iter().map(|s| s.to_string()).collect(),
    );
    menu.select(draft.level_choice.min(DIFFICULTY_ITEMS.len() - 1));
    menu
}

fn personality_menu(draft: &NewMatchDraft) -> Menu {
    let mut menu = Menu::new(
        "OPPONENT",
        PERSONALITY_ITEMS.iter().map(|s| s.to_string()).collect(),
    );
    menu.select(draft.personality_choice.min(PERSONALITY_ITEMS.len() - 1));
    menu
}

fn faction_menu(draft: &NewMatchDraft) -> Menu {
    let mut menu = Menu::new(
        "FACTION",
        FACTION_ITEMS.iter().map(|s| s.to_string()).collect(),
    );
    menu.select(draft.faction_choice.min(FACTION_ITEMS.len() - 1));
    menu
}

/// Builds the game the draft describes.
fn launch(draft: &NewMatchDraft) -> Result<Game> {
    let mut scenario = (**draft.scenario.as_ref().context("draft has a map")?).clone();
    let level = oxide_sim::bot::Level::LADDER[draft.level_choice.min(3)];
    let aggression = personality_knob(draft.personality_choice);
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

const HOME_ITEMS: [&str; 3] = ["Play", "Settings", "Quit"];

fn home_menu() -> Menu {
    Menu::new("OXIDE", HOME_ITEMS.iter().map(|s| s.to_string()).collect())
}

/// The settings rows: label, the value steps it cycles through, and a
/// getter/setter pair over the config. Enter advances to the next step;
/// every change applies live and saves.
fn settings_menu(config: &config::Config) -> Menu {
    let pct = |v: f32| format!("{}%", (v * 100.0).round());
    let onoff = |v: bool| if v { "on" } else { "off" };
    Menu::new(
        "SETTINGS",
        vec![
            format!("Master volume: {}", pct(config.volumes.master)),
            format!("Effects volume: {}", pct(config.volumes.effects)),
            format!("UI volume: {}", pct(config.volumes.ui)),
            format!("UI scale: {}", pct(config.ui_scale)),
            format!("Edge pan: {}", onoff(config.camera.edge_pan)),
            format!("Invert zoom: {}", onoff(config.camera.zoom_inverted)),
            "Controls...".to_string(),
            "Back".to_string(),
        ],
    )
}

/// Advances one settings row to its next value step. Returns false on
/// the Back row.
fn cycle_setting(config: &mut config::Config, row: usize) -> bool {
    let step = |v: f32| {
        // 0 -> 25 -> 50 -> 75 -> 100 -> 0, tolerant of odd stored values.
        let next = ((v * 4.0).round() as u32 + 1) % 5;
        next as f32 / 4.0
    };
    match row {
        0 => config.volumes.master = step(config.volumes.master),
        1 => config.volumes.effects = step(config.volumes.effects),
        2 => config.volumes.ui = step(config.volumes.ui),
        3 => {
            // 75 -> 100 -> 125 -> 150 -> 75.
            config.ui_scale = match (config.ui_scale * 100.0).round() as u32 {
                75 => 1.0,
                100 => 1.25,
                125 => 1.5,
                _ => 0.75,
            };
            render::set_user_scale(config.ui_scale);
        }
        4 => config.camera.edge_pan = !config.camera.edge_pan,
        5 => config.camera.zoom_inverted = !config.camera.zoom_inverted,
        _ => return false, // Controls... and Back route in the arm
    }
    true
}

/// The remappable actions, in display order. Digits and structural keys
/// (Back, Confirm, group slots) stay fixed — their meaning is
/// positional, not preferential.
const REMAPPABLE: [(action::Action, &str); 10] = [
    (action::Action::StopOrScrap, "Stop / scrap site"),
    (action::Action::TrainSlot(0), "Train slot 1"),
    (action::Action::TrainSlot(1), "Train slot 2"),
    (action::Action::TogglePause, "Pause"),
    (action::Action::ToggleBuildPalette, "Build palette"),
    (action::Action::Patrol, "Patrol"),
    (action::Action::HomeCamera, "Center home"),
    (action::Action::ToggleOverlay, "Debug overlay"),
    (action::Action::PanLeft, "Pan left"),
    (action::Action::PanRight, "Pan right"),
];

fn controls_menu(config: &config::Config) -> Menu {
    let mut items: Vec<String> = REMAPPABLE
        .iter()
        .map(|(action, label)| {
            let chord = config
                .bindings
                .chord_for(*action)
                .map(action::BindingMap::chord_label)
                .unwrap_or_else(|| "unbound".to_string());
            format!("{label}: {chord}")
        })
        .collect();
    items.push("Back".to_string());
    Menu::new("CONTROLS", items)
}

const PAUSE_ITEMS: [&str; 4] = ["Resume", "Restart", "Main Menu", "Quit"];

/// Cancel sits first and preselected: confirming destruction takes a
/// deliberate second motion, never a double-tap.
fn confirm_menu(choice: usize) -> Menu {
    let verb = PAUSE_ITEMS.get(choice).copied().unwrap_or("Quit");
    Menu::new(
        format!("{}?", verb.to_uppercase()),
        vec!["Cancel".to_string(), verb.to_string()],
    )
}

fn build_main_menu(draft: &NewMatchDraft) -> (Menu, Vec<ScenarioEntry>) {
    let entries = menu::discover_scenarios();
    let mut items: Vec<String> = entries.iter().map(|e| e.label.clone()).collect();
    items.push("Back".to_string());
    let mut menu = Menu::new("OXIDE", items);
    menu.select(draft.scenario_choice.min(entries.len()));
    (menu, entries)
}

/// Plays queued clips with a per-kind rate limit, so twenty simultaneous
/// lasers read as battle, not noise.
#[derive(Default)]
struct Mixer {
    last_played: std::collections::HashMap<SoundKind, f64>,
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

    fn play(&mut self, sounds: &assets::Sounds, kind: SoundKind, volumes: &config::Volumes) {
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
        let volume = volume * Self::bus(volumes, kind);
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
    let mut home = home_menu();
    let mut mode = if args.automation {
        Mode::Home
    } else if args.debug_server || args.scenario.is_some() || args.replay.is_some() {
        Mode::Playing
    } else {
        Mode::MainMenu
    };
    let mut draft = NewMatchDraft::default();
    let mut previews = PreviewCache::default();
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
    let mut ui_view = capture_ui(&mode, &home, &main_menu, &sub_menu, &pause_menu, &game);

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
                    &ui_view,
                );
            }
        }

        let mut events = if args.automation {
            Vec::new()
        } else {
            input::poll_events(&input)
        };
        events.append(&mut injected);

        let mode_before = std::mem::discriminant(&mode);
        match mode {
            Mode::Home => {
                if let Some(choice) = home.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    match choice {
                        0 => {
                            main_menu = None;
                            mode = Mode::MainMenu;
                        }
                        1 => {
                            sub_menu = settings_menu(&config);
                            mode = Mode::Settings;
                        }
                        _ => std::process::exit(0),
                    }
                }
                render::draw(&game, &sprites, &input);
                veil();
                home.draw("machines eating a dead world");
            }
            Mode::Settings => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    mode = Mode::Home;
                } else if let Some(row) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    if cycle_setting(&mut config, row) {
                        // Apply live, persist immediately, keep the
                        // cursor on the row being tuned.
                        config.save().ok();
                        let selected = sub_menu.selected;
                        sub_menu = settings_menu(&config);
                        sub_menu.select(selected);
                    } else if row == 6 {
                        sub_menu = controls_menu(&config);
                        mode = Mode::Controls { rebinding: None };
                    } else {
                        mode = Mode::Home;
                    }
                }
                render::draw(&game, &sprites, &input);
                veil();
                sub_menu.draw("Enter cycles a value - changes stick immediately");
            }
            Mode::Controls { rebinding } => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if let Some(row) = rebinding {
                    // Armed: the next key IS the answer — raw, before any
                    // binding resolution, or the old meaning would fire.
                    let pressed = events.iter().find_map(|e| match e {
                        RawEvent::KeyDown { key } => Some(*key),
                        _ => None,
                    });
                    match pressed {
                        Some(Key::Escape) => {
                            mode = Mode::Controls { rebinding: None };
                        }
                        Some(key) if !matches!(key, Key::Shift | Key::Ctrl) => {
                            let (target, _) = REMAPPABLE[row];
                            let chord = action::Chord::bare(key);
                            if config.bindings.rebind(target, chord) {
                                config.save().ok();
                                input.bindings = config.bindings.clone();
                                sub_menu = controls_menu(&config);
                                sub_menu.select(row);
                                mode = Mode::Controls { rebinding: None };
                            } else {
                                game.toast("that key already means something");
                                game.sounds_pending.push(SoundKind::Denied);
                                mode = Mode::Controls { rebinding: None };
                            }
                        }
                        _ => {}
                    }
                } else if escaped {
                    sub_menu = settings_menu(&config);
                    sub_menu.select(6);
                    mode = Mode::Settings;
                } else if let Some(row) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    if row < REMAPPABLE.len() {
                        mode = Mode::Controls {
                            rebinding: Some(row),
                        };
                    } else {
                        sub_menu = settings_menu(&config);
                        sub_menu.select(6);
                        mode = Mode::Settings;
                    }
                }
                render::draw(&game, &sprites, &input);
                veil();
                let hint = if matches!(mode, Mode::Controls { rebinding: Some(_) }) {
                    "press the new key - Escape cancels"
                } else {
                    "Enter arms a row, then press its new key"
                };
                sub_menu.draw(hint);
            }
            Mode::MainMenu => {
                let (menu, entries) = main_menu.get_or_insert_with(|| build_main_menu(&draft));
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    mode = Mode::Home;
                } else if let Some(choice) = menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    if choice >= entries.len() {
                        // The appended Back row returns to the front door.
                        mode = Mode::Home;
                        render::draw(&game, &sprites, &input);
                        veil();
                        home.draw("machines eating a dead world");
                        continue;
                    }
                    let scenario = match &entries[choice].path {
                        Some(path) => Scenario::load(path)
                            .with_context(|| format!("loading {}", path.display()))?,
                        None => Scenario::skirmish(),
                    };
                    draft.scenario = Some(Box::new(scenario));
                    draft.scenario_choice = choice;
                    sub_menu = difficulty_menu(&draft);
                    mode = Mode::DifficultyMenu;
                    main_menu = None;
                }
                render::draw(&game, &sprites, &input);
                veil();
                if let Some((menu, entries)) = &main_menu {
                    // The subtitle browses with the player: the
                    // highlighted map's hook and badges, the pointer's
                    // row winning over the keyboard cursor.
                    let focus = menu.hover().unwrap_or(menu.selected);
                    let subtitle = entries
                        .get(focus)
                        .and_then(|e| e.blurb.as_deref())
                        .unwrap_or("machines eating a dead world");
                    menu.draw(subtitle);
                    // Fog-free preview of the highlighted map, softly
                    // panelled on the right.
                    if let Some(entry) = entries.get(focus)
                        && let Some(tex) = previews.get(focus, entry)
                    {
                        let s = render::ui_scale();
                        // Strictly right of the menu's own row rects —
                        // shared geometry, no independent arithmetic to
                        // drift out of sync. Too narrow? No panel.
                        let left_bound = menu.rows_right_edge() + 24.0 * s;
                        let avail = screen_width() - left_bound - 24.0 * s;
                        let max_w = avail.min(screen_width() * 0.26);
                        let max_h = screen_height() * 0.34;
                        if max_w >= 96.0 * s {
                            let scale = (max_w / tex.width()).min(max_h / tex.height());
                            let (w, h) = (tex.width() * scale, tex.height() * scale);
                            let x = screen_width() - w - 24.0 * s;
                            let y = screen_height() * 0.5 - h * 0.5;
                            draw_rectangle(
                                x - 8.0 * s,
                                y - 8.0 * s,
                                w + 16.0 * s,
                                h + 16.0 * s,
                                Color::from_rgba(20, 20, 24, 230),
                            );
                            draw_texture_ex(
                                tex,
                                x,
                                y,
                                WHITE,
                                DrawTextureParams {
                                    dest_size: Some(vec2(w, h)),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
            }
            Mode::DifficultyMenu => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    // Escape walks backward; the draft keeps every answer.
                    mode = Mode::MainMenu;
                } else if let Some(choice) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    draft.level_choice = choice;
                    sub_menu = personality_menu(&draft);
                    mode = Mode::PersonalityMenu;
                }
                render::draw(&game, &sprites, &input);
                veil();
                sub_menu.draw("how hard should it think?");
            }
            Mode::PersonalityMenu => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    sub_menu = difficulty_menu(&draft);
                    mode = Mode::DifficultyMenu;
                } else if let Some(choice) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    draft.personality_choice = choice;
                    sub_menu = faction_menu(&draft);
                    mode = Mode::FactionMenu;
                }
                render::draw(&game, &sprites, &input);
                veil();
                sub_menu.draw("every one is the same mind, dialed differently");
            }
            Mode::FactionMenu => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                if escaped {
                    sub_menu = personality_menu(&draft);
                    mode = Mode::PersonalityMenu;
                } else if let Some(choice) = sub_menu.handle(&events, &mut input.mouse) {
                    game.sounds_pending.push(SoundKind::Click);
                    draft.faction_choice = choice;
                    let fresh = launch(&draft)?;
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
                input.ui = render::ui_scale();
                input.now = get_time();
                input.camera_prefs = config.camera;
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
                    Some(destructive) => {
                        // Restart, Main Menu, and Quit all throw away a
                        // live match — each asks first, with Cancel
                        // preselected so a double-tap cannot destroy.
                        sub_menu = confirm_menu(destructive);
                        mode = Mode::ConfirmPause {
                            choice: destructive,
                        };
                    }
                    None if escape_pressed => {
                        game.paused = false;
                        mode = Mode::Playing;
                    }
                    None => {}
                }
            }
            Mode::ConfirmPause { choice } => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                let picked = sub_menu.handle(&events, &mut input.mouse);
                render::draw(&game, &sprites, &input);
                veil();
                sub_menu.draw("this throws the current match away");
                if escaped || picked == Some(0) {
                    mode = Mode::PauseMenu;
                } else if picked == Some(1) {
                    match choice {
                        1 => {
                            let fresh = Game::new(game.scenario.clone())?;
                            game = keep_flags(fresh, &game);
                            game.paused = false;
                            mode = Mode::Playing;
                            input.reset_session();
                        }
                        2 => {
                            main_menu = None;
                            mode = Mode::Home;
                        }
                        _ => std::process::exit(0),
                    }
                }
            }
        }

        if std::mem::discriminant(&mode) != mode_before {
            input.reset_transient();
        }
        if matches!(mode, Mode::MainMenu) && main_menu.is_none() {
            main_menu = Some(build_main_menu(&draft));
        }
        ui_view = capture_ui(&mode, &home, &main_menu, &sub_menu, &pause_menu, &game);

        let queued: Vec<SoundKind> = game.sounds_pending.drain(..).collect();
        for kind in queued {
            mixer.play(&sounds, kind, &config.volumes);
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

fn capture_ui(
    mode: &Mode,
    home: &Menu,
    main_menu: &Option<(Menu, Vec<ScenarioEntry>)>,
    sub_menu: &Menu,
    pause_menu: &Menu,
    game: &Game,
) -> UiView {
    let (mode_name, menu) = match mode {
        Mode::Home => ("home", Some(home)),
        Mode::Settings => ("settings", Some(sub_menu)),
        Mode::Controls { .. } => ("controls", Some(sub_menu)),
        Mode::MainMenu => ("main_menu", main_menu.as_ref().map(|(menu, _)| menu)),
        Mode::DifficultyMenu => ("difficulty_menu", Some(sub_menu)),
        Mode::PersonalityMenu => ("personality_menu", Some(sub_menu)),
        Mode::FactionMenu => ("faction_menu", Some(sub_menu)),
        Mode::Playing => ("playing", None),
        Mode::PauseMenu => ("pause_menu", Some(pause_menu)),
        Mode::ConfirmPause { .. } => ("confirm_pause", Some(sub_menu)),
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
            [l.top_bar_h, panel_top, m.x, m.y, m.w, m.h]
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
    ui_view: &UiView,
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
            if matches!(mode, Mode::PauseMenu | Mode::ConfirmPause { .. }) {
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
