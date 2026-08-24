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

mod screen_flow;

use crate::debug_server::IncomingRequest;
use crate::frame_profile::{FrameObservation, FrameProfiler};
use crate::game::{Game, GameReplay, SoundKind};
use crate::menu::{Menu, PreviewCache};
use crate::screens::codex::CodexScreen;
use crate::screens::final_map::FinalMapScreen;
use crate::screens::home::HomeScreen;
use crate::screens::pause::PauseScreen;
use crate::screens::playback::{PlaybackSession, ReturnTo as PlaybackReturn};
use crate::screens::results::ResultsScreen;
use crate::screens::settings::SettingsScreen;
use crate::screens::shelf::Shelf;
use crate::screens::wizard::{NewMatchDraft, Out as WizardOut, Step as WizardStep, Wizard};
use crate::{Args, assets, autosave, config, input, render, screens, theme, tutorial};
use anyhow::{Context, Result};
use macroquad::audio::{PlaySoundParams, Sound, play_sound};
use macroquad::prelude::*;
use oxide_protocol::{
    CameraView, Key, MouseButton, OverlayView, RawEvent, Reply, Request, ResponseEnvelope,
    SavedView, ScreenshotView, UiView,
};
use oxide_sim::{PlayerCommand, Scenario};
use std::sync::mpsc::{Receiver, Sender};

/// Which screen owns input this frame, holding that screen's state in
/// the same place. Screens carry no match choices — the
/// [`NewMatchDraft`] in [`App`] does, which is what lets Back walk the
/// wizard flow without losing anything; the tutorial survives Pause
/// round trips in [`App`] for the same reason.
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
    /// The codex — every machine and works with its figures — opened
    /// over Home or the paused match, which waits here intact exactly
    /// as it does under Settings.
    Codex {
        /// The screen itself.
        screen: CodexScreen,
        /// The displaced screen, restored wholesale on leave.
        back: Box<Screen>,
    },
    /// The New Match wizard (map grid, then match setup).
    Wizard(Wizard),
    /// The game proper — the session lives in [`App::game`], which
    /// every screen needs as its backdrop.
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
    /// The lesson cards, when school is in session. Lives here, not in
    /// a screen: the tutorial must survive Playing -> Pause -> Playing.
    tutorial: Option<tutorial::Tutorial>,
    /// The wizard's remembered choices. Lives here, not in the wizard:
    /// Home -> Wizard -> Back -> Wizard keeps the map pick and dials.
    draft: NewMatchDraft,
    /// The one input funnel.
    input: input::InputState,
    /// Map preview textures for the browser and setup screens.
    previews: PreviewCache,
    /// A menu-context error line (message, wall-clock deadline): map
    /// and launch failures report here and the menus stay up — the
    /// in-game toast strip only draws with the HUD.
    menu_notice: Option<(String, f64)>,
    /// Window-size persistence: written once the size has been stable
    /// for a second — a live resize is a burst of intermediate sizes
    /// nobody wants fsynced.
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
    pending_shots: Vec<PendingScreenshot>,
    /// The one texture atlas and its source rects.
    sprites: assets::Sprites,
    /// The generated clips.
    sounds: assets::Sounds,
    /// The rate-limiting clip player.
    mixer: Mixer,
    /// Continuously running score beds and their pure crossfade state.
    ///
    /// Automation leaves this absent so a driven shell never starts
    /// long-lived audio sources merely to take screenshots.
    soundtrack: Option<crate::soundtrack::Soundtrack>,
    /// Opt-in bounded native-frame timing, queried over the debug socket.
    frame_profiler: FrameProfiler,
}

/// Builds the game a filled-in draft describes.
fn launch(draft: &NewMatchDraft) -> Result<Game> {
    let mut scenario = (**draft.scenario.as_ref().context("draft has a map")?).clone();
    // One consumer, one source: the per-seat vector the setup screen
    // filled. Every opponent uses the same fog-honest scripted
    // controller; the setup screen chooses chairs, factions, and teams.
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
        player.bot_config = player
            .bot
            .then_some(oxide_sim::scenario::BotConfig::Scripted);
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
    // Per-seat team chips (the setup screen's): teams regroup seats
    // without touching factions — an FFA chip drops the seat onto its
    // own team, and the sim densifies chosen ids by first appearance
    // at build. The scenario carries the choice, so saves and replays
    // reproduce the grouping with no extra plumbing. An all-one-team
    // draft fails the build (OneTeam) like any other launch error;
    // the wizard refuses it earlier with the reason inline.
    for (i, plan) in draft.seats.iter().enumerate() {
        scenario.players[i].team = screens::wizard::team_override(plan.team_choice);
    }
    // Duels use the same independent faction choices as larger maps.
    // Auto keeps each seat's authored faction; overriding one seat
    // retints only that seat, which supports swaps and mirror matches.
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

/// Plays queued clips with a per-kind rate limit, so twenty simultaneous
/// weapon reports read as battle, not noise.
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

    fn min_gap(kind: SoundKind) -> f64 {
        match kind {
            SoundKind::Laser | SoundKind::ScuttlerFire => 0.09,
            SoundKind::SentinelFire => 0.08,
            SoundKind::LancerFire | SoundKind::Ack => 0.15,
            SoundKind::BombardFire
            | SoundKind::BastionFire
            | SoundKind::Artillery
            | SoundKind::ArtilleryLaunch => 0.2,
            SoundKind::FlakhoundFire | SoundKind::FlakTurretFire => 0.12,
            SoundKind::WardenFire => 0.1,
            SoundKind::BreakerFire
            | SoundKind::AvalancheFire
            | SoundKind::BombRelease
            | SoundKind::DemolitionBoom => 0.2,
            SoundKind::UpgradeDone => 0.3,
            SoundKind::StingerFire
            | SoundKind::BuzzardFire
            | SoundKind::DarterFire
            | SoundKind::TalonFire
            | SoundKind::WispFire => 0.1,
            SoundKind::UnitDeath => 0.12,
            SoundKind::Deposit => 0.15,
            SoundKind::Alert => 1.5,
            _ => 0.05,
        }
    }

    fn base_volume(kind: SoundKind) -> f32 {
        match kind {
            SoundKind::Laser => 0.18,
            SoundKind::UnitDeath => 0.35,
            SoundKind::BuildingBoom => 0.6,
            SoundKind::Deposit => 0.25,
            SoundKind::TrainDone => 0.3,
            SoundKind::Click => 0.25,
            SoundKind::Denied => 0.3,
            SoundKind::Alert => 0.4,
            SoundKind::Victory | SoundKind::Defeat => 0.6,
            SoundKind::Artillery => 0.5,
            SoundKind::ArtilleryLaunch => 0.4,
            SoundKind::Ack => 0.18,
            SoundKind::SentinelFire => 0.26,
            SoundKind::ScuttlerFire => 0.2,
            SoundKind::LancerFire => 0.32,
            SoundKind::BombardFire => 0.5,
            SoundKind::FlakhoundFire => 0.3,
            SoundKind::StingerFire => 0.25,
            SoundKind::BuzzardFire => 0.35,
            SoundKind::DarterFire => 0.25,
            SoundKind::TalonFire => 0.28,
            SoundKind::WispFire => 0.23,
            SoundKind::BastionFire => 0.55,
            SoundKind::FlakTurretFire => 0.34,
            SoundKind::WardenFire => 0.3,
            SoundKind::BreakerFire => 0.55,
            SoundKind::AvalancheFire => 0.5,
            SoundKind::BombRelease => 0.45,
            SoundKind::DemolitionBoom => 0.65,
            SoundKind::UpgradeDone => 0.35,
        }
    }

    fn clip<'a>(&mut self, sounds: &'a assets::Sounds, kind: SoundKind) -> &'a Sound {
        match kind {
            SoundKind::Laser => {
                self.laser_flip = !self.laser_flip;
                if self.laser_flip {
                    &sounds.laser
                } else {
                    &sounds.laser2
                }
            }
            SoundKind::UnitDeath => &sounds.unit_death,
            SoundKind::BuildingBoom => &sounds.building_boom,
            SoundKind::Deposit => &sounds.deposit,
            SoundKind::TrainDone => &sounds.train_done,
            SoundKind::Click => &sounds.click,
            SoundKind::Denied => &sounds.denied,
            SoundKind::Alert => &sounds.alert,
            SoundKind::Victory => &sounds.victory,
            SoundKind::Defeat => &sounds.defeat,
            SoundKind::Artillery => &sounds.artillery_boom,
            SoundKind::ArtilleryLaunch => &sounds.artillery_launch,
            SoundKind::Ack => &sounds.ack,
            SoundKind::SentinelFire => &sounds.attack_sentinel,
            SoundKind::ScuttlerFire => &sounds.attack_scuttler,
            SoundKind::LancerFire => &sounds.attack_lancer,
            SoundKind::BombardFire => &sounds.attack_bombard,
            SoundKind::FlakhoundFire => &sounds.attack_flakhound,
            SoundKind::StingerFire => &sounds.attack_stinger,
            SoundKind::BuzzardFire => &sounds.attack_buzzard,
            SoundKind::DarterFire => &sounds.attack_darter,
            SoundKind::TalonFire => &sounds.attack_talon,
            SoundKind::WispFire => &sounds.attack_wisp,
            SoundKind::BastionFire => &sounds.attack_bastion,
            SoundKind::FlakTurretFire => &sounds.attack_flak_turret,
            SoundKind::WardenFire => &sounds.attack_warden,
            SoundKind::BreakerFire => &sounds.attack_breaker,
            SoundKind::AvalancheFire => &sounds.avalanche_launch,
            SoundKind::BombRelease => &sounds.bomb_release,
            SoundKind::DemolitionBoom => &sounds.demolition_boom,
            SoundKind::UpgradeDone => &sounds.upgrade_done,
        }
    }

    fn play(
        &mut self,
        sounds: &assets::Sounds,
        kind: SoundKind,
        volumes: &config::Volumes,
        attenuation: f32,
    ) {
        let now = get_time();
        let min_gap = Self::min_gap(kind);
        if now - self.last_played.get(&kind).copied().unwrap_or(f64::MIN) < min_gap {
            return;
        }
        self.last_played.insert(kind, now);
        let volume = Self::base_volume(kind) * Self::bus(volumes, kind) * attenuation;
        if volume <= 0.0 {
            return;
        }
        let sound = self.clip(sounds, kind);
        play_sound(
            sound,
            PlaySoundParams {
                looped: false,
                volume,
            },
        );
    }
}

fn match_soundtrack_scene(game: &Game, paused: bool) -> crate::soundtrack::Scene {
    match game.state.result() {
        Some(oxide_sim::GameResult::Draw) => crate::soundtrack::Scene::Result,
        Some(oxide_sim::GameResult::Victory { team })
            if !game.state.player(game.human).resigned
                && game.state.player(game.human).team == team =>
        {
            crate::soundtrack::Scene::Victory
        }
        Some(oxide_sim::GameResult::Victory { .. }) => crate::soundtrack::Scene::Defeat,
        None if paused => crate::soundtrack::Scene::Pause,
        None => crate::soundtrack::Scene::Match,
    }
}

fn result_playback(game: &Game) -> Result<PlaybackSession> {
    let mut replay = game.recorder.clone();
    replay.meta.ticks = Some(game.state.current_tick());
    let mut session = PlaybackSession::from_replay(replay)?;
    session.return_to = PlaybackReturn::Results;
    Ok(session)
}

fn soundtrack_scene(screen: &Screen, game: &Game) -> crate::soundtrack::Scene {
    match screen {
        Screen::Playing => match_soundtrack_scene(game, false),
        Screen::Playback(playback) => match_soundtrack_scene(
            &playback.game,
            playback.paused || playback.seeking.is_some(),
        ),
        Screen::FinalMap(_) => match_soundtrack_scene(game, true),
        Screen::Pause(_) => match_soundtrack_scene(game, true),
        Screen::Settings { back, .. } | Screen::Codex { back, .. }
            if matches!(**back, Screen::Pause(_)) =>
        {
            match_soundtrack_scene(game, true)
        }
        Screen::Results(_) => match_soundtrack_scene(game, false),
        Screen::Home(_)
        | Screen::Settings { .. }
        | Screen::Codex { .. }
        | Screen::Wizard(_)
        | Screen::Replays(_) => crate::soundtrack::Scene::Menu,
    }
}

fn raises_combat_music(kind: SoundKind) -> bool {
    matches!(
        kind,
        SoundKind::Alert
            | SoundKind::Laser
            | SoundKind::UnitDeath
            | SoundKind::BuildingBoom
            | SoundKind::Artillery
            | SoundKind::ArtilleryLaunch
            | SoundKind::SentinelFire
            | SoundKind::ScuttlerFire
            | SoundKind::LancerFire
            | SoundKind::BombardFire
            | SoundKind::FlakhoundFire
            | SoundKind::StingerFire
            | SoundKind::BuzzardFire
            | SoundKind::DarterFire
            | SoundKind::TalonFire
            | SoundKind::WispFire
            | SoundKind::BastionFire
            | SoundKind::FlakTurretFire
            | SoundKind::WardenFire
            | SoundKind::BreakerFire
            | SoundKind::AvalancheFire
            | SoundKind::BombRelease
            | SoundKind::DemolitionBoom
    )
}

pub(crate) async fn run(args: Args) -> Result<()> {
    let trace = crate::trace_startup_enabled(&args);
    let mark = |label: &str| {
        if trace {
            crate::trace_mark(label);
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
    let config = config::Config::load();
    render::set_user_scale(config.ui_scale);
    render::set_reduced_motion(config.reduced_motion);
    render::set_colorblind(config.colorblind);
    mark("config loaded");
    let sprites = assets::Sprites::load().await?;
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
    let purposeful =
        (args.debug_server && !args.automation) || args.scenario.is_some() || args.replay.is_some();
    let mut screen = if let Some(path) = &args.watch {
        let mut session = PlaybackSession::open(path)?;
        // The clock flags drive whichever session is visible: a viewer
        // launch applies them to the transport, not the hidden match.
        session.paused = args.paused;
        session.speed = args.speed as f32;
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
    let mut app = App {
        args,
        config,
        game,
        tutorial: None,
        draft: NewMatchDraft::default(),
        input: input::InputState::new(),
        previews: PreviewCache::default(),
        menu_notice: None,
        pending_size: None,
        capture_ctrl: false,
        capture_shift: false,
        injected: Vec::new(),
        pending_shots: Vec::new(),
        sprites,
        sounds,
        mixer: Mixer::default(),
        soundtrack,
        frame_profiler: FrameProfiler::new(profile_frames),
    };
    let mut ui_view = capture_ui(&screen, &app);

    loop {
        let dt = get_frame_time();
        if let Some(rx) = &debug_rx {
            while let Ok(incoming) = rx.try_recv() {
                // An injected event is consumed by the NEXT frame; any
                // query drained after it in the same burst would answer
                // from the pre-input frame. Hold the rest of the queue
                // until the event has actually been felt.
                let holds_queries = matches!(incoming.request, Request::InjectEvent { .. });
                handle_request(incoming, &mut app, &mut screen, &ui_view);
                if holds_queries {
                    break;
                }
            }
        }

        // Debug requests are control-plane work between presented frames, not
        // native frame work. Start timing after draining them so Resume and
        // status polling cannot become an artificial slow frame.
        let frame_started = app.frame_profiler.enabled().then(std::time::Instant::now);
        let frame_tick_start = visible_tick(&screen, &app);
        let frame_mode = visible_profile_mode(&screen);
        // The camera never queries the window itself; feed it the viewport
        // once per frame (handles live resizes, keeps camera math pure),
        // then advance any zoom glide. Menus take the same injection —
        // their update logic runs headless in tests on the default size.
        app.game
            .camera
            .set_viewport(vec2(screen_width(), screen_height()));
        render::set_viewport(screen_width(), screen_height());
        app.game.camera.update(dt);

        let mut events = if app.args.automation {
            Vec::new()
        } else {
            input::poll_events(matches!(&screen, Screen::Pause(pause) if pause.naming()))
        };
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

        let screen_before = std::mem::discriminant(&screen);
        let screen_frame = screen_flow::update_and_draw(
            &mut app,
            screen,
            events,
            dt,
            ctrl_at_frame_start,
            shift_at_frame_start,
        )?;
        screen = screen_frame.screen;
        let rerun = screen_frame.rerun;
        let profile_frame_active = screen_frame.profile_frame_active;
        if rerun {
            record_profile_frame(
                &mut app,
                &screen,
                frame_mode,
                profile_frame_active,
                frame_tick_start,
                frame_started,
            );
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
                let width = measure_text(msg, None, (16.0 * s) as u16, 1.0).width;
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

        if std::mem::discriminant(&screen) != screen_before {
            app.input.reset_transient();
        }
        ui_view = capture_ui(&screen, &app);

        // The mixer serves whichever session is visible; playback owns a
        // separate presentation game and therefore a separate sound queue.
        let (queued, cam_center, cam_half_extents, cam_zoom): (
            Vec<(SoundKind, Option<Vec2>)>,
            Vec2,
            Vec2,
            f32,
        ) = match &mut screen {
            Screen::Playback(pb) => (
                std::mem::take(&mut pb.game.sounds_pending),
                pb.game.camera.center,
                pb.game.camera.viewport() / pb.game.camera.zoom * 0.5,
                pb.game.camera.zoom,
            ),
            _ => (
                std::mem::take(&mut app.game.sounds_pending),
                app.game.camera.center,
                app.game.camera.viewport() / app.game.camera.zoom * 0.5,
                app.game.camera.zoom,
            ),
        };
        let combat_impulse = queued.iter().any(|(kind, _)| raises_combat_music(*kind));
        for event in crate::audio_mix::frame_mix(queued, cam_center, cam_half_extents, cam_zoom) {
            app.mixer
                .play(&app.sounds, event.kind, &app.config.volumes, event.gain);
        }
        if let Some(soundtrack) = &mut app.soundtrack {
            soundtrack.update(
                soundtrack_scene(&screen, &app.game),
                combat_impulse,
                dt,
                app.config.volumes,
            );
            soundtrack.apply(&app.sounds);
        }

        if !app.pending_shots.is_empty() {
            // One readback serves every request that arrived this frame.
            let image = get_screen_data();
            for shot in app.pending_shots.drain(..) {
                let response = match write_png(&image, &shot.path) {
                    Ok((width, height)) => ResponseEnvelope::ok(
                        shot.id,
                        Reply::Screenshot(ScreenshotView {
                            path: shot.path,
                            width,
                            height,
                            renderer: "gpu".to_string(),
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
        let persist_size = app.args.window.is_none() && !app.args.automation;
        if persist_size && live.0 >= 640 && live.1 >= 400 && live != app.config.window {
            match app.pending_size {
                Some((size, since)) if size == live => {
                    if get_time() - since > 1.0 {
                        app.config.window = live;
                        if let Err(err) = app.config.save() {
                            let line = format!("could not save settings: {err}");
                            if matches!(screen, Screen::Playing) {
                                app.game.toast(line);
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
            match autosave::save(&mut app.game) {
                Ok(_) => std::process::exit(0),
                Err(err) => {
                    app.game.paused = true;
                    // The dialog's home-vs-match classification must
                    // see THROUGH screens opened from Pause: a quit
                    // while Settings or Playback sits over a paused
                    // match still has an unsaved match behind it, and
                    // a Home-classified Cancel would strand it with no
                    // route back.
                    let over_a_match = screen_holds_live_match(&screen);
                    screen = Screen::Pause(PauseScreen::open_save_failed(
                        err.player_line(),
                        screens::pause::LeaveVerb::Quit,
                        app.game.state.result().is_some(),
                        can_surrender(&app.game),
                        !over_a_match,
                    ));
                }
            }
        }

        record_profile_frame(
            &mut app,
            &screen,
            frame_mode,
            profile_frame_active,
            frame_tick_start,
            frame_started,
        );

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

/// Whether the human's seat can still concede: it holds a Foundry and
/// has not already resigned — the Surrender row's gate, matching the
/// sim's own command gate so the menu never offers a verb the sim
/// would only reject.
fn can_surrender(game: &Game) -> bool {
    !game.state.player(game.human).resigned && game.home_foundry().is_some()
}

fn visible_tick(screen: &Screen, app: &App) -> u64 {
    match screen {
        Screen::Playback(playback) => playback.engine.position(),
        _ => app.game.state.current_tick(),
    }
}

fn visible_profile_mode(screen: &Screen) -> &'static str {
    match screen {
        Screen::Home(_) => "home",
        Screen::Settings { .. } => "settings",
        Screen::Codex { .. } => "codex",
        Screen::Wizard(_) => "wizard",
        Screen::Playing => "playing",
        Screen::Playback(_) => "playback",
        Screen::FinalMap(_) => "final_map",
        Screen::Results(_) => "results",
        Screen::Replays(_) => "replays",
        Screen::Pause(_) => "pause",
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
        Screen::Home(_) | Screen::Wizard(_) | Screen::Replays(_) => false,
    }
}

/// Keeps the cross-screen cursor position honest even when a click arrives
/// without a preceding move event, as injected and some native clicks do.
fn track_pointer_position(mouse: &mut Vec2, event: &RawEvent) {
    match *event {
        RawEvent::MouseMove { x, y }
        | RawEvent::MouseDown { x, y, .. }
        | RawEvent::MouseUp { x, y, .. } => *mouse = vec2(x, y),
        _ => {}
    }
}

/// Carries session-level toggles (pause/speed/overlay) onto a fresh game.
fn keep_flags(mut fresh: Game, old: &Game) -> Game {
    fresh.paused = old.paused;
    fresh.speed = old.speed;
    fresh.overlay = old.overlay;
    fresh
}

fn capture_ui(screen: &Screen, app: &App) -> UiView {
    let (mode_name, menu): (&str, Option<&Menu>) = match screen {
        Screen::Home(home) => ("home", Some(&home.menu)),
        Screen::Settings { screen: sc, .. } => (sc.mode_name(), Some(&sc.menu)),
        Screen::Codex { screen: codex, .. } => (codex.mode_name(), Some(&codex.menu)),
        Screen::Wizard(w) => {
            // The wizard's custom screens (grid, setup) speak the same
            // protocol surface the row menus do — automation keeps its
            // footing across redesigns.
            let (title, items, selected) = w.ui_surface(&app.draft);
            // The frame injected this viewport before drawing, so the
            // range reports the grid window the player is actually
            // seeing.
            let visible = w.ui_visible_range(&app.draft, render::viewport(), render::ui_scale());
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
        Screen::Playing => ("playing", None),
        Screen::Playback(_) => ("playback", None),
        Screen::FinalMap(_) => ("final_map", None),
        Screen::Results(results) => {
            return UiView {
                mode: "results".to_string(),
                title: Some("MATCH RESULT".to_string()),
                selected: Some(results.selected()),
                items: results.items(),
                visible_range: Some([0, 4]),
                hover: results.hover(),
                chrome: None,
            };
        }
        Screen::Replays(shelf) => ("replays", Some(&shelf.menu)),
        Screen::Pause(ps) => (
            if ps.saving_failed() {
                "save_failed"
            } else if ps.naming() {
                "save_name"
            } else if ps.confirming() {
                "confirm_pause"
            } else {
                "pause_menu"
            },
            Some(&ps.menu),
        ),
    };
    UiView {
        mode: mode_name.to_string(),
        title: menu.map(|menu| menu.title.clone()),
        selected: menu.map(|menu| menu.selected),
        items: menu.map_or_else(Vec::new, |menu| menu.items.clone()),
        visible_range: menu.map(Menu::visible_range),
        hover: menu.and_then(Menu::hover),
        chrome: matches!(screen, Screen::Playing).then(|| {
            let l = app.game.layout.get();
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
    // The GL framebuffer is bottom-up; PNG rows are top-down.
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

/// The mutating verbs a read-only viewer refuses wholesale — commands
/// and session swaps would act through the replay onto the hidden match
/// behind it.
fn viewer_refuses(request: &Request) -> bool {
    matches!(
        request,
        Request::BeginPerformanceWindow { .. }
            | Request::SendCommand { .. }
            | Request::LoadScenario { .. }
            | Request::LoadReplay { .. }
            | Request::SaveReplay { .. }
    )
}

fn frozen_map_refuses(request: &Request) -> bool {
    viewer_refuses(request)
        || matches!(
            request,
            Request::AdvanceTicks { .. }
                | Request::PresentTicks { .. }
                | Request::Resume
                | Request::SetSpeed { .. }
        )
}

/// Answers one debug request. Screenshots are parked; everything else
/// responds immediately, between frames, against a settled world.
///
/// One dispatcher over every session kind: the shared surface (state
/// reads, the driven clock) goes through
/// [`oxide_protocol::dispatch_shared`] against whichever session owns
/// the screen — the replay viewer when it is up, the live game
/// otherwise. What remains here is window-shaped (camera, UI, input,
/// screenshots, the overlay), answered for the screen the window shows,
/// or live-mutating (commands, loads, saves), refused wholesale while
/// the viewer owns the screen.
/// Which session a request addresses, and whether it is allowed to —
/// decided before any state is touched. The Playback pick is the
/// load-bearing row: a regression there aims a viewer-bound
/// `AdvanceTicks` at the hidden live match, which advances silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The final battlefield is frozen; time and session verbs bounce.
    RefuseFrozen,
    /// A local mutating verb while the read-only viewer owns the screen.
    RefuseViewer,
    /// A locally-answered verb against the app's own state.
    Local,
}

/// The local routing guards around shared protocol dispatch. Frozen-map
/// refusal runs before dispatch; the viewer's read-only guard runs after a
/// request proves not to be shared.
fn route(playback: bool, final_map: bool, request: &Request) -> Route {
    if final_map && frozen_map_refuses(request) {
        return Route::RefuseFrozen;
    }
    if playback && viewer_refuses(request) {
        return Route::RefuseViewer;
    }
    Route::Local
}

fn handle_request(incoming: IncomingRequest, app: &mut App, screen: &mut Screen, ui_view: &UiView) {
    let IncomingRequest { id, request, reply } = incoming;
    let playback = matches!(&*screen, Screen::Playback(_));
    let final_map = matches!(&*screen, Screen::FinalMap(_));
    if route(playback, final_map, &request) == Route::RefuseFrozen {
        reply
            .send(ResponseEnvelope::err(
                id,
                "the final battlefield is frozen; return to the report first".to_string(),
            ))
            .ok();
        return;
    }
    let shared = {
        let session: &mut dyn oxide_protocol::DebugSession = match &mut *screen {
            Screen::Playback(pb) => &mut **pb,
            _ => &mut app.game,
        };
        oxide_protocol::dispatch_shared(session, &request)
    };
    if let Some(outcome) = shared {
        // Resuming implies gameplay: leave the pause menu too, or the
        // sim runs behind a menu that still claims it is paused —
        // Settings opened over a paused match included. A screen
        // transition, not a session operation, which is why it lives
        // with the screen's owner instead of inside the trait. (While
        // the viewer owns the screen no pause menu can be up, so this
        // matches nothing there.)
        if matches!(request, Request::Resume) && outcome.is_ok() {
            let over_pause = match &*screen {
                Screen::Pause(_) => true,
                Screen::Settings { back, .. } | Screen::Codex { back, .. } => {
                    matches!(**back, Screen::Pause(_))
                }
                _ => false,
            };
            if over_pause {
                *screen = Screen::Playing;
                app.input.reset_transient();
            }
        }
        let envelope = match outcome {
            Ok(inner) => ResponseEnvelope::ok(id, inner),
            Err(error) => ResponseEnvelope::err(id, error),
        };
        reply.send(envelope).ok();
        return;
    }
    // The viewer is read-only: refusing beats acknowledging a request
    // that would silently mutate the hidden match.
    if route(playback, final_map, &request) == Route::RefuseViewer {
        let refusal = "the viewer is read-only; leave playback first".to_string();
        reply.send(ResponseEnvelope::err(id, refusal)).ok();
        return;
    }
    let game = &mut app.game;
    let outcome: Result<Reply, String> =
        match request {
            Request::QueryCamera => {
                // Window-shaped answers describe the screen the window
                // shows — the viewer's render vehicle during playback.
                let game = match &*screen {
                    Screen::Playback(pb) => &pb.game,
                    _ => &*game,
                };
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
            Request::QueryPerformance { reset } => {
                if app.frame_profiler.enabled() {
                    Ok(Reply::Performance(app.frame_profiler.snapshot(reset)))
                } else {
                    Err("native frame profiling is disabled; launch the shell with --profile-frames"
                    .to_string())
                }
            }
            Request::BeginPerformanceWindow { from_tick, to_tick } => {
                if !matches!(&*screen, Screen::Playing) {
                    Err("exact frame windows require the live Playing screen".to_string())
                } else if !game.paused {
                    Err("pause the live match before arming a frame window".to_string())
                } else if game.state.current_tick() != from_tick {
                    Err(format!(
                        "profile window starts at tick {from_tick}, but the live match is at {}",
                        game.state.current_tick()
                    ))
                } else {
                    app.frame_profiler
                        .arm(from_tick, to_tick)
                        .map(|()| Reply::Ok)
                }
            }
            Request::ToggleOverlay => {
                let game = match &mut *screen {
                    Screen::Playback(pb) => &mut pb.game,
                    _ => game,
                };
                game.overlay = !game.overlay;
                Ok(Reply::Overlay(OverlayView {
                    enabled: game.overlay,
                }))
            }
            Request::SendCommand { player, command } => {
                if (player.0 as usize) < game.state.players().len() {
                    game.stage(PlayerCommand { player, command });
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
                        "text event {ch:?} is outside printable ASCII; the funnel refuses it"
                    ))
                } else {
                    app.injected.push(event);
                    Ok(Reply::Ok)
                }
            }
            Request::Screenshot { path } => {
                // The default name carries the tick of the world the frame
                // will actually show — the replayed one during playback.
                let shown_tick = match &*screen {
                    Screen::Playback(pb) => pb.engine.state.current_tick(),
                    _ => game.state.current_tick(),
                };
                let path = path.unwrap_or_else(|| format!("screenshots/tick-{shown_tick}.png"));
                app.pending_shots
                    .push(PendingScreenshot { id, path, reply });
                return; // responds after the frame renders
            }
            Request::LoadScenario { path } => Scenario::load(&path)
                .map_err(|err| format!("loading {path}: {err}"))
                .and_then(|scenario| {
                    Game::new(scenario).map_err(|err| format!("building scenario: {err:#}"))
                })
                .map(|fresh| {
                    app.tutorial = None;
                    *game = keep_flags(fresh, game);
                    *screen = Screen::Playing;
                    app.input.reset_session();
                    Reply::Ok
                }),
            Request::LoadReplay { path } => GameReplay::load(&path)
                .map_err(|err| format!("loading replay {path}: {err}"))
                .and_then(|replay| {
                    Game::from_replay(replay).map_err(|err| format!("resuming replay: {err:#}"))
                })
                .map(|fresh| {
                    app.tutorial = None;
                    *game = keep_flags(fresh, game);
                    *screen = Screen::Playing;
                    app.input.reset_session();
                    Reply::Status(game.status_view())
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
            // The shared surface was answered above; listing it keeps this
            // match exhaustive, so a new protocol request forces a decision
            // about which side of the capability split it lives on.
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
        };
    let response = match outcome {
        Ok(ok) => ResponseEnvelope::ok(id, ok),
        Err(err) => ResponseEnvelope::err(id, err),
    };
    reply.send(response).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The routing guards, row by row: frozen-map precedence and the
    /// viewer's read-only boundary around local requests. Shared requests
    /// are covered by the dispatcher and session-parity suites.
    #[test]
    fn local_request_guards_follow_screen_ownership() {
        let advance = Request::AdvanceTicks { ticks: 8 };
        let send = Request::SendCommand {
            player: oxide_sim::PlayerId(0),
            command: oxide_sim::Command::Surrender,
        };
        let camera = Request::QueryCamera;

        // The frozen final map refuses time and mutation before shared
        // dispatch can advance the hidden live match.
        assert_eq!(route(false, true, &advance), Route::RefuseFrozen);
        assert_eq!(route(false, true, &send), Route::RefuseFrozen);
        assert_eq!(route(false, true, &camera), Route::Local);

        // The read-only viewer bounces local mutation and answers
        // local reads.
        assert_eq!(route(true, false, &send), Route::RefuseViewer);
        assert_eq!(route(true, false, &camera), Route::Local);

        // The live screen answers everything else locally.
        assert_eq!(route(false, false, &send), Route::Local);
    }

    #[test]
    fn every_authored_weapon_report_raises_combat_pressure() {
        for kind in [
            SoundKind::Alert,
            SoundKind::SentinelFire,
            SoundKind::ScuttlerFire,
            SoundKind::LancerFire,
            SoundKind::BombardFire,
            SoundKind::FlakhoundFire,
            SoundKind::StingerFire,
            SoundKind::BuzzardFire,
            SoundKind::DarterFire,
            SoundKind::TalonFire,
            SoundKind::WispFire,
            SoundKind::BastionFire,
            SoundKind::FlakTurretFire,
            SoundKind::ArtilleryLaunch,
            SoundKind::WardenFire,
            SoundKind::BreakerFire,
            SoundKind::AvalancheFire,
            SoundKind::BombRelease,
            SoundKind::DemolitionBoom,
        ] {
            assert!(
                raises_combat_music(kind),
                "{kind:?} must pressure the score"
            );
        }
    }

    #[test]
    fn mixer_specs_match_the_finalized_manifest_contract() {
        assert_eq!(Mixer::base_volume(SoundKind::ScuttlerFire), 0.20);
        assert_eq!(Mixer::min_gap(SoundKind::ScuttlerFire), 0.09);
        assert_eq!(Mixer::base_volume(SoundKind::Alert), 0.40);
        assert_eq!(Mixer::min_gap(SoundKind::Alert), 1.50);
        assert_eq!(Mixer::base_volume(SoundKind::ArtilleryLaunch), 0.40);
        assert_eq!(Mixer::min_gap(SoundKind::ArtilleryLaunch), 0.20);
        assert_eq!(Mixer::base_volume(SoundKind::Deposit), 0.25);
        assert_eq!(Mixer::min_gap(SoundKind::Deposit), 0.15);
    }

    fn team_draft() -> NewMatchDraft {
        let mut draft = NewMatchDraft::default();
        let scenario = Scenario::load("../scenarios/trident-plateau.json").expect("shipped map");
        draft.set_scenario(scenario, None);
        draft
    }

    #[test]
    fn launch_uses_the_scripted_controller_for_every_opponent() {
        let mut draft = team_draft();
        draft.seat_choice = 2;
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
            assert_eq!(
                p.bot_config,
                Some(oxide_sim::scenario::BotConfig::Scripted),
                "every opponent uses the fair scripted controller"
            );
        }
        // Auto chips keep the seat's authored faction.
        assert_eq!(players[2].faction, oxide_sim::Faction::Ferrous);
    }

    #[test]
    fn wizard_seat_swap_opens_on_the_new_humans_foundry() {
        render::set_viewport(1280.0, 800.0);
        let scenario = Scenario::load("../scenarios/basalt-spine.json").expect("shipped map");

        let mut input = input::InputState::new();
        input.camera_prefs.edge_pan = true;
        for event in [
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: 352.0,
                y: 222.0,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: 352.0,
                y: 222.0,
            },
        ] {
            track_pointer_position(&mut input.mouse, &event);
        }
        input.reset_session();
        assert_eq!(input.mouse, vec2(352.0, 222.0));

        let mut backdrop_draft = NewMatchDraft::default();
        backdrop_draft.set_scenario(scenario.clone(), None);
        let mut backdrop = launch(&backdrop_draft).expect("backdrop match");
        backdrop.camera.pan(vec2(-1000.0, -1000.0));
        backdrop.paused = true;
        backdrop.speed = 4.0;
        backdrop.overlay = true;
        let backdrop_center = backdrop.camera.center;

        let mut draft = NewMatchDraft::default();
        draft.set_scenario(scenario, None);
        draft.seat_choice = 1;
        let mut game = keep_flags(launch(&draft).expect("swapped-seat match"), &backdrop);

        assert_eq!(game.human, oxide_sim::PlayerId(1));
        let home = game.home_foundry().expect("human Foundry").center();
        let home = vec2(home.x.to_num::<f32>(), home.y.to_num::<f32>());
        let (lo, hi) = game.camera.world_rect();
        assert!(
            home.x >= lo.x && home.x <= hi.x && home.y >= lo.y && home.y <= hi.y,
            "the new human's Foundry at {home:?} is outside the opening view {lo:?}..{hi:?}"
        );
        assert_ne!(
            game.camera.center, backdrop_center,
            "session flags must not carry the backdrop camera into the new match"
        );
        assert!(game.paused);
        assert_eq!(game.speed, 4.0);
        assert!(game.overlay);

        let opening_center = game.camera.center;
        input::update_held(&mut game, &input, 1.0);
        assert_eq!(
            game.camera.center, opening_center,
            "edge pan must not mistake the menu click for a pointer at (0, 0)"
        );
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
    fn launch_writes_the_chosen_teams_into_the_scenario() {
        // Untouched dials reproduce the authored grouping — the
        // scenario (and so every save and replay) carries the teams.
        let draft = team_draft(); // trident-plateau: teams 0,0,0 / 1,1,1
        let game = launch(&draft).expect("launches");
        let teams: Vec<Option<u8>> = game.scenario.players.iter().map(|p| p.team).collect();
        assert_eq!(
            teams,
            vec![Some(0), Some(0), Some(0), Some(1), Some(1), Some(1)],
            "defaults launch the map as authored"
        );

        // Re-dialed seats regroup: FFA drops the seat onto its own
        // team, a moved seat joins its new one — factions untouched.
        let mut draft = team_draft();
        let authored: Vec<_> = draft
            .scenario
            .as_deref()
            .unwrap()
            .players
            .iter()
            .map(|p| p.faction)
            .collect();
        draft.seats[0].team_choice = 0; // FFA
        draft.seats[3].team_choice = 1; // crosses to Team 1
        let game = launch(&draft).expect("launches");
        let players = &game.scenario.players;
        assert_eq!(players[0].team, None, "the FFA seat drops its team");
        assert_eq!(players[3].team, Some(0), "the moved seat joined Team 1");
        assert_eq!(players[1].team, Some(0));
        let launched: Vec<_> = players.iter().map(|p| p.faction).collect();
        assert_eq!(launched, authored, "the team dial never retints a seat");
        // The sim's dense normalization sees the regrouping: the FFA
        // seat stands alone against everyone.
        let alone = game.state.player(oxide_sim::PlayerId(0)).team;
        assert!(
            (1..players.len())
                .all(|i| game.state.player(oxide_sim::PlayerId(i as u8)).team != alone),
            "an FFA seat shares a team with no one"
        );
    }

    #[test]
    fn an_all_one_team_draft_fails_the_launch_instead_of_the_process() {
        // The wizard refuses this at Start; launch stays the backstop
        // and surfaces the sim's OneTeam build error as a menu notice,
        // never a crash.
        let mut draft = team_draft();
        for plan in &mut draft.seats {
            plan.team_choice = 1;
        }
        assert!(launch(&draft).is_err(), "one team can never launch");
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
    fn soundtrack_context_tracks_pause_victory_and_surrender() {
        let mut won = Game::new(Scenario::skirmish()).expect("game");
        assert_eq!(
            match_soundtrack_scene(&won, false),
            crate::soundtrack::Scene::Match
        );
        assert_eq!(
            match_soundtrack_scene(&won, true),
            crate::soundtrack::Scene::Pause
        );
        won.state.tick(&[oxide_sim::PlayerCommand {
            player: oxide_sim::PlayerId(1),
            command: oxide_sim::Command::Surrender,
        }]);
        assert_eq!(
            match_soundtrack_scene(&won, false),
            crate::soundtrack::Scene::Victory
        );

        let mut lost = Game::new(Scenario::skirmish()).expect("game");
        lost.state.tick(&[oxide_sim::PlayerCommand {
            player: lost.human,
            command: oxide_sim::Command::Surrender,
        }]);
        assert_eq!(
            match_soundtrack_scene(&lost, false),
            crate::soundtrack::Scene::Defeat,
            "a resigned human never hears a teammate's eventual win as their victory"
        );
    }

    #[test]
    fn final_map_allows_inspection_but_refuses_time_and_session_mutation() {
        for request in [
            Request::AdvanceTicks { ticks: 1 },
            Request::PresentTicks { ticks: 1 },
            Request::Resume,
            Request::SetSpeed { multiplier: 2.0 },
            Request::BeginPerformanceWindow {
                from_tick: 0,
                to_tick: 1,
            },
            Request::LoadScenario {
                path: "other.json".to_string(),
            },
        ] {
            assert!(frozen_map_refuses(&request), "{request:?}");
        }
        for request in [
            Request::Status,
            Request::QueryState {
                filter: oxide_protocol::StateFilter::default(),
            },
            Request::QueryCamera,
            Request::QueryUi,
            Request::Screenshot { path: None },
        ] {
            assert!(!frozen_map_refuses(&request), "{request:?}");
        }
    }

    #[test]
    fn result_replay_returns_to_the_report() {
        let game = Game::new(Scenario::skirmish()).expect("game");

        let watch = result_playback(&game).expect("watch replay");
        assert_eq!(watch.return_to, PlaybackReturn::Results);
        assert!(!watch.paused);
        assert!(watch.seeking.is_none());
    }

    #[test]
    fn failed_quit_dialogs_return_to_every_hidden_live_match() {
        let home = || Screen::Home(HomeScreen::with_resumable(false));
        let pause = || Screen::Pause(PauseScreen::open(false, true));

        assert!(!screen_holds_live_match(&home()));
        assert!(screen_holds_live_match(&Screen::Playing));
        assert!(screen_holds_live_match(&pause()));
        assert!(screen_holds_live_match(&Screen::Results(
            ResultsScreen::open()
        )));
        assert!(screen_holds_live_match(&Screen::FinalMap(
            FinalMapScreen::open()
        )));

        let settings_from_home = Screen::Settings {
            screen: SettingsScreen::open(&config::Config::default()),
            back: Box::new(home()),
        };
        let settings_from_pause = Screen::Settings {
            screen: SettingsScreen::open(&config::Config::default()),
            back: Box::new(pause()),
        };
        assert!(!screen_holds_live_match(&settings_from_home));
        assert!(screen_holds_live_match(&settings_from_pause));

        let game = Game::new(Scenario::skirmish()).expect("game");
        let mut playback = result_playback(&game).expect("viewer");
        playback.return_to = PlaybackReturn::Home;
        assert!(!screen_holds_live_match(&Screen::Playback(Box::new(
            playback
        ))));
        let mut playback = result_playback(&game).expect("viewer");
        playback.return_to = PlaybackReturn::Pause;
        assert!(screen_holds_live_match(&Screen::Playback(Box::new(
            playback
        ))));
    }

    #[test]
    fn screenshot_encoding_flips_gpu_rows_and_reports_path_errors() {
        let root = std::env::temp_dir().join(format!(
            "oxide-png-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let path = root.join("nested/shot.png");
        let image = Image {
            width: 2,
            height: 2,
            // GPU order: bottom row first, then top row.
            bytes: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };

        assert_eq!(write_png(&image, path.to_str().unwrap()).unwrap(), (2, 2));
        let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        assert_eq!(
            &decoded[..info.buffer_size()],
            &[
                0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255,
            ],
            "PNG rows are top-down even though the captured framebuffer is bottom-up"
        );

        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        assert!(
            write_png(&image, blocked.join("shot.png").to_str().unwrap()).is_err(),
            "a malformed screenshot path is a protocol error, not a process panic"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
