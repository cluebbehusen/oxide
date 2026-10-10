use super::*;
use crate::game::Game;
use macroquad::prelude::vec2;

fn replay() -> GameReplay {
    // A short real replay: run the embedded skirmish headless and
    // record it, exactly what a save file contains.
    let scenario = oxide_sim::Scenario::skirmish();
    let outcome = oxide_kit::runner::run_scenario(
        &scenario,
        60,
        true,
        Some(&oxide_kit::recovery::BuildIdentity::default()),
    )
    .expect("run");
    let mut replay = outcome.replay.expect("recorded");
    replay.meta.ticks = Some(60);
    replay
}

fn session() -> PlaybackSession {
    PlaybackSession::from_replay(replay()).expect("session opens")
}

#[test]
fn composition_intervals_align_with_ticks_and_preserve_a_short_final_interval() {
    for start in [0, 37] {
        let ticks = [start, start + 3, start + 6, start + 8];
        assert_eq!(composition_interval(&ticks, 0), (0.0, 3.0 / 8.0));
        assert_eq!(composition_interval(&ticks, 1), (3.0 / 8.0, 6.0 / 8.0));
        assert_eq!(composition_interval(&ticks, 2), (6.0 / 8.0, 1.0));
        assert_eq!(composition_interval(&ticks, 3), (1.0, 1.0));
    }
    assert_eq!(composition_interval(&[37], 0), (0.0, 1.0));
}

#[test]
fn checkpoint_origin_transport_starts_and_scrubs_at_the_available_boundary() {
    let scenario = oxide_sim::Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    for _ in 0..37 {
        state.tick(&[]);
    }
    let origin = oxide_kit::recording::WorldOrigin::capture(&scenario, &state).unwrap();
    let mut replay = GameReplay::with_origin(SIM_VERSION, "test", scenario, origin).unwrap();
    replay.meta.ticks = Some(97);
    assert!(game::Game::from_replay(replay.clone()).is_err());
    let mut pb = PlaybackSession::from_replay(replay).unwrap();
    assert_eq!(pb.engine.position(), 37);
    let bar = macroquad::prelude::Rect::new(10.0, 10.0, 100.0, 10.0);
    assert_eq!(pb.tick_at(bar, 10.0), 37);
    assert_eq!(pb.tick_at(bar, 60.0), 67);
    assert_eq!(pb.tick_at(bar, 110.0), 97);
    key(&mut pb, Key::End);
    assert_eq!(pb.engine.position(), 97);
    key(&mut pb, Key::Home);
    assert_eq!(pb.engine.position(), 37);
    assert_eq!(pb.engine.state.hash(), state.hash());
}

fn long_session(ticks: u64) -> PlaybackSession {
    let mut replay = GameReplay::new(SIM_VERSION, "test", oxide_sim::Scenario::skirmish());
    replay.meta.ticks = Some(ticks);
    PlaybackSession::from_replay(replay).expect("long session opens")
}

fn key(session: &mut PlaybackSession, key: Key) -> bool {
    key_with(session, &crate::action::BindingMap::classic(), key)
}

fn key_with(session: &mut PlaybackSession, bindings: &crate::action::BindingMap, key: Key) -> bool {
    let mut mouse = vec2(0.0, 0.0);
    let leave = session.update(
        bindings,
        &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
        0.0,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    // Seeks are budgeted across frames; drain any pending one so
    // asserts see the settled position.
    let mut frames = 0;
    while session.seeking.is_some() {
        session.update(
            &crate::action::BindingMap::classic(),
            &[],
            0.0,
            vec2(1280.0, 800.0),
            crate::config::CameraPrefs::default(),
            &mut mouse,
        );
        frames += 1;
        assert!(frames < 1_000, "a pending seek must finish");
    }
    leave
}

#[test]
fn an_all_bot_record_opens_for_watching() {
    // Driver benchmarks and bot-vs-bot spectacles record replays
    // with no human seat; the viewer must not demand one.
    let mut scenario = oxide_sim::Scenario::skirmish();
    for p in &mut scenario.players {
        p.bot = true;
        p.bot_config = Some(oxide_sim::scenario::BotConfig::default());
    }
    let outcome = oxide_kit::runner::run_scenario(
        &scenario,
        60,
        true,
        Some(&oxide_kit::recovery::BuildIdentity::default()),
    )
    .expect("run");
    let mut replay = outcome.replay.expect("recorded");
    replay.meta.ticks = Some(60);
    let pb = PlaybackSession::from_replay(replay).expect("a spectator needs no command seat");
    assert!(pb.presentation.spectate, "the viewer stays fog-free");
}

#[test]
fn transport_input_defers_replay_work_until_the_frame_advance() {
    let mut pb = session();
    let viewport = vec2(1280.0, 800.0);
    let mut mouse = Vec2::ZERO;
    assert!(!pb.apply_input(
        &crate::action::BindingMap::classic(),
        &[RawEvent::KeyDown { key: Key::End }],
        1.0,
        viewport,
        crate::config::CameraPrefs {
            zoom_inverted: false,
            pan_speed: 1.0,
            ..crate::config::CameraPrefs::default()
        },
        &mut mouse
    ));
    assert_eq!(pb.engine.position(), 0);
    assert_eq!(pb.seeking, Some(60));
    pb.advance_frame(FrameTime::measure(1.0), viewport);
    assert_eq!(pb.engine.position(), 60);
    assert_eq!(pb.engine.state.current_tick(), 60);

    assert!(!pb.apply_input(
        &crate::action::BindingMap::classic(),
        &[RawEvent::KeyDown { key: Key::Home }],
        0.0,
        viewport,
        crate::config::CameraPrefs {
            zoom_inverted: false,
            pan_speed: 1.0,
            ..crate::config::CameraPrefs::default()
        },
        &mut mouse
    ));
    pb.advance_frame(FrameTime::measure(0.0), viewport);
    assert_eq!(pb.engine.position(), 0);
    assert!(!pb.apply_input(
        &crate::action::BindingMap::classic(),
        &[],
        0.1,
        viewport,
        crate::config::CameraPrefs {
            zoom_inverted: false,
            pan_speed: 1.0,
            ..crate::config::CameraPrefs::default()
        },
        &mut mouse
    ));
    assert_eq!(pb.engine.position(), 0);
    pb.advance_frame(FrameTime::measure(0.1), viewport);
    assert_eq!(pb.engine.position(), 2);
}

#[test]
fn a_suspension_length_frame_ages_effects_by_at_most_a_quarter_second() {
    let mut pb = session();
    let viewport = vec2(1280.0, 800.0);
    pb.presentation.toast("seek complete");
    pb.advance_frame(FrameTime::measure(60.0), viewport);
    assert!(
        pb.engine.position() > 0,
        "the replay clock still catches up on the unclamped time"
    );
    let toast = pb
        .presentation
        .toasts
        .iter()
        .find(|toast| toast.text == "Seek complete")
        .expect("a toast outlives a long frame");
    assert_eq!(toast.age, 0.25);
}

#[test]
fn the_transport_answers_its_keys() {
    let mut pb = session();
    assert!(!pb.clock.paused);
    key(&mut pb, Key::Space);
    assert!(pb.clock.paused, "space pauses");
    for (key_code, speed) in [
        (Key::Num1, 0.5),
        (Key::Num2, 1.0),
        (Key::Num3, 2.0),
        (Key::Num4, 4.0),
        (Key::Num5, 8.0),
        (Key::Num6, 16.0),
        (Key::Num7, 32.0),
        (Key::Num8, 64.0),
    ] {
        key(&mut pb, key_code);
        assert!(
            (pb.clock.speed - speed).abs() < f64::EPSILON,
            "{key_code:?} selects {speed}x"
        );
    }
    key(&mut pb, Key::End);
    assert_eq!(pb.engine.position(), 60, "End seeks to the tail");
    key(&mut pb, Key::Home);
    assert_eq!(pb.engine.position(), 0, "Home rewinds");
    key(&mut pb, Key::PageDown);
    assert_eq!(pb.engine.position(), 60, "seeks clamp to the total");
    assert!(key(&mut pb, Key::Escape), "Escape closes the viewer");
}

fn feed(pb: &mut PlaybackSession, events: &[RawEvent]) -> bool {
    let mut mouse = vec2(0.0, 0.0);
    pb.apply_input(
        &crate::action::BindingMap::classic(),
        events,
        0.0,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs {
            zoom_inverted: false,
            pan_speed: 1.0,
            ..crate::config::CameraPrefs::default()
        },
        &mut mouse,
    )
}

fn tap(id: u64, p: Vec2) -> [RawEvent; 2] {
    [
        RawEvent::TouchDown { id, x: p.x, y: p.y },
        RawEvent::TouchUp { id, x: p.x, y: p.y },
    ]
}

fn click(p: Vec2) -> [RawEvent; 2] {
    [
        RawEvent::MouseDown {
            button: MouseButton::Left,
            x: p.x,
            y: p.y,
        },
        RawEvent::MouseUp {
            button: MouseButton::Left,
            x: p.x,
            y: p.y,
        },
    ]
}

#[test]
fn the_corner_buttons_leave_and_toggle_by_click_and_by_tap() {
    let [back, play] = transport_buttons(render::ui_scale()).map(|(rect, _)| rect.center());
    let mut pb = session();
    assert!(!feed(&mut pb, &tap(1, play)));
    assert!(pb.clock.paused, "a tap pauses");
    assert!(!feed(&mut pb, &click(play)));
    assert!(!pb.clock.paused, "a click resumes");
    assert!(feed(&mut pb, &tap(2, back)), "a tap on Back leaves");
    let mut pb = session();
    assert!(feed(&mut pb, &click(back)), "so does a click");
}

#[test]
fn a_back_press_that_slides_off_stays() {
    let [(back, _), _] = transport_buttons(render::ui_scale());
    let off = vec2(back.center().x, back.y + back.h + 200.0);
    let mut pb = session();
    let before = pb.presentation.camera.center;
    let events = [
        RawEvent::TouchDown {
            id: 1,
            x: back.center().x,
            y: back.center().y,
        },
        RawEvent::TouchMove {
            id: 1,
            x: off.x,
            y: off.y,
        },
        RawEvent::TouchUp {
            id: 1,
            x: off.x,
            y: off.y,
        },
    ];
    assert!(!feed(&mut pb, &events));
    assert_eq!(
        pb.presentation.camera.center, before,
        "a finger born on a button never pans"
    );
}

#[test]
fn a_touch_on_the_scrub_band_seeks_and_a_drag_retargets() {
    let mut pb = session();
    let bar = scrub_rect(&pb.view(), vec2(1280.0, 800.0));
    // Just above the thin bar, inside its fingertip target.
    let start = vec2(bar.x + bar.w * 0.5, bar.y - 8.0);
    feed(
        &mut pb,
        &[RawEvent::TouchDown {
            id: 1,
            x: start.x,
            y: start.y,
        }],
    );
    assert_eq!(pb.seeking, Some(30));
    feed(
        &mut pb,
        &[RawEvent::TouchMove {
            id: 1,
            x: bar.x + bar.w,
            y: start.y,
        }],
    );
    assert_eq!(pb.seeking, Some(60), "the drag re-targets the seek");
    feed(
        &mut pb,
        &[RawEvent::TouchUp {
            id: 1,
            x: bar.x + bar.w,
            y: start.y,
        }],
    );
    assert_eq!(pb.scrub_finger, None);
}

#[test]
fn a_touch_drag_on_the_battlefield_moves_only_the_camera() {
    let mut pb = session();
    let before = pb.presentation.camera.center;
    let from = vec2(640.0, 300.0);
    let to = vec2(540.0, 300.0);
    feed(
        &mut pb,
        &[
            RawEvent::TouchDown {
                id: 1,
                x: from.x,
                y: from.y,
            },
            RawEvent::TouchMove {
                id: 1,
                x: to.x,
                y: to.y,
            },
            RawEvent::TouchUp {
                id: 1,
                x: to.x,
                y: to.y,
            },
        ],
    );
    assert!(pb.presentation.camera.center.x > before.x);
    assert_eq!(pb.seeking, None);
    assert_eq!(pb.engine.position(), 0, "camera gestures never seek");
}

#[test]
fn a_scrub_press_seeks_to_the_bar_fraction_and_a_drag_retargets() {
    let mut pb = session();
    let viewport = vec2(1280.0, 800.0);
    let bar = scrub_rect(&pb.view(), viewport);
    let mut mouse = vec2(0.0, 0.0);
    pb.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: bar.x + bar.w * 0.75,
            y: bar.y + bar.h * 0.5,
        }],
        0.0,
        viewport,
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert!(pb.scrubbing, "the press grabs the timeline");
    // A 60-tick record fits one frame's budget, so the seek has
    // already landed; the position is the proof.
    let landed = pb.engine.position();
    assert!(
        (40..=50).contains(&landed),
        "three quarters of a 60-tick record is ~45, got {landed}"
    );
    // Dragging retargets before release.
    pb.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::MouseMove { x: bar.x, y: bar.y }],
        0.0,
        viewport,
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(pb.engine.position(), 0, "the drag walked the target home");
    pb.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::MouseUp {
            button: MouseButton::Left,
            x: bar.x,
            y: bar.y,
        }],
        0.0,
        viewport,
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert!(!pb.scrubbing, "release lets go");
}

#[test]
fn the_viewer_answers_the_shared_surface_exactly_like_a_resumed_live_session() {
    // A replayed world and a live world resumed from the same record
    // answer the protocol identically. Both go through the one shared
    // dispatcher, so agreement here is agreement on the wire.
    use oxide_protocol::{DebugSession, Reply, Request, StateFilter, dispatch_shared};
    let scenario = oxide_sim::Scenario::skirmish();
    let outcome = oxide_kit::runner::run_scenario(
        &scenario,
        120,
        true,
        Some(&oxide_kit::recovery::BuildIdentity::default()),
    )
    .expect("run");
    let mut replay = outcome.replay.expect("recorded");
    replay.meta.ticks = Some(120);
    let mut live = Game::from_replay(replay.clone()).expect("the record resumes live");
    let mut pb = PlaybackSession::from_replay(replay).expect("the record opens for watching");
    // The driven clock: the viewer seeks to the tick the resumed
    // session re-simulated to, reporting what actually ran.
    let Some(Ok(Reply::Advanced(advanced))) =
        dispatch_shared(&mut pb, &Request::AdvanceTicks { ticks: 500 })
    else {
        panic!("advance is a shared request");
    };
    assert_eq!(
        advanced.ticks, 120,
        "a replay near its end advances less than asked"
    );
    assert_eq!(advanced.tick, 120);
    for request in [
        Request::StateHash,
        Request::QueryState {
            filter: StateFilter {
                map: true,
                ..StateFilter::default()
            },
        },
        Request::QueryFogView {
            player: oxide_sim::PlayerId(0),
        },
    ] {
        let a = dispatch_shared(&mut live, &request).expect("shared");
        let b = dispatch_shared(&mut pb, &request).expect("shared");
        assert_eq!(a, b, "replies diverged on {request:?}");
    }
    // Status: one world, two transports. The world's fields agree;
    // pause stance, speed, and the recorder are each transport's own
    // (the read-only viewer records nothing).
    let live_status = DebugSession::status(&live);
    let viewer_status = DebugSession::status(&pb);
    assert_eq!(live_status.tick, viewer_status.tick);
    assert_eq!(live_status.scenario, viewer_status.scenario);
    assert_eq!(live_status.sim_version, viewer_status.sim_version);
    assert_eq!(live_status.result, viewer_status.result);
    assert_eq!(viewer_status.recorded_commands, 0);
    // The clock family answers on both, and refuses the same speeds
    // in the same words.
    for session in [&mut live as &mut dyn DebugSession, &mut pb] {
        assert_eq!(
            dispatch_shared(session, &Request::Pause),
            Some(Ok(Reply::Ok))
        );
        let refusal = dispatch_shared(session, &Request::SetSpeed { multiplier: 1000.0 })
            .expect("speed is shared")
            .expect_err("1000x is out of range");
        assert!(refusal.contains("outside 0.05..=64"));
    }
}

#[test]
fn paused_time_does_not_advance_the_reproduction() {
    let mut pb = session();
    key(&mut pb, Key::Space);
    let before = pb.engine.position();
    let fx_before = pb.presentation.fx_time();
    let mut mouse = vec2(0.0, 0.0);
    pb.update(
        &crate::action::BindingMap::classic(),
        &[],
        1.0,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(pb.engine.position(), before, "a paused viewer holds still");
    assert_eq!(
        pb.presentation.fx_time(),
        fx_before,
        "a paused viewer holds decorative animation too"
    );
}

#[test]
fn replay_fraction_drives_interpolation_and_authored_cycles() {
    let mut pb = session();
    let mut mouse = vec2(0.0, 0.0);
    let start = pb.engine.position();

    pb.update(
        &crate::action::BindingMap::classic(),
        &[],
        game::TICK_DT * 0.25,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(pb.engine.position(), start);
    assert!((pb.presentation.tick_fraction() - 0.25).abs() < 1e-6);
    assert!((pb.clock.render_alpha() - 0.25).abs() < 1e-6);

    pb.update(
        &crate::action::BindingMap::classic(),
        &[],
        game::TICK_DT,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(pb.engine.position(), start + 1);
    assert!((pb.presentation.tick_fraction() - 0.25).abs() < 1e-6);
}

#[test]
fn driven_steps_discard_partial_wall_clock_debt() {
    use oxide_protocol::DebugSession;

    for present in [false, true] {
        let mut pb = session();
        let mut mouse = vec2(0.0, 0.0);
        pb.update(
            &crate::action::BindingMap::classic(),
            &[],
            game::TICK_DT * 0.75,
            vec2(1280.0, 800.0),
            crate::config::CameraPrefs::default(),
            &mut mouse,
        );
        assert!((pb.presentation.tick_fraction() - 0.75).abs() < 1e-6);

        if present {
            DebugSession::present(&mut pb, 1);
        } else {
            DebugSession::advance(&mut pb, 1);
        }

        assert_eq!(
            pb.clock.accum, 0.0,
            "a driven step must reset the viewer clock like a live session"
        );
        assert_eq!(pb.presentation.tick_fraction(), 0.0);
        let after_step = pb.engine.position();
        pb.update(
            &crate::action::BindingMap::classic(),
            &[],
            game::TICK_DT * 0.3,
            vec2(1280.0, 800.0),
            crate::config::CameraPrefs::default(),
            &mut mouse,
        );
        assert_eq!(
            pb.engine.position(),
            after_step,
            "a sub-tick frame after a driven step must not consume old debt"
        );
    }
}

#[test]
fn seek_replacement_resets_interpolation_and_old_timeline_facing() {
    use oxide_protocol::DebugSession;

    let mut pb = session();
    pb.presentation
        .prev_pos
        .values_mut()
        .for_each(|position| *position += vec2(99.0, 99.0));
    pb.presentation.facing.insert(0, 1.25);

    DebugSession::advance(&mut pb, 40);

    for (&id, &angle) in &pb.presentation.facing {
        let unit = pb.engine.state.unit(oxide_sim::UnitId(id)).unwrap();
        let expected =
            f32::from(unit.heading) * std::f32::consts::TAU / 256.0 + std::f32::consts::FRAC_PI_2;
        assert_eq!(angle, expected);
    }
    for unit in pb.engine.state.units() {
        let expected = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
        assert_eq!(pb.presentation.prev_pos.get(&unit.id.0), Some(&expected));
    }
}

#[test]
fn long_seeks_are_budgeted_and_a_new_transport_command_replaces_them() {
    use oxide_protocol::DebugSession;
    let mut pb = long_session(5_000);
    let viewport = vec2(1280.0, 800.0);
    let mut mouse = Vec2::ZERO;

    pb.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::KeyDown { key: Key::End }],
        0.0,
        viewport,
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(
        pb.engine.position(),
        2_000,
        "one frame consumes one seek budget"
    );
    assert_eq!(pb.seeking, Some(5_000));

    pb.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::KeyDown { key: Key::PageUp }],
        0.0,
        viewport,
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(pb.engine.position(), 1_500);
    assert_eq!(pb.seeking, None, "the replacement target settled");

    pb.seeking = Some(5_000);
    pb.clock.accum = game::TICK_DT * 0.75;
    let advanced = DebugSession::advance(&mut pb, 10);
    assert_eq!(advanced.ticks, 10);
    assert_eq!(pb.engine.position(), 1_510);
    assert_eq!(pb.seeking, None, "driven input cancels stale UI seeks");
    assert_eq!(pb.clock.accum, 0.0, "driven input drops wall-clock debt");
}

#[test]
fn playback_hitches_run_only_one_frames_tick_budget() {
    let mut pb = long_session(5_000);
    pb.clock.speed = 64.0;
    let mut mouse = Vec2::ZERO;
    pb.update(
        &crate::action::BindingMap::classic(),
        &[],
        1.0,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(pb.engine.position(), 24);
    assert!(
        pb.clock.accum < game::TICK_DT,
        "excess frame debt is dropped"
    );
}

#[test]
fn statistics_are_created_lazily_and_retained_while_hidden() {
    let mut pb = session();
    assert!(!pb.show_stats);
    assert!(pb.stats.is_none());

    key(&mut pb, Key::Tab);
    assert!(pb.show_stats);
    let final_tick = pb.stats.as_ref().expect("statistics computed").final_tick;
    assert_eq!(final_tick, pb.engine.total());

    key(&mut pb, Key::Tab);
    assert!(!pb.show_stats);
    assert_eq!(
        pb.stats.as_ref().map(|stats| stats.final_tick),
        Some(final_tick)
    );
}
#[test]
fn playback_uses_rebound_camera_and_transport_keys_without_advancing_the_record() {
    use crate::action::Chord;
    let mut pb = session();
    pb.clock.paused = true;
    pb.presentation.camera.zoom_at(vec2(640.0, 400.0), 4.0);
    pb.presentation.camera.update(1.0);
    let mut bindings = crate::action::BindingMap::classic();
    assert!(bindings.rebind(Action::PanRight, Chord::bare(Key::L)));
    assert!(bindings.rebind(Action::ReplayStats, Chord::bare(Key::O)));
    let mut mouse = vec2(640.0, 400.0);
    let tick = pb.engine.position();
    let before = pb.presentation.camera.center.x;
    pb.update(
        &bindings,
        &[
            RawEvent::KeyDown { key: Key::L },
            RawEvent::KeyDown { key: Key::Right },
        ],
        0.1,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    let halfway = pb.presentation.camera.center.x;
    pb.update(
        &bindings,
        &[RawEvent::KeyUp { key: Key::L }],
        0.1,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    let after = pb.presentation.camera.center.x;
    assert!(before < halfway && halfway < after);
    pb.update(
        &bindings,
        &[RawEvent::KeyUp { key: Key::Right }],
        0.1,
        vec2(1280.0, 800.0),
        crate::config::CameraPrefs::default(),
        &mut mouse,
    );
    assert_eq!(pb.presentation.camera.center.x, after);
    assert_eq!(pb.engine.position(), tick);
    key_with(&mut pb, &bindings, Key::Tab);
    assert!(!pb.show_stats);
    key_with(&mut pb, &bindings, Key::O);
    assert!(pb.show_stats);
}
