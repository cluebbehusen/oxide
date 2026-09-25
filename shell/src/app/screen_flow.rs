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
        Screen::Home(_) | Screen::Wizard(_) | Screen::Replays(_) | Screen::Results(_) => true,
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
fn playing_escape_opens_pause(
    escape_pressed: bool,
    had_selection: bool,
    decided: bool,
    conceded_banner: bool,
) -> bool {
    escape_pressed && (!had_selection || decided || conceded_banner)
}

/// Freezes the live match under the pause menu. Opening the menu
/// dismisses the concede overlay for good, so Resume from here is clean
/// spectating.
fn open_pause(game: &mut Game) -> Screen {
    game.presentation.conceded_banner = false;
    game.presentation.paused = true;
    game.demo.paused_menu = true;
    Screen::Pause(PauseScreen::open(
        game.state.result().is_some(),
        can_surrender(game),
    ))
}

/// Resolves the wizard's semantic outcome and owns the one seed-consumption
/// boundary. A failed launch leaves the current window available for retry;
/// successful construction consumes exactly one window before the game is
/// installed by the caller.
fn resolve_new_match(
    out: WizardOut,
    draft: &NewMatchDraft,
    personality_seeds: &mut PersonalitySeedSource,
) -> Option<Result<Game>> {
    match out {
        WizardOut::Home | WizardOut::Stay => None,
        WizardOut::Launch => {
            let result = launch(draft, personality_seeds.match_base());
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
    dt: f32,
    ctrl_at_frame_start: bool,
    shift_at_frame_start: bool,
) -> Result<ScreenFrame> {
    // Controls capture and text editing keep their conventional recovery keys.
    let fixed_editor = matches!(&screen, Screen::Settings { screen, .. } if matches!(screen.face, screens::settings::Face::Controls { .. }))
        || matches!(&screen, Screen::Pause(pause) if pause.naming());
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
        app.game.update_fx(dt);
    }
    // A `rerun` transition re-enters the loop under the new screen before
    // presenting, so the frame shows the destination.
    let mut rerun = false;
    screen = match screen {
        Screen::Home(home) => home_frame(app, home, &events, dt)?,
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
        Screen::Playing => playing_frame(
            app,
            &mut events,
            dt,
            &mut rerun,
            &mut profile_frame_active,
            ctrl_at_frame_start,
            shift_at_frame_start,
        ),
        Screen::Playback(pb) => playback_frame(app, pb, &events, dt, &mut rerun),
        Screen::FinalMap(final_map) => final_map_frame(app, final_map, &events, dt, &mut rerun),
        Screen::Results(results) => results_frame(app, results, &events, &mut rerun)?,
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
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
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
        screens::home::Out::Stay | screens::home::Out::Settings | screens::home::Out::Roster => {}
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
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
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
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
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
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
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
    let launch_result = resolve_new_match(out, &app.draft, &mut app.personality_seeds);
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
            Ok(fresh) => {
                app.install_session(fresh, app.args.paused, None);
                render::draw(&app.game.view(), &app.sprites, &app.input);
                *rerun = true;
                next = Some(Screen::Playing);
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
        Screen::Wizard(w)
    }
}

fn playing_frame(
    app: &mut App,
    events: &mut Vec<RawEvent>,
    dt: f32,
    rerun: &mut bool,
    profile_frame_active: &mut bool,
    ctrl_at_frame_start: bool,
    shift_at_frame_start: bool,
) -> Screen {
    let input_scope = app
        .game
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
    // The tutorial card is chrome; clicks on it must not reach the
    // world or consume an armed gameplay action.
    if let Some(t) = &app.tutorial {
        let dismiss = render::tutorial_dismiss_rect();
        let card = render::tutorial_card_rect(t);
        if events.iter().any(|e| {
            matches!(e, RawEvent::MouseDown { button: MouseButton::Left, x, y }
                if dismiss.contains(vec2(*x, *y)))
        }) {
            app.tutorial = None;
        }
        // Swallowing a release whose press began in the world must
        // also end that drag, or a later release completes it.
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
    input::update_held(&mut app.game, &app.input, dt);
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
        next = Some(open_pause(&mut app.game));
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
    drop(input_scope);
    let profile_barrier = !app.game.presentation.paused && app.frame_profiler.take_start_barrier();
    *profile_frame_active = !app.game.presentation.paused && !profile_barrier;
    let profile_stopped = if profile_barrier {
        false
    } else {
        let stopped = app
            .game
            .advance_wall_clock(dt, app.frame_profiler.stop_tick());
        app.game.update_wall_clock_fx(dt);
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
    dt: f32,
    rerun: &mut bool,
) -> Screen {
    pb.bindings.clone_from(&app.input.bindings);
    let input_scope = pb.diagnostics.as_ref().and_then(|recorder| {
        recorder.span(oxide_kit::diagnostics::Phase::Input, pb.engine.position())
    });
    let leave = pb.apply_input(
        events,
        dt,
        vec2(screen_width(), screen_height()),
        app.config.camera.zoom_inverted,
        app.config.camera.pan_speed,
        &mut app.input.mouse,
    );
    drop(input_scope);
    if leave {
        pb.finish_diagnostics();
        *rerun = true;
        match pb.return_to {
            PlaybackReturn::Pause => Screen::Pause(PauseScreen::open(
                app.game.state.result().is_some(),
                can_surrender(&app.game),
            )),
            PlaybackReturn::Results => Screen::Results(ResultsScreen::open()),
            PlaybackReturn::Home => Screen::Home(HomeScreen::open()),
        }
    } else {
        pb.advance_frame(dt, vec2(screen_width(), screen_height()));
        render::draw_with_performance(
            &pb.view(),
            &app.sprites,
            &app.input,
            Some(app.performance.view()),
        );
        screens::playback::playback_hud(&pb, vec2(screen_width(), screen_height()));
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
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
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
    final_map.draw_hud();
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
) -> Result<Screen> {
    let input_scope = app
        .game
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
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
    Ok(match out {
        screens::results::Out::Stay => Screen::Results(results),
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
            *rerun = true;
            Screen::FinalMap(FinalMapScreen::open())
        }
        screens::results::Out::Home => app.persistence_screen(
            persistence::Intent::Leave(screens::pause::LeaveVerb::MainMenu, false),
            Screen::Results(results),
        ),
    })
}

fn replays_frame(app: &mut App, mut shelf: Shelf, events: &[RawEvent], rerun: &mut bool) -> Screen {
    if !shelf.catalog_ready {
        app.request_catalog();
    }
    let mut leave: Option<Screen> = None;
    let input_scope = app
        .game
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
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
        shelf.menu.draw(&shelf.subtitle());
        Screen::Replays(shelf)
    }
}

fn pause_frame(app: &mut App, mut ps: PauseScreen, events: &[RawEvent]) -> Result<Screen> {
    let input_scope = app
        .game
        .diagnostic_span(oxide_kit::diagnostics::Phase::Input);
    let out = ps.update(
        events,
        &mut app.input.mouse,
        &mut app.game.presentation.sounds_pending,
    );
    drop(input_scope);
    render::draw(&app.game.view(), &app.sprites, &app.input);
    veil();
    ps.menu.draw(ps.subtitle(&app.game.scenario.name));
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
            ps.begin_naming(suggested);
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
mod tests {
    use super::*;

    fn configured_new_match_draft() -> NewMatchDraft {
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(Scenario::skirmish(), None);
        draft.seats[1].difficulty = oxide_sim::scenario::BotDifficulty::Prime;
        draft.seats[1].stance = oxide_sim::scenario::BotStance::Aggressive;
        draft
    }

    #[test]
    fn backdrop_animation_freezes_only_when_a_live_match_is_paused() {
        let home = || Screen::Home(HomeScreen::with_resumable(false));
        let pause = || Screen::Pause(PauseScreen::open(false, true));

        assert!(backdrop_fx_advances(&home()));
        assert!(!backdrop_fx_advances(&Screen::Playing));
        assert!(!backdrop_fx_advances(&pause()));

        let settings_from_home = Screen::Settings {
            screen: SettingsScreen::open(&config::Config::default()),
            back: Box::new(home()),
        };
        let settings_from_pause = Screen::Settings {
            screen: SettingsScreen::open(&config::Config::default()),
            back: Box::new(pause()),
        };
        assert!(backdrop_fx_advances(&settings_from_home));
        assert!(!backdrop_fx_advances(&settings_from_pause));
    }

    #[test]
    fn escape_clears_a_selection_before_pausing_except_for_terminal_overlays() {
        assert!(!playing_escape_opens_pause(false, false, false, false));
        assert!(playing_escape_opens_pause(true, false, false, false));
        assert!(!playing_escape_opens_pause(true, true, false, false));
        assert!(playing_escape_opens_pause(true, true, true, false));
        assert!(playing_escape_opens_pause(true, true, false, true));
    }

    #[test]
    fn opening_pause_freezes_play_and_ends_the_concede_banner() {
        let mut game =
            Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
        game.presentation.conceded_banner = true;
        let screen = open_pause(&mut game);
        assert!(matches!(screen, Screen::Pause(_)));
        assert!(game.presentation.paused);
        assert!(!game.presentation.conceded_banner);
        assert!(
            game.demo.paused_menu,
            "the menu button teaches the tutorial's pause lesson like Escape"
        );
    }

    #[test]
    fn new_match_seed_window_advances_once_only_after_a_successful_launch() {
        let mut personality_seeds = PersonalitySeedSource::from_seed(41);
        let first_base = personality_seeds.match_base();
        let draft = configured_new_match_draft();

        assert!(resolve_new_match(WizardOut::Stay, &draft, &mut personality_seeds).is_none());
        assert_eq!(personality_seeds.match_base(), first_base);
        assert!(resolve_new_match(WizardOut::Home, &draft, &mut personality_seeds).is_none());
        assert_eq!(
            personality_seeds.match_base(),
            first_base,
            "backing out cannot consume an opponent identity"
        );

        let mut invalid = configured_new_match_draft();
        invalid.seats.pop();
        let failed = resolve_new_match(WizardOut::Launch, &invalid, &mut personality_seeds)
            .expect("launch outcome has a result");
        assert!(failed.is_err());
        assert_eq!(
            personality_seeds.match_base(),
            first_base,
            "a launch refusal must leave the same seed window available for retry"
        );

        let launched = resolve_new_match(WizardOut::Launch, &draft, &mut personality_seeds)
            .expect("launch outcome has a result");
        let game = launched.expect("valid draft launches");
        assert_eq!(
            game.scenario.players[1]
                .bot_config
                .expect("opponent is configured")
                .personality_seed,
            first_base + 1
        );
        assert_eq!(
            personality_seeds.match_base(),
            first_base.wrapping_add(BOT_PERSONALITY_WINDOW),
            "one successful match consumes exactly one complete roster window"
        );
    }

    #[test]
    fn restart_and_rematch_rebuild_the_exact_opponents_without_rerolling() {
        let mut personality_seeds = PersonalitySeedSource::from_seed(41);
        let draft = configured_new_match_draft();
        let launched = resolve_new_match(WizardOut::Launch, &draft, &mut personality_seeds)
            .expect("launch outcome has a result");
        let game = launched.expect("valid draft launches");
        let next_base = personality_seeds.match_base();
        let expected = game.scenario.clone();

        let restarted = rebuild_match(&game).expect("Restart rebuilds the match");
        let rematched = rebuild_match(&game).expect("Rematch rebuilds the match");
        for rebuilt in [&restarted, &rematched] {
            assert_eq!(rebuilt.scenario, expected);
            assert_eq!(rebuilt.recorder.setup, expected);
            assert_eq!(rebuilt.hash_hex(), game.hash_hex());
        }
        assert_eq!(
            personality_seeds.match_base(),
            next_base,
            "rebuilding an existing scenario never consumes New Match entropy"
        );
    }
}
