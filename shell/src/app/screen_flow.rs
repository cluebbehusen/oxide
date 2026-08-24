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
        Screen::Playing | Screen::Playback(_) | Screen::FinalMap(_) | Screen::Pause(_) => false,
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

pub(super) fn update_and_draw(
    app: &mut App,
    mut screen: Screen,
    mut events: Vec<RawEvent>,
    dt: f32,
    ctrl_at_frame_start: bool,
    shift_at_frame_start: bool,
) -> Result<ScreenFrame> {
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
                screens::home::Out::Stay
                | screens::home::Out::Settings
                | screens::home::Out::Roster => {}
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
            } else if out == screens::home::Out::Roster {
                Screen::Codex {
                    screen: CodexScreen::open(),
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
        Screen::Codex {
            screen: mut codex,
            back,
        } => {
            let out = codex.update(&events, &mut app.input.mouse, &mut app.game.sounds_pending);
            render::draw(&app.game, &app.sprites, &app.input);
            veil();
            let viewer = app.game.state.player(app.game.human).faction;
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
                        app.menu_notice =
                            Some((format!("can't start that match: {err:#}"), get_time() + 5.0));
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
            let had_selection =
                !app.game.selection.units.is_empty() || !app.game.selection.buildings.is_empty();
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
            if playing_escape_opens_pause(
                escape_pressed,
                had_selection,
                app.game.state.result().is_some(),
                app.game.conceded_banner,
            ) {
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
            let profile_barrier = !app.game.paused && app.frame_profiler.take_start_barrier();
            profile_frame_active = !app.game.paused && !profile_barrier;
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
                app.game.paused = true;
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
        Screen::FinalMap(mut final_map) => {
            let leave = final_map.update(
                &events,
                dt,
                vec2(screen_width(), screen_height()),
                app.config.camera,
                &mut app.input.mouse,
                &mut app.game,
            );
            render::draw(&app.game, &app.sprites, &app.input);
            FinalMapScreen::draw_hud();
            if leave {
                app.game.spectate = false;
                rerun = true;
                Screen::Results(ResultsScreen::open())
            } else {
                Screen::FinalMap(final_map)
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
                screens::results::Out::Watch => match result_playback(&app.game) {
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
                screens::results::Out::ViewFinalMap => {
                    app.game.paused = true;
                    app.game.spectate = true;
                    app.game.selection.units.clear();
                    app.game.selection.buildings.clear();
                    rerun = true;
                    Screen::FinalMap(FinalMapScreen::open())
                }
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
                    // Restarting a tutorial also restarts its lesson state.
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

    Ok(ScreenFrame {
        screen,
        rerun,
        profile_frame_active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
