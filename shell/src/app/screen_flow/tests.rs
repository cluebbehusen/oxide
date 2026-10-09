use super::*;

const LOOPBACK: &str = "127.0.0.1:0";

#[test]
fn the_tutorial_card_keeps_its_presses_and_a_tap_dismisses_it() {
    let card = macroquad::math::Rect::new(400.0, 36.0, 460.0, 120.0);
    let dismiss = macroquad::math::Rect::new(834.0, 40.0, 22.0, 22.0);
    let on_card = vec2(500.0, 100.0);
    let world = vec2(500.0, 400.0);

    let mut events = vec![
        RawEvent::TouchDown {
            id: 1,
            x: on_card.x,
            y: on_card.y,
        },
        RawEvent::TouchDown {
            id: 2,
            x: world.x,
            y: world.y,
        },
        RawEvent::TouchUp {
            id: 2,
            x: world.x,
            y: world.y,
        },
    ];
    let filtered = filter_tutorial_pointer(&mut events, card, dismiss, 1.0);
    assert_eq!(filtered, TutorialPointer::default());
    assert_eq!(
        events,
        vec![
            RawEvent::TouchDown {
                id: 2,
                x: world.x,
                y: world.y,
            },
            RawEvent::TouchUp {
                id: 2,
                x: world.x,
                y: world.y,
            },
        ],
        "the card eats its finger and leaves the world's alone"
    );

    // A fingertip just outside the drawn box still reaches it.
    let near = vec2(dismiss.right() + 6.0, dismiss.y + 4.0);
    assert!(!dismiss.contains(near));
    let mut events = vec![RawEvent::TouchDown {
        id: 3,
        x: near.x,
        y: near.y,
    }];
    let filtered = filter_tutorial_pointer(&mut events, card, dismiss, 1.0);
    assert!(filtered.dismissed);
    assert!(events.is_empty());

    let center = dismiss.center();
    let mut events = vec![
        RawEvent::MouseDown {
            button: MouseButton::Left,
            x: center.x,
            y: center.y,
        },
        RawEvent::MouseUp {
            button: MouseButton::Left,
            x: on_card.x,
            y: on_card.y,
        },
    ];
    let filtered = filter_tutorial_pointer(&mut events, card, dismiss, 1.0);
    assert_eq!(
        filtered,
        TutorialPointer {
            dismissed: true,
            swallowed_release: true,
        }
    );
    assert!(events.is_empty());
}

fn configured_new_match_draft() -> NewMatchDraft {
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(Scenario::skirmish(), None);
    draft.seats[1].difficulty = oxide_sim::scenario::BotDifficulty::Prime;
    draft.seats[1].stance = oxide_sim::scenario::BotStance::Aggressive;
    draft
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
    let screen = open_pause(&mut game, PauseCause::Player);
    assert!(matches!(screen, Screen::Pause(_)));
    enter(&mut game, &screen);
    assert!(game.clock.paused);
    assert!(!game.presentation.conceded_banner);
    assert!(
        game.demo.paused_menu,
        "the menu button teaches the tutorial's pause lesson like Escape"
    );
}

#[test]
fn a_suspension_gap_pauses_only_a_running_live_match() {
    // (raw_dt, live_streak, running, exempt) -> pauses
    let cases = [
        ((SUSPENSION_GAP_SECS, 2, true, false), true),
        ((60.0, 9, true, false), true),
        ((SUSPENSION_GAP_SECS - 0.01, 9, true, false), false),
        ((60.0, 1, true, false), false),
        ((60.0, 0, true, false), false),
        ((60.0, 9, false, false), false),
        ((60.0, 9, true, true), false),
    ];
    for ((dt, streak, running, exempt), pauses) in cases {
        assert_eq!(
            gap_opens_pause(dt, streak, running, exempt),
            pauses,
            "dt {dt}, streak {streak}, running {running}, exempt {exempt}"
        );
    }
}

#[test]
fn a_suspension_pause_explains_itself_without_teaching_the_lesson() {
    let mut game =
        Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
    let screen = open_pause(&mut game, PauseCause::Suspension);
    enter(&mut game, &screen);
    let Screen::Pause(pause) = screen else {
        panic!("a suspension opens the pause menu");
    };
    assert!(game.clock.paused);
    assert!(!game.demo.paused_menu);
    assert_eq!(pause.subtitle("Skirmish"), "paused after an interruption");
}

#[test]
fn new_match_seed_window_advances_once_only_after_a_successful_launch() {
    let mut personality_seeds = PersonalitySeedSource::from_seed(41);
    let first_base = personality_seeds.match_base();
    let draft = configured_new_match_draft();

    assert!(resolve_new_match(WizardOut::Stay, &draft, &mut personality_seeds, LOOPBACK).is_none());
    assert_eq!(personality_seeds.match_base(), first_base);
    assert!(resolve_new_match(WizardOut::Home, &draft, &mut personality_seeds, LOOPBACK).is_none());
    assert_eq!(
        personality_seeds.match_base(),
        first_base,
        "backing out cannot consume an opponent identity"
    );

    let mut invalid = configured_new_match_draft();
    invalid.seats.pop();
    let failed = resolve_new_match(
        WizardOut::Launch,
        &invalid,
        &mut personality_seeds,
        LOOPBACK,
    )
    .expect("launch outcome has a result");
    assert!(failed.is_err());
    assert_eq!(
        personality_seeds.match_base(),
        first_base,
        "a launch refusal must leave the same seed window available for retry"
    );

    let launched = resolve_new_match(WizardOut::Launch, &draft, &mut personality_seeds, LOOPBACK)
        .expect("launch outcome has a result");
    let Ok(NewMatch::Local(game)) = launched else {
        panic!("a valid draft launches locally");
    };
    assert_eq!(
        game.scenario.players[1]
            .bot_config
            .expect("opponent is configured")
            .personality_seed,
        first_base + 1
    );
    assert_eq!(
        personality_seeds.match_base(),
        first_base.wrapping_add(crate::screens::wizard::launch::BOT_PERSONALITY_WINDOW),
        "one successful match consumes exactly one complete roster window"
    );
}

#[test]
fn a_remote_chair_hosts_the_match_and_a_refused_listen_keeps_the_seeds() {
    let mut personality_seeds = PersonalitySeedSource::from_seed(41);
    let first_base = personality_seeds.match_base();
    let mut draft = configured_new_match_draft();
    draft.seat_choice = 1;
    draft.seats[0].remote = true;

    let taken = std::net::TcpListener::bind(LOOPBACK).unwrap();
    let busy = taken.local_addr().unwrap().to_string();
    let refused = resolve_new_match(WizardOut::Launch, &draft, &mut personality_seeds, &busy)
        .expect("launch outcome has a result");
    assert!(refused.is_err(), "the port is taken");
    assert_eq!(personality_seeds.match_base(), first_base);

    let hosted = resolve_new_match(WizardOut::Launch, &draft, &mut personality_seeds, LOOPBACK)
        .expect("launch outcome has a result");
    let Ok(NewMatch::Hosted(lobby)) = hosted else {
        panic!("a remote chair hosts");
    };
    assert!(
        crate::netplay::Lobby::Host(lobby)
            .status()
            .ends_with("1 of 2 players here")
    );
    assert_eq!(
        personality_seeds.match_base(),
        first_base.wrapping_add(crate::screens::wizard::launch::BOT_PERSONALITY_WINDOW)
    );
}

#[test]
fn restart_and_rematch_rebuild_the_exact_opponents_without_rerolling() {
    let mut personality_seeds = PersonalitySeedSource::from_seed(41);
    let draft = configured_new_match_draft();
    let launched = resolve_new_match(WizardOut::Launch, &draft, &mut personality_seeds, LOOPBACK)
        .expect("launch outcome has a result");
    let Ok(NewMatch::Local(game)) = launched else {
        panic!("a valid draft launches locally");
    };
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

#[test]
fn a_viewer_restores_the_exact_screen_that_opened_it() {
    let game = Game::new(Scenario::skirmish()).expect("game");
    let mut pause = PauseScreen::open(false, true);
    pause.begin_naming("Skirmish | t40");
    let viewer = open_playback(live_playback(&game).expect("viewer"), Screen::Pause(pause));
    let Screen::Playback { session, back } = viewer else {
        panic!("a viewer opens");
    };
    assert!(!session.clock.paused);
    assert!(session.seeking.is_none());
    assert_eq!(
        back.mode(),
        "save_name",
        "leaving lands on the same pause menu, mid-edit, not a fresh one"
    );
}

#[test]
fn every_way_out_of_the_final_map_ends_inspection() {
    let mut game =
        Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
    game.presentation.selection.units.push(oxide_sim::UnitId(1));
    let final_map = Screen::FinalMap(FinalMapScreen::open());
    enter(&mut game, &final_map);
    assert!(game.clock.paused);
    assert!(game.presentation.spectate);
    assert!(game.presentation.selection.units.is_empty());
    // The step keys on the screen left, so a quit-save or a debug load
    // ends inspection as surely as the Back button.
    exit(&mut game, ScreenKind::FinalMap);
    assert!(!game.presentation.spectate);
}

#[test]
fn a_notice_lands_where_the_player_is_looking() {
    let mut game =
        Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
    let notice = || Notice {
        text: "could not save settings: disk full".to_owned(),
        danger: true,
    };
    let deliver = |game: &mut Game, screen: &mut Screen| {
        let mut menu = None;
        deliver_notices(&mut vec![notice()], game, &mut menu, screen, 10.0);
        menu
    };

    assert!(deliver(&mut game, &mut Screen::Playing).is_none());
    assert_eq!(
        game.presentation.toasts.len(),
        1,
        "live play: the HUD toast"
    );

    let mut settings = Screen::Settings {
        screen: SettingsScreen::open(&config::Config::default()),
        back: Box::new(Screen::Home(HomeScreen::open())),
    };
    assert!(deliver(&mut game, &mut settings).is_none());
    let Screen::Settings { screen, .. } = settings else {
        unreachable!()
    };
    assert!(
        screen.notice.is_some_and(|notice| notice.danger),
        "its own line"
    );

    let mut pause = Screen::Pause(PauseScreen::open(false, true));
    assert!(deliver(&mut game, &mut pause).is_none());
    let Screen::Pause(pause) = pause else {
        unreachable!()
    };
    assert_eq!(
        pause.subtitle("Skirmish"),
        "could not save settings: disk full",
        "the pause menu shows it above the veil"
    );

    for mut screen in [
        Screen::Home(HomeScreen::open()),
        Screen::Results(ResultsScreen::open()),
        Screen::FinalMap(FinalMapScreen::open()),
    ] {
        let menu = deliver(&mut game, &mut screen);
        assert_eq!(
            menu,
            Some(("could not save settings: disk full".to_owned(), 18.0)),
            "{}",
            screen.mode()
        );
    }
    assert_eq!(game.presentation.toasts.len(), 1, "nothing else toasts");
}
