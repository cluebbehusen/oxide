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
//! Every screen variant carries its screen's whole state, so a mode
//! without its payload is unrepresentable — the old coordinator kept a
//! bare `Mode` discriminant beside six parallel `Option` locals and
//! spent five repair arms defending states the types admitted.

use crate::debug_server::IncomingRequest;
use crate::game::{Game, GameReplay, SoundKind};
use crate::menu::{Menu, PreviewCache};
use crate::screens::home::HomeScreen;
use crate::screens::pause::PauseScreen;
use crate::screens::playback::{PlaybackSession, ReturnTo as PlaybackReturn};
use crate::screens::results::ResultsScreen;
use crate::screens::settings::SettingsScreen;
use crate::screens::shelf::Shelf;
use crate::screens::wizard::{NewMatchDraft, Out as WizardOut, Step as WizardStep, Wizard};
use crate::{Args, assets, autosave, config, input, render, screens, theme, tutorial};
use anyhow::{Context, Result};
use macroquad::audio::{PlaySoundParams, play_sound};
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
}

/// Builds the game a filled-in draft describes.
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
                aggression: None,
                style: screens::wizard::personality_style(plan.personality_choice),
                variant: None,
                team_role: None,
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

fn result_playback(game: &Game, final_map: bool) -> Result<PlaybackSession> {
    let mut replay = game.recorder.clone();
    replay.meta.ticks = Some(game.state.current_tick());
    let mut session = if final_map {
        PlaybackSession::from_replay_at_end(replay)?
    } else {
        PlaybackSession::from_replay(replay)?
    };
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
        Screen::Pause(_) => match_soundtrack_scene(game, true),
        Screen::Settings { back, .. } if matches!(**back, Screen::Pause(_)) => {
            match_soundtrack_scene(game, true)
        }
        Screen::Results(_) => match_soundtrack_scene(game, false),
        Screen::Home(_) | Screen::Settings { .. } | Screen::Wizard(_) | Screen::Replays(_) => {
            crate::soundtrack::Scene::Menu
        }
    }
}

fn raises_combat_music(kind: SoundKind) -> bool {
    matches!(
        kind,
        SoundKind::Laser
            | SoundKind::RailFire
            | SoundKind::UnitDeath
            | SoundKind::BuildingBoom
            | SoundKind::Flak
            | SoundKind::Artillery
            | SoundKind::ArtilleryLaunch
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
    };
    let mut ui_view = capture_ui(&screen, &app);

    loop {
        let dt = get_frame_time();
        // The camera never queries the window itself; feed it the viewport
        // once per frame (handles live resizes, keeps camera math pure),
        // then advance any zoom glide. Menus take the same injection —
        // their update logic runs headless in tests on the default size.
        app.game
            .camera
            .set_viewport(vec2(screen_width(), screen_height()));
        render::set_viewport(screen_width(), screen_height());
        app.game.camera.update(dt);

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
            match e {
                RawEvent::KeyDown { key: Key::Ctrl } => app.capture_ctrl = true,
                RawEvent::KeyUp { key: Key::Ctrl } => app.capture_ctrl = false,
                RawEvent::KeyDown { key: Key::Shift } => app.capture_shift = true,
                RawEvent::KeyUp { key: Key::Shift } => app.capture_shift = false,
                _ => {}
            }
        }

        let screen_before = std::mem::discriminant(&screen);
        // Menu backdrops are presentation worlds too. Home, setup, and
        // the replay shelf animate even when `--paused` reserves the next
        // match for driven control; Settings inherits its caller, so the
        // pause-menu path stays frozen. Playing and Playback advance their
        // own clocks below, while Pause deliberately advances neither.
        match &screen {
            Screen::Home(_) | Screen::Wizard(_) | Screen::Replays(_) | Screen::Results(_) => {
                app.game.update_fx(dt);
            }
            Screen::Settings { back, .. } if !matches!(**back, Screen::Pause(_)) => {
                app.game.update_fx(dt);
            }
            Screen::Settings { .. } | Screen::Playing | Screen::Playback(_) | Screen::Pause(_) => {}
        }
        // A `rerun` transition re-enters the loop under the new screen
        // before presenting, so the frame shows the destination — the
        // old coordinator's `continue` arms.
        let mut rerun = false;
        screen = match screen {
            Screen::Home(mut home) => {
                // The title scene: a cold front door drifts its camera
                // slowly across the backdrop world instead of freezing
                // a frame — presentation only, and only while nothing
                // is at stake (a resumable match keeps its exact view).
                if app.game.state.current_tick() == 0 && !render::reduced_motion() {
                    app.game.camera.pan(vec2(dt * 0.55, dt * 0.22));
                    let (_, hi) = app.game.camera.world_rect();
                    if hi.x >= app.game.state.map().width() as f32 + 1.9 {
                        app.game.camera.center = vec2(0.0, 0.0);
                        app.game.camera.pan(vec2(0.0, 0.0)); // re-clamp home
                    }
                }
                let out = home.update(&events, &mut app.input.mouse, &mut app.game.sounds_pending);
                // Session verbs first — Continue and Tutorial swap the
                // game this frame then draws under the menu. The menu
                // draw needs `home`, so verbs that displace it (only
                // Settings) build their screen after the draw below.
                let mut next: Option<Screen> = None;
                match out {
                    screens::home::Out::Stay | screens::home::Out::Settings => {}
                    screens::home::Out::Continue => {
                        // Resume the newest autosave — a replay load, so
                        // it cannot desync from its own history.
                        if let Some(fresh) =
                            autosave::latest_compatible().and_then(|path| resume(&path).ok())
                        {
                            app.tutorial = None;
                            app.game = keep_flags(fresh, &app.game);
                            app.game.paused = app.args.paused;
                            app.input.reset_session();
                            next = Some(Screen::Playing);
                        } else {
                            app.game.toast("that save no longer loads");
                        }
                    }
                    screens::home::Out::Play => {
                        next = Some(Screen::Wizard(Wizard::open(&app.draft)));
                    }
                    screens::home::Out::Tutorial => {
                        // The tutorial is a gentle real match with the
                        // lesson cards riding on top.
                        let fresh = Game::new(tutorial::tutorial_scenario())?;
                        app.game = keep_flags(fresh, &app.game);
                        app.game.paused = app.args.paused;
                        app.tutorial = Some(tutorial::Tutorial::new());
                        app.input.reset_session();
                        next = Some(Screen::Playing);
                    }
                    screens::home::Out::Replays => {
                        next = Some(Screen::Replays(Shelf::open()));
                    }
                    screens::home::Out::Quit => match autosave::save(&mut app.game) {
                        Ok(_) => std::process::exit(0),
                        Err(err) => {
                            // Exiting anyway would be silent data loss:
                            // the failure dialog holds the door.
                            next = Some(Screen::Pause(PauseScreen::open_save_failed(
                                err.player_line(),
                                screens::pause::LeaveVerb::Quit,
                                app.game.state.result().is_some(),
                                can_surrender(&app.game),
                                true,
                            )));
                        }
                    },
                }
                render::draw(&app.game, &app.sprites, &app.input);
                veil();
                home.menu.draw(home.subtitle());
                if out == screens::home::Out::Settings {
                    Screen::Settings {
                        screen: SettingsScreen::open(&app.config),
                        back: Box::new(Screen::Home(home)),
                    }
                } else {
                    next.unwrap_or(Screen::Home(home))
                }
            }
            Screen::Settings {
                screen: mut sc,
                back,
            } => {
                let up = sc.update(
                    &events,
                    &mut app.input.mouse,
                    &mut app.game.sounds_pending,
                    &mut app.config,
                    &mut app.input.bindings,
                    ctrl_at_frame_start,
                    shift_at_frame_start,
                );
                if up.dirty
                    && let Err(err) = app.config.save()
                {
                    app.menu_notice =
                        Some((format!("could not save settings: {err}"), get_time() + 5.0));
                }
                render::draw(&app.game, &app.sprites, &app.input);
                veil();
                sc.draw();
                if up.out == screens::settings::Out::Leave {
                    // Back to wherever this screen displaced: Home, or
                    // the untouched pause menu still waiting on its
                    // Settings row.
                    *back
                } else {
                    Screen::Settings { screen: sc, back }
                }
            }
            Screen::Wizard(mut w) => {
                // Wizard trouble — an unreadable map file, a scenario
                // that fails validation — is a dialog problem, never a
                // process abort: report and stay on the menu.
                let out = match w.update(
                    &events,
                    &mut app.input.mouse,
                    &mut app.draft,
                    &mut app.game.sounds_pending,
                ) {
                    Ok(out) => out,
                    Err(err) => {
                        app.menu_notice =
                            Some((format!("can't open that map: {err:#}"), get_time() + 5.0));
                        WizardOut::Stay
                    }
                };
                let mut next: Option<Screen> = None;
                match out {
                    WizardOut::Home => {
                        let home = HomeScreen::open();
                        render::draw(&app.game, &app.sprites, &app.input);
                        veil();
                        home.menu.draw(home.subtitle());
                        rerun = true;
                        next = Some(Screen::Home(home));
                    }
                    WizardOut::Launch => match launch(&app.draft) {
                        Ok(fresh) => {
                            app.tutorial = None;
                            app.game = keep_flags(fresh, &app.game);
                            app.game.paused = app.args.paused;
                            app.input.reset_session();
                            render::draw(&app.game, &app.sprites, &app.input);
                            rerun = true;
                            next = Some(Screen::Playing);
                        }
                        Err(err) => {
                            app.menu_notice = Some((
                                format!("can't start that match: {err:#}"),
                                get_time() + 5.0,
                            ));
                        }
                    },
                    WizardOut::Stay => {}
                }
                if let Some(next) = next {
                    next
                } else {
                    render::draw(&app.game, &app.sprites, &app.input);
                    veil();
                    match w.step {
                        WizardStep::Map => w.browser.draw(&w.entries, &mut app.previews),
                        WizardStep::Setup => w.draw_setup(&app.draft, &mut app.previews),
                    }
                    Screen::Wizard(w)
                }
            }
            Screen::Playing => {
                // The tutorial card is chrome: a click on it (the
                // dismiss box included) must never reach the world —
                // it once deselected armies and even placed buildings.
                if let Some(t) = &app.tutorial {
                    let dismiss = render::tutorial_dismiss_rect();
                    let card = render::tutorial_card_rect(t);
                    if events.iter().any(|e| {
                        matches!(e, RawEvent::MouseDown { button: MouseButton::Left, x, y }
                            if dismiss.contains(vec2(*x, *y)))
                    }) {
                        app.tutorial = None;
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
                        app.input.drag_origin = None;
                    }
                }
                let had_selection = !app.game.selection.units.is_empty()
                    || !app.game.selection.buildings.is_empty();
                let escape_pressed = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                app.input.ui = render::ui_scale();
                app.input.now = get_time();
                app.input.camera_prefs = app.config.camera;
                app.input.touch_prefs = app.config.touch;
                input::apply_events(&mut app.game, &mut app.input, &events);
                input::update_held(&mut app.game, &app.input, dt);
                input::update_touch(&mut app.game, &mut app.input);
                // The cursor telegraphs the verb: crosshair while
                // placing or plotting, pointer over chrome.
                macroquad::miniquad::window::set_mouse_cursor(input::desired_cursor(
                    &app.game, &app.input,
                ));
                // Escape walks outward: deselect first, then the menu —
                // except over a decided match (or the concede overlay),
                // where the banner promises 'Press Esc to continue' and
                // must mean it even with a selection still alive.
                let mut next: Option<Screen> = None;
                if escape_pressed
                    && (!had_selection
                        || app.game.state.result().is_some()
                        || app.game.conceded_banner)
                {
                    // Opening the menu dismisses the concede overlay for
                    // good — Resume from here is clean spectating.
                    app.game.conceded_banner = false;
                    app.game.paused = true;
                    app.game.demo.paused_menu = true;
                    next = Some(Screen::Pause(PauseScreen::open(
                        app.game.state.result().is_some(),
                        can_surrender(&app.game),
                    )));
                }
                if let Some(t) = app.tutorial.as_mut() {
                    if !t.advance(&app.game.demo) {
                        app.tutorial = None;
                    } else {
                        // A click on the card's dismiss box ends school.
                        let dismiss = render::tutorial_dismiss_rect();
                        if events.iter().any(|e| {
                            matches!(e, RawEvent::MouseDown { button: MouseButton::Left, x, y }
                                if dismiss.contains(vec2(*x, *y)))
                        }) {
                            app.tutorial = None;
                        }
                    }
                }
                app.game.advance_wall_clock(dt);
                app.game.update_wall_clock_fx(dt);
                if app.game.state.result().is_some() && app.game.end_stats.is_none() {
                    // One re-execution of the record at match end; the
                    // sim replays thousands of ticks per second, so the
                    // hitch hides inside the banner's arrival.
                    let mut replay = app.game.recorder.clone();
                    let total = app.game.state.current_tick();
                    replay.meta.ticks = Some(total);
                    app.game.end_stats =
                        oxide_kit::stats::compute(&replay, (total / 48).max(1)).ok();
                }
                if app.game.state.result().is_some() && app.game.end_stats.is_some() {
                    next = Some(Screen::Results(ResultsScreen::open()));
                    // Re-enter immediately so the first decided frame is
                    // the report, not a bare frozen battlefield.
                    rerun = true;
                }
                render::draw(&app.game, &app.sprites, &app.input);
                if let Some(t) = &app.tutorial {
                    render::draw_tutorial(t, &app.game);
                }
                next.unwrap_or(Screen::Playing)
            }
            Screen::Playback(mut pb) => {
                let leave = pb.update(
                    &events,
                    dt,
                    vec2(screen_width(), screen_height()),
                    app.config.camera.zoom_inverted,
                    app.config.camera.pan_speed,
                    &mut app.input.mouse,
                );
                if leave {
                    rerun = true;
                    match pb.return_to {
                        PlaybackReturn::Pause => Screen::Pause(PauseScreen::open(
                            app.game.state.result().is_some(),
                            can_surrender(&app.game),
                        )),
                        PlaybackReturn::Results => Screen::Results(ResultsScreen::open()),
                        PlaybackReturn::Home => Screen::Home(HomeScreen::open()),
                    }
                } else {
                    render::draw(&pb.game, &app.sprites, &app.input);
                    screens::playback::playback_hud(&pb, vec2(screen_width(), screen_height()));
                    Screen::Playback(pb)
                }
            }
            Screen::Results(mut results) => {
                let out = results.update(
                    &events,
                    &mut app.input.mouse,
                    vec2(screen_width(), screen_height()),
                    render::ui_scale(),
                    &mut app.game.sounds_pending,
                );
                render::draw(&app.game, &app.sprites, &app.input);
                results.draw(&app.game);
                match out {
                    screens::results::Out::Stay => Screen::Results(results),
                    screens::results::Out::Rematch => match autosave::save(&mut app.game) {
                        Ok(_) => {
                            let fresh = Game::new(app.game.scenario.clone())?;
                            app.game = keep_flags(fresh, &app.game);
                            app.game.paused = app.args.paused;
                            app.tutorial = None;
                            app.input.reset_session();
                            rerun = true;
                            Screen::Playing
                        }
                        Err(err) => {
                            app.menu_notice = Some((
                                format!("cannot save result: {}", err.player_line()),
                                get_time() + 5.0,
                            ));
                            Screen::Results(results)
                        }
                    },
                    screens::results::Out::Watch => match result_playback(&app.game, false) {
                        Ok(session) => {
                            rerun = true;
                            Screen::Playback(Box::new(session))
                        }
                        Err(err) => {
                            app.menu_notice =
                                Some((format!("cannot open playback: {err}"), get_time() + 5.0));
                            Screen::Results(results)
                        }
                    },
                    screens::results::Out::ViewFinalMap => match result_playback(&app.game, true) {
                        Ok(session) => {
                            rerun = true;
                            Screen::Playback(Box::new(session))
                        }
                        Err(err) => {
                            app.menu_notice =
                                Some((format!("cannot open final map: {err}"), get_time() + 5.0));
                            Screen::Results(results)
                        }
                    },
                    screens::results::Out::Home => match autosave::save(&mut app.game) {
                        Ok(_) => {
                            rerun = true;
                            Screen::Home(HomeScreen::open())
                        }
                        Err(err) => Screen::Pause(PauseScreen::open_save_failed(
                            err.player_line(),
                            screens::pause::LeaveVerb::MainMenu,
                            true,
                            false,
                            false,
                        )),
                    },
                }
            }
            Screen::Replays(mut shelf) => {
                let mut leave: Option<Screen> = None;
                match shelf.update(&events, &mut app.input.mouse, &mut app.game.sounds_pending) {
                    screens::shelf::Out::Home => {
                        let home = HomeScreen::open();
                        render::draw(&app.game, &app.sprites, &app.input);
                        veil();
                        home.menu.draw(home.subtitle());
                        rerun = true;
                        leave = Some(Screen::Home(home));
                    }
                    screens::shelf::Out::Watch(path) => {
                        match PlaybackSession::open(&path.to_string_lossy()) {
                            Ok(session) => {
                                render::draw(&app.game, &app.sprites, &app.input);
                                rerun = true;
                                leave = Some(Screen::Playback(Box::new(session)));
                            }
                            Err(_) => {
                                app.game.sounds_pending.push((SoundKind::Denied, None));
                            }
                        }
                    }
                    screens::shelf::Out::Load(path) => match resume(&path) {
                        // The same loader Continue uses, so the two
                        // verbs cannot drift apart.
                        Ok(fresh) => {
                            app.tutorial = None;
                            app.game = keep_flags(fresh, &app.game);
                            app.game.paused = app.args.paused;
                            app.input.reset_session();
                            render::draw(&app.game, &app.sprites, &app.input);
                            rerun = true;
                            leave = Some(Screen::Playing);
                        }
                        Err(_) => {
                            app.game.sounds_pending.push((SoundKind::Denied, None));
                        }
                    },
                    screens::shelf::Out::Deleted => {
                        // Re-list; Home re-evaluates its Continue row on
                        // the way out, since every exit rebuilds it.
                        shelf = Shelf::open();
                    }
                    screens::shelf::Out::Stay => {}
                }
                if let Some(next) = leave {
                    next
                } else {
                    render::draw(&app.game, &app.sprites, &app.input);
                    veil();
                    shelf.menu.draw(&shelf.subtitle());
                    Screen::Replays(shelf)
                }
            }
            Screen::Pause(mut ps) => {
                let out = ps.update(&events, &mut app.input.mouse, &mut app.game.sounds_pending);
                render::draw(&app.game, &app.sprites, &app.input);
                veil();
                ps.menu.draw(ps.subtitle(&app.game.scenario.name));
                match out {
                    screens::pause::Out::Stay => Screen::Pause(ps),
                    screens::pause::Out::Resume => {
                        app.game.paused = false;
                        Screen::Playing
                    }
                    screens::pause::Out::SaveGame => {
                        // Only the session knows its map and tick; the
                        // screen just edits the string.
                        let suggested = format!(
                            "{} | t{}",
                            app.game.scenario.name,
                            app.game.state.current_tick()
                        );
                        ps.begin_naming(suggested);
                        Screen::Pause(ps)
                    }
                    screens::pause::Out::Save(name) => {
                        // Stay paused either way: the player may want to
                        // save and then quit.
                        let verdict = match autosave::save_named(&app.game, &name) {
                            Ok(_) => format!("saved: {name}"),
                            Err(err) => err.player_line(),
                        };
                        ps.end_naming(verdict);
                        Screen::Pause(ps)
                    }
                    screens::pause::Out::Settings => {
                        // The pause payload rides along intact: leaving
                        // Settings lands back on this exact menu, cursor
                        // still on the row that opened it. The sim stays
                        // frozen — neither screen ever advances the wall
                        // clock.
                        Screen::Settings {
                            screen: SettingsScreen::open(&app.config),
                            back: Box::new(Screen::Pause(ps)),
                        }
                    }
                    screens::pause::Out::Surrender => {
                        // The command lands on the next tick like any
                        // other. A 1v1 decides on the spot and the
                        // normal result flow takes over; in a team game
                        // the concede overlay meets the player back in
                        // the match while the ally plays on.
                        app.game.issue(oxide_sim::Command::Surrender);
                        app.game.paused = false;
                        Screen::Playing
                    }
                    screens::pause::Out::WatchReplay => {
                        // The recorder IS the record — clone it, stamp
                        // its length, play it back. Non-destructive; the
                        // live match waits.
                        let mut replay = app.game.recorder.clone();
                        replay.meta.ticks = Some(app.game.state.current_tick());
                        match PlaybackSession::from_replay(replay) {
                            Ok(mut session) => {
                                session.return_to = PlaybackReturn::Pause;
                                Screen::Playback(Box::new(session))
                            }
                            Err(err) => {
                                app.game.toast(format!("cannot open playback: {err}"));
                                Screen::Pause(ps)
                            }
                        }
                    }
                    screens::pause::Out::Restart => {
                        let fresh = Game::new(app.game.scenario.clone())?;
                        // Restarting a tutorial restarts the lessons —
                        // discarding them turned it into a plain match.
                        if app.tutorial.is_some() {
                            app.tutorial = Some(tutorial::Tutorial::new());
                        }
                        app.game = keep_flags(fresh, &app.game);
                        app.game.paused = app.args.paused;
                        app.input.reset_session();
                        Screen::Playing
                    }
                    screens::pause::Out::MainMenu => match autosave::save(&mut app.game) {
                        Ok(_) => Screen::Home(HomeScreen::open()),
                        Err(err) => Screen::Pause(PauseScreen::open_save_failed(
                            err.player_line(),
                            screens::pause::LeaveVerb::MainMenu,
                            app.game.state.result().is_some(),
                            can_surrender(&app.game),
                            false,
                        )),
                    },
                    screens::pause::Out::Quit => match autosave::save(&mut app.game) {
                        Ok(_) => std::process::exit(0),
                        Err(err) => Screen::Pause(PauseScreen::open_save_failed(
                            err.player_line(),
                            screens::pause::LeaveVerb::Quit,
                            app.game.state.result().is_some(),
                            can_surrender(&app.game),
                            false,
                        )),
                    },
                    screens::pause::Out::RetrySave(verb) => match autosave::save(&mut app.game) {
                        Ok(_) => match verb {
                            screens::pause::LeaveVerb::MainMenu => Screen::Home(HomeScreen::open()),
                            screens::pause::LeaveVerb::Quit => std::process::exit(0),
                        },
                        Err(err) => {
                            // The dialog stays up; only the reason may
                            // have changed.
                            ps.set_save_failure_line(err.player_line());
                            Screen::Pause(ps)
                        }
                    },
                    screens::pause::Out::LeaveUnsaved(verb) => match verb {
                        screens::pause::LeaveVerb::MainMenu => Screen::Home(HomeScreen::open()),
                        screens::pause::LeaveVerb::Quit => std::process::exit(0),
                    },
                    screens::pause::Out::Home => Screen::Home(HomeScreen::open()),
                }
            }
        };
        if rerun {
            continue;
        }

        // The menu-context error line draws over whichever menu is up;
        // the gameplay screens speak through the HUD's toast strip.
        if !matches!(screen, Screen::Playing | Screen::Playback(_))
            && let Some((msg, until)) = &app.menu_notice
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

        // The mixer serves whichever session is on screen: a playback
        // viewer queues its own sounds on its own game, and draining the
        // hidden match instead left replays silent while its queue grew.
        let (queued, cam_center, cam_half_w): (Vec<(SoundKind, Option<Vec2>)>, Vec2, f32) =
            match &mut screen {
                Screen::Playback(pb) => (
                    pb.game.sounds_pending.drain(..).collect(),
                    pb.game.camera.center,
                    pb.game.camera.viewport().x / pb.game.camera.zoom * 0.5,
                ),
                _ => (
                    app.game.sounds_pending.drain(..).collect(),
                    app.game.camera.center,
                    app.game.camera.viewport().x / app.game.camera.zoom * 0.5,
                ),
            };
        let combat_impulse = queued.iter().any(|(kind, _)| raises_combat_music(*kind));
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
            app.mixer
                .play(&app.sounds, kind, &app.config.volumes, attenuation);
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
                    let over_a_match = match &screen {
                        Screen::Playing | Screen::Results(_) | Screen::Pause(_) => true,
                        Screen::Settings { back, .. } => matches!(**back, Screen::Pause(_)),
                        Screen::Playback(pb) => matches!(
                            pb.return_to,
                            PlaybackReturn::Pause | PlaybackReturn::Results
                        ),
                        Screen::Home(_) | Screen::Wizard(_) | Screen::Replays(_) => false,
                    };
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

/// The mutating verbs a read-only viewer refuses wholesale — commands
/// and session swaps would act through the replay onto the hidden match
/// behind it.
fn viewer_refuses(request: &Request) -> bool {
    matches!(
        request,
        Request::SendCommand { .. }
            | Request::LoadScenario { .. }
            | Request::LoadReplay { .. }
            | Request::SaveReplay { .. }
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
fn handle_request(incoming: IncomingRequest, app: &mut App, screen: &mut Screen, ui_view: &UiView) {
    let IncomingRequest { id, request, reply } = incoming;
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
                Screen::Settings { back, .. } => matches!(**back, Screen::Pause(_)),
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
    if matches!(&*screen, Screen::Playback(_)) && viewer_refuses(&request) {
        let refusal = "the viewer is read-only; leave playback first".to_string();
        reply.send(ResponseEnvelope::err(id, refusal)).ok();
        return;
    }
    let game = &mut app.game;
    let outcome: Result<Reply, String> = match request {
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
                assert_eq!(
                    config.style,
                    Some(oxide_sim::bot::NamedStyle::Turtle),
                    "the seat's OWN dials"
                );
                assert_eq!(config.aggression, None, "named styles are not raw knobs");
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
    fn result_viewers_return_to_the_report_and_final_map_starts_frozen() {
        let game = Game::new(Scenario::skirmish()).expect("game");

        let watch = result_playback(&game, false).expect("watch replay");
        assert_eq!(watch.return_to, PlaybackReturn::Results);
        assert!(!watch.paused);
        assert!(watch.seeking.is_none());

        let final_map = result_playback(&game, true).expect("view final map");
        assert_eq!(final_map.return_to, PlaybackReturn::Results);
        assert!(final_map.paused);
        assert_eq!(final_map.seeking, Some(final_map.engine.total()));
        assert!(final_map.game.recorder.commands.is_empty());
    }
}
