use super::*;
use crate::screens::wizard::launch::launch;

#[test]
fn keyboard_is_wanted_only_while_naming() {
    let mut pause = PauseScreen::open(false, true);
    assert!(!text_entry(&Screen::Pause(PauseScreen::open(false, true))));
    pause.begin_naming("Skirmish | t40");
    assert!(text_entry(&Screen::Pause(pause)));
    assert!(!text_entry(&Screen::Playing));
}

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

#[test]
fn frame_context_follows_playback_speed_instead_of_the_hidden_live_clock() {
    use oxide_protocol::DebugSession;

    let mut live = Game::new(Scenario::skirmish()).unwrap();
    live.presentation.speed = 4.0;
    let replay = oxide_kit::GameReplay::new(oxide_sim::SIM_VERSION, "test", Scenario::skirmish());
    let mut playback = PlaybackSession::from_replay(replay).unwrap();
    for speed in [0.5, 1.0, 8.0, 64.0] {
        playback.set_speed(speed).unwrap();
        let screen = Screen::Playback(Box::new(playback));
        assert_eq!(visible_speed(&screen, &live), speed);
        assert_eq!(live.presentation.speed, 4.0);
        let Screen::Playback(session) = screen else {
            unreachable!()
        };
        playback = *session;
    }
    assert_eq!(visible_speed(&Screen::Playing, &live), 4.0);
    assert_eq!(
        visible_speed(&Screen::Pause(PauseScreen::open(false, true)), &live),
        4.0
    );
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
    backdrop.presentation.paused = true;
    backdrop.presentation.speed = 4.0;
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
    assert!(game.presentation.paused);
    assert_eq!(game.presentation.speed, 4.0);
    assert!(game.presentation.overlay);

    let opening_center = game.presentation.camera.center;
    input::update_held(&mut game, &input, 1.0);
    assert_eq!(
        game.presentation.camera.center, opening_center,
        "edge pan must not mistake the menu click for a pointer at (0, 0)"
    );
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
fn only_a_frames_first_pass_reads_the_hardware() {
    let escape = || vec![RawEvent::KeyDown { key: Key::Escape }];
    assert_eq!(hardware_events(true, escape), escape());
    assert!(
        hardware_events(false, || panic!("a rerun pass must not poll")).is_empty(),
        "the screen a key opened never receives that key"
    );
}
