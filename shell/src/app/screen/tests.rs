use super::*;

/// A replay viewer over `back`.
fn viewer(session: PlaybackSession, back: Screen) -> Screen {
    Screen::Playback {
        session: Box::new(session),
        back: Box::new(back),
    }
}

#[test]
fn each_screen_kind_states_its_gameplay_and_performance_context() {
    for kind in [
        ScreenKind::Home,
        ScreenKind::Settings,
        ScreenKind::Codex,
        ScreenKind::Wizard,
        ScreenKind::Lobby,
        ScreenKind::Replays,
        ScreenKind::Results,
        ScreenKind::Pause,
        ScreenKind::Busy,
    ] {
        assert!(!kind.gameplay(), "{kind:?}");
        assert_eq!(kind.performance_context(), 0, "{kind:?}");
    }
    assert_eq!(
        [
            ScreenKind::Playing,
            ScreenKind::Playback,
            ScreenKind::FinalMap
        ]
        .map(|kind| (kind.gameplay(), kind.performance_context())),
        [(true, 1), (true, 2), (true, 3)]
    );
}

#[test]
fn a_menu_opened_from_pause_still_counts_as_over_pause() {
    let pause = || Screen::Pause(PauseScreen::open(false, true));
    let codex_over_pause = Screen::Codex {
        screen: CodexScreen::open(),
        back: Box::new(pause()),
    };
    assert!(pause().over_pause());
    assert!(codex_over_pause.over_pause());
    assert!(
        !Screen::Codex {
            screen: CodexScreen::open(),
            back: Box::new(Screen::Home(HomeScreen::open())),
        }
        .over_pause()
    );
    assert!(!Screen::Playing.over_pause());
}

#[test]
fn keyboard_is_wanted_only_while_naming() {
    let mut pause = PauseScreen::open(false, true);
    assert!(!Screen::Pause(PauseScreen::open(false, true)).text_entry());
    pause.begin_naming("Skirmish | t40");
    assert!(Screen::Pause(pause).text_entry());
    assert!(!Screen::Playing.text_entry());
}

#[test]
fn frame_context_follows_playback_speed_instead_of_the_hidden_live_clock() {
    use oxide_protocol::DebugSession;

    let mut live = Game::new(oxide_sim::Scenario::skirmish()).unwrap();
    live.clock.speed = 4.0;
    let replay = oxide_kit::GameReplay::new(
        oxide_sim::SIM_VERSION,
        "test",
        oxide_sim::Scenario::skirmish(),
    );
    let mut playback = PlaybackSession::from_replay(replay).unwrap();
    for speed in [0.5, 1.0, 8.0, 64.0] {
        playback.set_speed(speed).unwrap();
        let screen = viewer(playback, Screen::Home(HomeScreen::open()));
        assert_eq!(screen.visible(&live).speed(), speed);
        assert_eq!(live.clock.speed, 4.0);
        let Screen::Playback { session, .. } = screen else {
            unreachable!()
        };
        playback = *session;
    }
    assert_eq!(Screen::Playing.visible(&live).speed(), 4.0);
    assert_eq!(
        Screen::Pause(PauseScreen::open(false, true))
            .visible(&live)
            .speed(),
        4.0
    );
}

#[test]
fn failed_quit_dialogs_return_to_every_hidden_live_match() {
    let home = || Screen::Home(HomeScreen::with_resumable(false));
    let pause = || Screen::Pause(PauseScreen::open(false, true));

    assert!(!home().holds_live_match());
    assert!(Screen::Playing.holds_live_match());
    assert!(pause().holds_live_match());
    assert!(Screen::Results(ResultsScreen::open()).holds_live_match());
    assert!(Screen::FinalMap(FinalMapScreen::open()).holds_live_match());

    let settings_from_home = Screen::Settings {
        screen: SettingsScreen::open(&crate::config::Config::default()),
        back: Box::new(home()),
    };
    let settings_from_pause = Screen::Settings {
        screen: SettingsScreen::open(&crate::config::Config::default()),
        back: Box::new(pause()),
    };
    assert!(!settings_from_home.holds_live_match());
    assert!(settings_from_pause.holds_live_match());

    let game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
    let session = || super::super::live_playback(&game).expect("viewer");
    assert!(!viewer(session(), home()).holds_live_match());
    assert!(viewer(session(), pause()).holds_live_match());
    assert!(viewer(session(), Screen::Results(ResultsScreen::open())).holds_live_match());
}

#[test]
fn backdrop_animation_freezes_only_when_a_live_match_is_paused() {
    let home = || Screen::Home(HomeScreen::with_resumable(false));
    let pause = || Screen::Pause(PauseScreen::open(false, true));

    assert!(home().backdrop_runs());
    assert!(!Screen::Playing.backdrop_runs());
    assert!(!pause().backdrop_runs());

    let settings_from_home = Screen::Settings {
        screen: SettingsScreen::open(&crate::config::Config::default()),
        back: Box::new(home()),
    };
    let settings_from_pause = Screen::Settings {
        screen: SettingsScreen::open(&crate::config::Config::default()),
        back: Box::new(pause()),
    };
    assert!(settings_from_home.backdrop_runs());
    assert!(!settings_from_pause.backdrop_runs());
}
