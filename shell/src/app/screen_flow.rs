//! Cross-screen transitions and drawing for one presented frame.

use super::*;

pub(super) struct ScreenFrame {
    pub(super) screen: Screen,
    pub(super) rerun: bool,
    pub(super) profile_frame_active: bool,
}

/// Whether this screen owns a decorative backdrop whose presentation clock
/// should keep moving. A menu opened from Pause is deliberately different
/// from the same menu opened from Home: the paused battlefield must stay
/// frozen behind it.
fn backdrop_fx_advances(screen: &Screen) -> bool {
    match screen {
        Screen::Home(_)
        | Screen::Wizard(_)
        | Screen::Replays(_)
        | Screen::Results(_)
        | Screen::Lobby { .. } => true,
        Screen::Settings { back, .. } | Screen::Codex { back, .. } => {
            !matches!(**back, Screen::Pause(_))
        }
        Screen::Playing
        | Screen::Playback(_)
        | Screen::FinalMap(_)
        | Screen::Pause(_)
        | Screen::Busy(_) => false,
    }
}

/// Escape clears a live selection before it opens Pause. A decided match and
/// the concession banner are terminal overlays, so their advertised Escape
/// action wins even when a selection survived underneath.
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "a predicate over four independent facts"
)]
fn playing_escape_opens_pause(
    escape_pressed: bool,
    had_selection: bool,
    decided: bool,
    conceded_banner: bool,
) -> bool {
    escape_pressed && (!had_selection || decided || conceded_banner)
}

/// Why the pause menu opened over a live match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PauseCause {
    /// Escape or the menu button.
    Player,
    /// The app stopped presenting frames for a while (a suspended iPad
    /// app, a sleeping Mac) and would otherwise resume live.
    Suspension,
}

/// A frame this long means the app was not running rather than merely
/// slow: no ordinary hitch comes close.
const SUSPENSION_GAP_SECS: f32 = 2.0;

/// Whether this frame's wall time is a suspension that should land on
/// the pause menu. Frame time is reported one frame late, so the live
/// streak must cover both the frame that measured the gap and the one
/// before it; that keeps startup, match launches, and loads (heavy
/// frames on other screens) from reading as suspensions. Debug-server
/// sessions are exempt because an agent may stall the loop on purpose.
fn gap_opens_pause(raw_dt: f32, live_streak: u8, running: bool, exempt: bool) -> bool {
    raw_dt >= SUSPENSION_GAP_SECS && live_streak >= 2 && running && !exempt
}

/// Freezes the live match under the pause menu. Opening the menu
/// dismisses the concede overlay for good, so Resume from here is clean
/// spectating. Only a player's choice teaches the tutorial's pause
/// lesson.
fn open_pause(game: &mut Game, cause: PauseCause) -> Screen {
    game.presentation.conceded_banner = false;
    let pause = pause_menu(game);
    if game.net_role().is_none() {
        game.presentation.paused = true;
    }
    Screen::Pause(match cause {
        PauseCause::Player => {
            game.demo.paused_menu = true;
            pause
        }
        PauseCause::Suspension => pause.with_notice("paused after an interruption"),
    })
}

/// Resolves the wizard's semantic outcome and owns the one seed-consumption
/// boundary. A failed launch leaves the current window available for retry;
/// successful construction consumes exactly one window before the game is
/// installed by the caller.
fn resolve_new_match(
    out: WizardOut,
    draft: &NewMatchDraft,
    personality_seeds: &mut PersonalitySeedSource,
    bind: &str,
) -> Option<Result<NewMatch>> {
    match out {
        WizardOut::Home | WizardOut::Stay => None,
        WizardOut::Launch => {
            let result = start_new_match(draft, personality_seeds.match_base(), bind);
            if result.is_ok() {
                personality_seeds.commit_launch();
            }
            Some(result)
        }
    }
}

/// Restart and Rematch both rebuild the exact recorded scenario. Keeping this
/// path independent of [`PersonalitySeedSource`] prevents either action from
/// silently becoming a new opponent roll.
fn rebuild_match(game: &Game) -> Result<Game> {
    Game::new(game.scenario.clone())
}

pub(super) fn update_and_draw(
    app: &mut App,
    mut screen: Screen,
    mut events: Vec<RawEvent>,
    time: FrameTime,
    ctrl_at_frame_start: bool,
    shift_at_frame_start: bool,
) -> Result<ScreenFrame> {
    // Coaching waits until the player seems stuck on this screen.
    let pressed = events.iter().any(crate::hints::is_press);
    crate::hints::set_alpha(app.hint_clock.observe(
        super::screen_mode(&screen),
        pressed,
        time.presentation,
        render::reduced_motion(),
    ));
    // Controls capture and text editing keep their conventional recovery keys.
    let fixed_editor = matches!(&screen, Screen::Settings { screen, .. } if matches!(screen.face, screens::settings::Face::Controls { .. }))
        || text_entry(&screen);
    crate::menu::set_bindings(if fixed_editor {
        crate::action::BindingMap::classic()
    } else {
        app.input.bindings.clone()
    });
    if !fixed_editor
        && !matches!(
            &screen,
            Screen::Playing | Screen::Playback(_) | Screen::FinalMap(_)
        )
    {
        events = app
            .input
            .bindings
            .menu_events(&events, ctrl_at_frame_start, shift_at_frame_start);
    }
    let mut profile_frame_active = false;
    // Menu backdrops are presentation worlds too. Home, setup, and
    // the replay shelf animate even when `--paused` reserves the next
    // match for driven control; Settings inherits its caller, so the
    // pause-menu path stays frozen. Playing and Playback advance their
    // own clocks below, while Pause deliberately advances neither.
    if backdrop_fx_advances(&screen) {
        app.game.update_fx(time.presentation);
    } else if app.net.is_some() && !matches!(screen, Screen::Playing) {
        // A LAN match keeps ticking under menus, so its effects age too.
        app.game.update_wall_clock_fx(time.presentation);
    }
    // A `rerun` transition re-enters the loop under the new screen before
    // presenting, so the frame shows the destination.
    let mut rerun = false;
    screen = match screen {
        Screen::Home(home) => home_frame(app, home, &events, time.presentation)?,
        Screen::Settings { screen: sc, back } => settings_frame(
            app,
            sc,
            back,
            &events,
            ctrl_at_frame_start,
            shift_at_frame_start,
        ),
        Screen::Codex {
            screen: codex,
            back,
        } => codex_frame(app, codex, back, &events),
        Screen::Wizard(w) => wizard_frame(app, w, &events, &mut rerun),
        Screen::Lobby {
            screen: lobby,
            back,
        } => lobby_frame(app, lobby, back, &events, &mut rerun),
        Screen::Playing => playing_frame(
            app,
            &mut events,
            time,
            &mut rerun,
            &mut profile_frame_active,
            ctrl_at_frame_start,
            shift_at_frame_start,
        ),
        Screen::Playback(pb) => playback_frame(app, pb, &events, time, &mut rerun),
        Screen::FinalMap(final_map) => {
            final_map_frame(app, final_map, &events, time.presentation, &mut rerun)
        }
        Screen::Results(results) => results_frame(app, results, &events, &mut rerun),
        Screen::Replays(shelf) => replays_frame(app, shelf, &events, &mut rerun),
        Screen::Pause(ps) => pause_frame(app, ps, &events)?,
        Screen::Busy(busy) => persistence::frame(app, busy, &events)?,
    };

    Ok(ScreenFrame {
        screen,
        rerun,
        profile_frame_active,
    })
}

/// A LAN match gathering: polls the lobby and installs the match on Go.
fn lobby_frame(
    app: &mut App,
    mut lobby: Box<crate::screens::lobby::LobbyScreen>,
    back: Box<Screen>,
    events: &[RawEvent],
    rerun: &mut bool,
) -> Screen {
    if let Some((game, link)) = lobby.poll(app.clock.elapsed(), render::viewport()) {
        app.install_networked(game, link);
        *rerun = true;
        return Screen::Playing;
    }
    match lobby.update(
        events,
        &mut app.input.mouse,
        &mut app.game.presentation.sounds_pending,
    ) {
        crate::screens::lobby::Out::Stay => {}
        crate::screens::lobby::Out::Cancel => return *back,
        crate::screens::lobby::Out::Join(address) => {
            app.config.last_join_address = Some(address.clone());
            if let Err(err) = app.config.save() {
                app.menu_notice =
                    Some((format!("could not save settings: {err}"), get_time() + 5.0));
            }
            let commit = crate::build_identity().revision;
            *lobby = crate::screens::lobby::LobbyScreen::waiting(crate::netplay::Lobby::Client(
                crate::netplay::ClientLobby::new(&address, &commit),
            ));
        }
    }
    render::draw(&app.game.view(), &app.sprites, &app.input);
    veil();
    lobby.draw(app.input.mouse);
    Screen::Lobby {
        screen: lobby,
        back,
    }
}

fn home_frame(app: &mut App, mut home: HomeScreen, events: &[RawEvent], dt: f32) -> Result<Screen> {
    if !home.catalog_ready {
        app.request_catalog();
    }
    // The title scene: a cold front door drifts its camera
    // slowly across the backdrop world instead of freezing
    // a frame — presentation only, and only while nothing
    // is at stake (a resumable match keeps its exact view).
    if app.game.state.current_tick() == 0 && !render::reduced_motion() {
        app.game.presentation.camera.pan(vec2(dt * 0.55, dt * 0.22));
        let (_, hi) = app.game.presentation.camera.world_rect();
        if hi.x >= app.game.state.map().width() as f32 + 1.9 {
            app.game.presentation.camera.center = vec2(0.0, 0.0);
            app.game.presentation.camera.pan(vec2(0.0, 0.0)); // re-clamp home
        }
    }
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let out = home.update(
        events,
        &mut app.input.mouse,
        &mut app.game.presentation.sounds_pending,
    );
    drop(input_scope);
    // Session verbs first — Continue and Tutorial swap the
    // game this frame then draws under the menu. The menu
    // draw needs `home`, so verbs that displace it (only
    // Settings) build their screen after the draw below.
    let mut next: Option<Screen> = None;
    match out {
        screens::home::Out::Stay
        | screens::home::Out::Settings
        | screens::home::Out::Roster
        | screens::home::Out::Join => {}
        screens::home::Out::Recover => {
            if let Some(record) = &home.recovery {
                let path = record.directory.clone();
                return Ok(
                    app.persistence_screen(persistence::Intent::Recover(path), Screen::Home(home))
                );
            }
        }
        screens::home::Out::Continue => {
            return Ok(app.persistence_screen(persistence::Intent::Continue, Screen::Home(home)));
        }
        screens::home::Out::Play => {
            next = Some(Screen::Wizard(Wizard::open(&app.draft)));
        }
        screens::home::Out::Tutorial => {
            // The tutorial is a gentle real match with the
            // lesson cards riding on top.
            let fresh = Game::new(tutorial::tutorial_scenario())?;
            app.install_session(fresh, app.args.paused, Some(tutorial::Tutorial::new()));
            next = Some(Screen::Playing);
        }
        screens::home::Out::Replays => {
            next = Some(Screen::Replays(Shelf::open()));
        }
        screens::home::Out::Quit => {
            return Ok(app.persistence_screen(
                persistence::Intent::Leave(screens::pause::LeaveVerb::Quit, true),
                Screen::Home(home),
            ));
        }
    }
    render::draw(&app.game.view(), &app.sprites, &app.input);
    veil();
    home.menu.draw(home.subtitle());
    Ok(if out == screens::home::Out::Settings {
        Screen::Settings {
            screen: SettingsScreen::open(&app.config),
            back: Box::new(Screen::Home(home)),
        }
    } else if out == screens::home::Out::Roster {
        Screen::Codex {
            screen: CodexScreen::open(),
            back: Box::new(Screen::Home(home)),
        }
    } else if out == screens::home::Out::Join {
        let address = app.config.last_join_address.as_deref().unwrap_or_default();
        Screen::Lobby {
            screen: Box::new(crate::screens::lobby::LobbyScreen::address(address)),
            back: Box::new(Screen::Home(home)),
        }
    } else {
        next.unwrap_or(Screen::Home(home))
    })
}

fn settings_frame(
    app: &mut App,
    mut sc: SettingsScreen,
    back: Box<Screen>,
    events: &[RawEvent],
    ctrl_at_frame_start: bool,
    shift_at_frame_start: bool,
) -> Screen {
    if sc.notice.is_none()
        && let Some(error) = app
            .game
            .recovery
            .as_ref()
            .and_then(|writer| writer.status().error)
    {
        sc.notice = Some(screens::settings::Notice {
            text: format!("Recovery stopped: {error}"),
            danger: true,
        });
    }
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let up = sc.update(
        events,
        &mut app.input.mouse,
        &mut app.game.presentation.sounds_pending,
        &mut app.config,
        &mut app.input.bindings,
        ctrl_at_frame_start,
        shift_at_frame_start,
    );
    drop(input_scope);
    match up.out {
        screens::settings::Out::OpenDiagnostics => {
            if let Err(error) = app.report_job.open_folder() {
                sc.notice = Some(screens::settings::Notice {
                    text: error.to_string(),
                    danger: true,
                });
            }
        }
        screens::settings::Out::ExportDiagnostics => {
            sc.notice = Some(match app.report_job.start(&app.game) {
                Ok(()) => screens::settings::Notice {
                    text: "Exporting diagnostic report...".into(),
                    danger: false,
                },
                Err(error) => screens::settings::Notice {
                    text: error.to_string(),
                    danger: true,
                },
            });
        }
        _ => {}
    }
    if up.dirty
        && let Err(err) = app.config.save()
    {
        app.menu_notice = Some((format!("could not save settings: {err}"), get_time() + 5.0));
    }
    render::draw(&app.game.view(), &app.sprites, &app.input);
    veil();
    sc.draw();
    crate::button::draw_back(app.input.mouse);
    if up.out == screens::settings::Out::Leave {
        // Back to wherever this screen displaced: Home, or
        // the untouched pause menu still waiting on its
        // Settings row.
        *back
    } else {
        Screen::Settings { screen: sc, back }
    }
}

fn codex_frame(
    app: &mut App,
    mut codex: CodexScreen,
    back: Box<Screen>,
    events: &[RawEvent],
) -> Screen {
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let out = codex.update(
        events,
        &mut app.input.mouse,
        &mut app.game.presentation.sounds_pending,
    );
    drop(input_scope);
    render::draw(&app.game.view(), &app.sprites, &app.input);
    veil();
    let viewer = app.game.state.player(app.game.presentation.human).faction;
    codex.draw(&app.sprites, viewer);
    crate::button::draw_back(app.input.mouse);
    if out == screens::codex::Out::Leave {
        *back
    } else {
        Screen::Codex {
            screen: codex,
            back,
        }
    }
}

fn wizard_frame(app: &mut App, mut w: Wizard, events: &[RawEvent], rerun: &mut bool) -> Screen {
    // Wizard trouble — an unreadable map file, a scenario
    // that fails validation — is a dialog problem, never a
    // process abort: report and stay on the menu.
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let out = match w.update(
        events,
        &mut app.input.mouse,
        &mut app.draft,
        &mut app.game.presentation.sounds_pending,
    ) {
        Ok(out) => out,
        Err(err) => {
            app.menu_notice = Some((format!("can't open that map: {err:#}"), get_time() + 5.0));
            WizardOut::Stay
        }
    };
    let mut next: Option<Screen> = None;
    drop(input_scope);
    let bind = format!("0.0.0.0:{}", oxide_net::DEFAULT_PORT);
    let launch_result = resolve_new_match(out, &app.draft, &mut app.personality_seeds, &bind);
    match out {
        WizardOut::Home => {
            let home = HomeScreen::open();
            render::draw(&app.game.view(), &app.sprites, &app.input);
            veil();
            home.menu.draw(home.subtitle());
            *rerun = true;
            next = Some(Screen::Home(home));
        }
        WizardOut::Launch => match launch_result.expect("launch outcome has a result") {
            Ok(NewMatch::Local(fresh)) => {
                app.install_session(*fresh, app.args.paused, None);
                render::draw(&app.game.view(), &app.sprites, &app.input);
                *rerun = true;
                next = Some(Screen::Playing);
            }
            Ok(NewMatch::Hosted(lobby)) => {
                *rerun = true;
                return Screen::Lobby {
                    screen: Box::new(crate::screens::lobby::LobbyScreen::waiting(
                        crate::netplay::Lobby::Host(lobby),
                    )),
                    back: Box::new(Screen::Wizard(w)),
                };
            }
            Err(err) => {
                app.menu_notice =
                    Some((format!("can't start that match: {err:#}"), get_time() + 5.0));
            }
        },
        WizardOut::Stay => {}
    }
    if let Some(next) = next {
        next
    } else {
        render::draw(&app.game.view(), &app.sprites, &app.input);
        veil();
        match w.step {
            WizardStep::Map => w.browser.draw(&w.entries, &mut app.previews),
            WizardStep::Setup => w.draw_setup(&app.draft, &mut app.previews),
        }
        crate::button::draw_back(app.input.mouse);
        Screen::Wizard(w)
    }
}

/// What the tutorial card did with a frame's pointer events.
#[derive(Debug, Default, PartialEq)]
struct TutorialPointer {
    /// A press landed on the dismiss box.
    dismissed: bool,
    /// A mouse release landed on the card and was removed.
    swallowed_release: bool,
}

/// The tutorial card is chrome: presses on it never reach the world or
/// consume an armed gameplay action, and a press on its dismiss box
/// ends the tutorial. A finger landing on the card is dropped before
/// the gesture funnel sees it, so its later moves and lift belong to no
/// touch; the dismiss box takes the padded touch target.
fn filter_tutorial_pointer(
    events: &mut Vec<RawEvent>,
    card: macroquad::math::Rect,
    dismiss: macroquad::math::Rect,
    ui: f32,
) -> TutorialPointer {
    let touch_dismiss = crate::layout::touch_pad(dismiss, ui);
    let mut filtered = TutorialPointer::default();
    events.retain(|e| match *e {
        RawEvent::MouseDown { button, x, y } if card.contains(vec2(x, y)) => {
            filtered.dismissed |= button == MouseButton::Left && dismiss.contains(vec2(x, y));
            false
        }
        RawEvent::MouseUp { x, y, .. } if card.contains(vec2(x, y)) => {
            filtered.swallowed_release = true;
            false
        }
        RawEvent::TouchDown { x, y, .. }
            if card.contains(vec2(x, y)) || touch_dismiss.contains(vec2(x, y)) =>
        {
            filtered.dismissed |= touch_dismiss.contains(vec2(x, y));
            false
        }
        _ => true,
    });
    filtered
}

fn playing_frame(
    app: &mut App,
    events: &mut Vec<RawEvent>,
    time: FrameTime,
    rerun: &mut bool,
    profile_frame_active: &mut bool,
    ctrl_at_frame_start: bool,
    shift_at_frame_start: bool,
) -> Screen {
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    if let Some(t) = &app.tutorial {
        let filtered = filter_tutorial_pointer(
            events,
            render::tutorial_card_rect(t),
            render::tutorial_dismiss_rect(),
            render::ui_scale(),
        );
        if filtered.dismissed {
            app.tutorial = None;
        }
        // Swallowing a release whose press began in the world must
        // also end that drag, or a later release completes it.
        if filtered.swallowed_release {
            app.input.drag_origin = None;
        }
    }
    let had_selection = !app.game.presentation.selection.units.is_empty()
        || !app.game.presentation.selection.buildings.is_empty();
    let mut ctrl = ctrl_at_frame_start;
    let mut shift = shift_at_frame_start;
    let escape_pressed = events.iter().any(|event| {
        match event {
            RawEvent::KeyDown { key: Key::Ctrl } => ctrl = true,
            RawEvent::KeyUp { key: Key::Ctrl } => ctrl = false,
            RawEvent::KeyDown { key: Key::Shift } => shift = true,
            RawEvent::KeyUp { key: Key::Shift } => shift = false,
            RawEvent::KeyDown { key } => {
                return app.input.bindings.resolve_in(
                    *key,
                    ctrl,
                    shift,
                    app.input.context(&app.game),
                ) == Some(crate::action::Action::Back);
            }
            _ => {}
        }
        false
    });
    app.input.ui = render::ui_scale();
    app.input.now = get_time();
    app.input.camera_prefs = app.config.camera;
    app.input.touch_prefs = app.config.touch;
    input::apply_events(&mut app.game, &mut app.input, events);
    input::update_held(&mut app.game, &app.input, time.presentation);
    input::update_touch(&mut app.game, &mut app.input);
    let menu_pressed = app.input.take_menu_request();
    // The cursor telegraphs the verb: crosshair while
    // placing or plotting, pointer over chrome.
    macroquad::miniquad::window::set_mouse_cursor(input::desired_cursor(&app.game, &app.input));
    // Escape walks outward: deselect first, then the menu —
    // except over a decided match (or the concede overlay),
    // where the banner promises 'Press Esc to continue' and
    // must mean it even with a selection still alive.
    // The menu button skips that walk: it names the menu outright.
    let mut next: Option<Screen> = None;
    if menu_pressed
        || playing_escape_opens_pause(
            escape_pressed,
            had_selection,
            app.game.state.result().is_some(),
            app.game.presentation.conceded_banner,
        )
    {
        next = Some(open_pause(&mut app.game, PauseCause::Player));
    } else if gap_opens_pause(
        time.raw,
        app.live_streak,
        !app.game.presentation.paused && app.game.state.result().is_none(),
        app.args.automation || app.args.debug_server || app.game.net_role().is_some(),
    ) {
        next = Some(open_pause(&mut app.game, PauseCause::Suspension));
    }
    if let Some(t) = app.tutorial.as_mut()
        && !t.advance(app.game.demo)
    {
        app.tutorial = None;
    }
    drop(input_scope);
    let profile_barrier = !app.game.presentation.paused && app.frame_profiler.take_start_barrier();
    *profile_frame_active = !app.game.presentation.paused && !profile_barrier;
    let profile_stopped = if profile_barrier {
        false
    } else {
        // A LAN match's link runs its ticks before the screen frame.
        let stopped = app.game.net_role().is_none()
            && app
                .game
                .advance_wall_clock(time.raw, app.frame_profiler.stop_tick());
        app.game.update_wall_clock_fx(time.presentation);
        stopped
    };
    if profile_stopped {
        app.game.presentation.paused = true;
    }
    if app.game.state.result().is_some() && app.game.end_stats.is_some() {
        next = Some(Screen::Results(ResultsScreen::open()));
        // Re-enter immediately so the first decided frame is
        // the report, not a bare frozen battlefield.
        *rerun = true;
    }
    render::draw_with_performance(
        &app.game.view(),
        &app.sprites,
        &app.input,
        Some(app.performance.view()),
    );
    if let Some(t) = &app.tutorial {
        render::draw_tutorial(t, &app.game, &app.input.bindings);
    }
    next.unwrap_or(Screen::Playing)
}

fn playback_frame(
    app: &mut App,
    mut pb: Box<PlaybackSession>,
    events: &[RawEvent],
    time: FrameTime,
    rerun: &mut bool,
) -> Screen {
    pb.bindings.clone_from(&app.input.bindings);
    let input_scope =
        oxide_kit::diagnostics::stage(oxide_kit::diagnostics::Stage::Input, pb.engine.position());
    let leave = pb.apply_input(
        events,
        time.presentation,
        vec2(screen_width(), screen_height()),
        app.config.camera.zoom_inverted,
        app.config.camera.pan_speed,
        &mut app.input.mouse,
    );
    drop(input_scope);
    if leave {
        *rerun = true;
        match pb.return_to {
            PlaybackReturn::Pause => Screen::Pause(pause_menu(&app.game)),
            PlaybackReturn::Results => Screen::Results(ResultsScreen::open()),
            PlaybackReturn::Home => Screen::Home(HomeScreen::open()),
        }
    } else {
        pb.advance_frame(time, vec2(screen_width(), screen_height()));
        render::draw_with_performance(
            &pb.view(),
            &app.sprites,
            &app.input,
            Some(app.performance.view()),
        );
        screens::playback::playback_hud(
            &pb,
            vec2(screen_width(), screen_height()),
            app.input.mouse,
        );
        Screen::Playback(pb)
    }
}

fn final_map_frame(
    app: &mut App,
    mut final_map: FinalMapScreen,
    events: &[RawEvent],
    dt: f32,
    rerun: &mut bool,
) -> Screen {
    final_map.bindings.clone_from(&app.input.bindings);
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let leave = final_map.update(
        events,
        dt,
        vec2(screen_width(), screen_height()),
        app.config.camera,
        &mut app.input.mouse,
        &mut app.game,
    );
    drop(input_scope);
    render::draw_with_performance(
        &app.game.view(),
        &app.sprites,
        &app.input,
        Some(app.performance.view()),
    );
    final_map.draw_hud(app.input.mouse);
    if leave {
        app.game.presentation.spectate = false;
        *rerun = true;
        Screen::Results(ResultsScreen::open())
    } else {
        Screen::FinalMap(final_map)
    }
}

fn results_frame(
    app: &mut App,
    mut results: ResultsScreen,
    events: &[RawEvent],
    rerun: &mut bool,
) -> Screen {
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let out = results.update(
        events,
        &mut app.input.mouse,
        vec2(screen_width(), screen_height()),
        render::ui_scale(),
        &mut app.game.presentation.sounds_pending,
    );
    drop(input_scope);
    render::draw(&app.game.view(), &app.sprites, &app.input);
    results.draw(&app.game);
    match out {
        screens::results::Out::Stay => Screen::Results(results),
        screens::results::Out::Rematch if app.game.net_role().is_some() => {
            app.menu_notice = Some((
                "Host a new match to play again.".to_owned(),
                get_time() + 5.0,
            ));
            Screen::Results(results)
        }
        screens::results::Out::Rematch => {
            app.persistence_screen(persistence::Intent::Rematch, Screen::Results(results))
        }
        screens::results::Out::Watch => match result_playback(&app.game) {
            Ok(session) => {
                *rerun = true;
                Screen::Playback(Box::new(session))
            }
            Err(err) => {
                app.menu_notice = Some((format!("cannot open playback: {err}"), get_time() + 5.0));
                Screen::Results(results)
            }
        },
        screens::results::Out::ViewFinalMap => {
            app.game.presentation.paused = true;
            app.game.presentation.spectate = true;
            app.game.presentation.selection.units.clear();
            app.game.presentation.selection.buildings.clear();
            app.game.presentation.selection.pile = None;
            *rerun = true;
            Screen::FinalMap(FinalMapScreen::open())
        }
        screens::results::Out::Home => app.persistence_screen(
            persistence::Intent::Leave(screens::pause::LeaveVerb::MainMenu, false),
            Screen::Results(results),
        ),
    }
}

fn replays_frame(app: &mut App, mut shelf: Shelf, events: &[RawEvent], rerun: &mut bool) -> Screen {
    if !shelf.catalog_ready {
        app.request_catalog();
    }
    let mut leave: Option<Screen> = None;
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let out = shelf.update(
        events,
        &mut app.input.mouse,
        &mut app.game.presentation.sounds_pending,
    );
    drop(input_scope);
    match out {
        screens::shelf::Out::Home => {
            let home = HomeScreen::open();
            render::draw(&app.game.view(), &app.sprites, &app.input);
            veil();
            home.menu.draw(home.subtitle());
            *rerun = true;
            leave = Some(Screen::Home(home));
        }
        screens::shelf::Out::Watch(path) => match PlaybackSession::open(&path.to_string_lossy()) {
            Ok(session) => {
                render::draw(&app.game.view(), &app.sprites, &app.input);
                *rerun = true;
                leave = Some(Screen::Playback(Box::new(session)));
            }
            Err(_) => {
                app.game
                    .presentation
                    .sounds_pending
                    .push((SoundKind::Denied, None));
            }
        },
        screens::shelf::Out::Load(path) => {
            return app.persistence_screen(persistence::Intent::Load(path), Screen::Replays(shelf));
        }
        screens::shelf::Out::Delete(path) => {
            app.catalog_delete = Some(path);
            shelf.catalog_ready = false;
        }
        screens::shelf::Out::Stay => {}
    }
    if let Some(next) = leave {
        next
    } else {
        render::draw(&app.game.view(), &app.sprites, &app.input);
        veil();
        shelf
            .menu
            .draw_with_coaching(&shelf.subtitle(), shelf.coaching().as_deref());
        crate::button::draw_back(app.input.mouse);
        Screen::Replays(shelf)
    }
}

fn pause_frame(app: &mut App, mut ps: PauseScreen, events: &[RawEvent]) -> Result<Screen> {
    let input_scope = app
        .game
        .diagnostic_stage(oxide_kit::diagnostics::Stage::Input);
    let out = ps.update(
        events,
        &mut app.input.mouse,
        &mut app.game.presentation.sounds_pending,
    );
    drop(input_scope);
    render::draw(&app.game.view(), &app.sprites, &app.input);
    veil();
    ps.draw(&app.game.scenario.name, app.input.mouse);
    Ok(match out {
        screens::pause::Out::Stay => Screen::Pause(ps),
        screens::pause::Out::Resume => {
            app.game.presentation.paused = false;
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
            ps.begin_naming(&suggested);
            Screen::Pause(ps)
        }
        screens::pause::Out::Save(name) => {
            app.persistence_screen(persistence::Intent::Named(name), Screen::Pause(ps))
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
        screens::pause::Out::Roster => Screen::Codex {
            screen: CodexScreen::open(),
            back: Box::new(Screen::Pause(ps)),
        },
        screens::pause::Out::Surrender => {
            // The command lands on the next tick like any
            // other. A 1v1 decides on the spot and the
            // normal result flow takes over; in a team game
            // the concede overlay meets the player back in
            // the match while the ally plays on.
            app.game.issue(oxide_sim::Command::Surrender);
            app.game.presentation.paused = false;
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
                    app.game
                        .presentation
                        .toast(format!("cannot open playback: {err}"));
                    Screen::Pause(ps)
                }
            }
        }
        screens::pause::Out::Restart => {
            let fresh = rebuild_match(&app.game)?;
            // Restarting a tutorial also restarts its lesson state.
            let tutorial = app.tutorial.is_some().then(tutorial::Tutorial::new);
            app.install_session(fresh, app.args.paused, tutorial);
            Screen::Playing
        }
        screens::pause::Out::MainMenu => app.persistence_screen(
            persistence::Intent::Leave(screens::pause::LeaveVerb::MainMenu, false),
            Screen::Pause(ps),
        ),
        screens::pause::Out::Quit => app.persistence_screen(
            persistence::Intent::Leave(screens::pause::LeaveVerb::Quit, false),
            Screen::Pause(ps),
        ),
        screens::pause::Out::RetrySave(verb, cancel_home) => app.persistence_screen(
            persistence::Intent::Leave(verb, cancel_home),
            Screen::Pause(ps),
        ),
        screens::pause::Out::LeaveUnsaved(verb) => match verb {
            screens::pause::LeaveVerb::MainMenu => Screen::Home(HomeScreen::open()),
            screens::pause::LeaveVerb::Quit => std::process::exit(0),
        },
        screens::pause::Out::Home => Screen::Home(HomeScreen::open()),
    })
}

#[cfg(test)]
mod tests;
