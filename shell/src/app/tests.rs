use super::*;
use crate::screens::wizard::launch::launch;

#[test]
fn the_live_streak_counts_only_unbroken_live_frames() {
    let mut streak = 0;
    for _ in 0..3 {
        streak = next_live_streak(streak, true, true);
    }
    assert_eq!(streak, 3);
    assert_eq!(next_live_streak(streak, true, false), 0, "leaving play");
    assert_eq!(next_live_streak(streak, false, true), 0, "arriving in play");
    assert_eq!(next_live_streak(u8::MAX, true, true), u8::MAX);
}

/// The routing guards, row by row: frozen-map precedence and the
/// viewer's read-only boundary around local requests. Shared requests
/// are covered by the dispatcher and session-parity suites.
#[test]
fn a_lan_match_decided_under_its_menu_opens_the_report() {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }
    let mut game = Game::networked(
        scenario,
        oxide_sim::PlayerId(0),
        crate::game::network::NetRole::Host,
        vec2(1280.0, 800.0),
    )
    .unwrap();
    let running = || Screen::Pause(pause_menu(&Game::new(Scenario::skirmish()).unwrap()));
    let is_results = |screen: &Screen| matches!(screen, Screen::Results(_));
    assert!(!is_results(&report_under_menu(&game, running())));
    game.run_batch(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: oxide_sim::Command::Surrender,
    }]);
    assert!(game.state.result().is_some());
    assert!(is_results(&report_under_menu(&game, running())));
    assert!(
        !is_results(&report_under_menu(&game, Screen::Pause(pause_menu(&game)))),
        "a menu opened after the end stays"
    );
    let local = Game::new(Scenario::skirmish()).unwrap();
    assert!(!is_results(&report_under_menu(&local, running())));
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
    let mut backdrop = launch(&backdrop_draft, 0x2000).expect("backdrop match");
    backdrop.presentation.camera.pan(vec2(-1000.0, -1000.0));
    backdrop.clock.paused = true;
    backdrop.clock.speed = 4.0;
    backdrop.presentation.overlay = true;
    let backdrop_center = backdrop.presentation.camera.center;

    let mut draft = NewMatchDraft::default();
    draft.set_scenario(scenario, None);
    draft.seat_choice = 1;
    let mut game = keep_flags(
        launch(&draft, 0x3000).expect("swapped-seat match"),
        &backdrop,
    );

    assert_eq!(game.presentation.human, oxide_sim::PlayerId(1));
    let home = game.home_foundry().expect("human Foundry").center();
    let home = vec2(home.x.to_num::<f32>(), home.y.to_num::<f32>());
    let (lo, hi) = game.presentation.camera.world_rect();
    assert!(
        home.x >= lo.x && home.x <= hi.x && home.y >= lo.y && home.y <= hi.y,
        "the new human's Foundry at {home:?} is outside the opening view {lo:?}..{hi:?}"
    );
    assert_ne!(
        game.presentation.camera.center, backdrop_center,
        "session flags must not carry the backdrop camera into the new match"
    );
    assert!(game.clock.paused);
    assert_eq!(game.clock.speed, 4.0);
    assert!(game.presentation.overlay);

    let opening_center = game.presentation.camera.center;
    input::update_held(&mut game, &input, 1.0);
    assert_eq!(
        game.presentation.camera.center, opening_center,
        "edge pan must not mistake the menu click for a pointer at (0, 0)"
    );
}

#[test]
fn only_a_frames_first_pass_reads_the_hardware() {
    let escape = || vec![RawEvent::KeyDown { key: Key::Escape }];
    assert_eq!(hardware_events(true, escape), escape());
    assert!(
        hardware_events(false, || panic!("a rerun pass must not poll")).is_empty(),
        "the screen a key opened never receives that key"
    );
}
