use super::*;

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
    let replay = oxide_kit::GameReplay::new(oxide_sim::SIM_VERSION, Scenario::skirmish());
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
fn local_request_guards_follow_screen_ownership() {
    let advance = Request::AdvanceTicks { ticks: 8 };
    let send = Request::SendCommand {
        player: oxide_sim::PlayerId(0),
        command: oxide_sim::Command::Surrender,
    };
    let camera = Request::QueryCamera;

    // The frozen final map refuses time and mutation before shared
    // dispatch can advance the hidden live match.
    assert_eq!(route(false, true, None, &advance), Route::RefuseFrozen);
    assert_eq!(route(false, true, None, &send), Route::RefuseFrozen);
    assert_eq!(route(false, true, None, &camera), Route::Local);

    // The read-only viewer bounces local mutation and answers
    // local reads.
    assert_eq!(route(true, false, None, &send), Route::RefuseViewer);
    assert_eq!(route(true, false, None, &camera), Route::Local);

    // The live screen answers everything else locally.
    assert_eq!(route(false, false, None, &send), Route::Local);
}

/// A LAN match refuses the clock and other seats before shared
/// dispatch, and only its host may pause.
#[test]
fn a_lan_match_refuses_its_clock_and_other_seats() {
    use crate::game::network::NetRole;
    let seat = oxide_sim::PlayerId(1);
    let host = Some((NetRole::Host, seat));
    let client = Some((NetRole::Client, seat));
    let send = |player| Request::SendCommand {
        player: oxide_sim::PlayerId(player),
        command: oxide_sim::Command::Surrender,
    };
    for net in [host, client] {
        for refused in [
            Request::AdvanceTicks { ticks: 1 },
            Request::PresentTicks { ticks: 1 },
            Request::SetSpeed { multiplier: 2.0 },
            send(0),
        ] {
            assert_eq!(
                route(false, false, net, &refused),
                Route::RefuseLockstep,
                "{refused:?}"
            );
        }
        assert_eq!(route(false, false, net, &send(1)), Route::Local);
        assert_eq!(
            route(false, false, net, &Request::QueryCamera),
            Route::Local
        );
    }
    for pause in [Request::Pause, Request::Resume] {
        assert_eq!(route(false, false, host, &pause), Route::Local);
        assert_eq!(route(false, false, client, &pause), Route::RefuseLockstep);
    }
    assert_eq!(
        route(true, false, client, &Request::Pause),
        Route::Local,
        "the viewer keeps its own clock"
    );
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
fn every_sound_kind_mixes_from_its_manifest_row() {
    for kind in SoundKind::ALL {
        let spec = mixer_spec(kind);
        assert!(spec.volume > 0.0 && spec.min_gap > 0.0, "{kind:?}");
    }
    assert_eq!(MIXER_SPECS.get("laser2"), MIXER_SPECS.get("laser"));
}

fn team_draft() -> NewMatchDraft {
    let mut draft = NewMatchDraft::default();
    let scenario = Scenario::load("../scenarios/trident-plateau.json").expect("shipped map");
    draft.set_scenario(scenario, None);
    draft
}

#[test]
fn remote_chairs_launch_as_humans_without_a_bot() {
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(Scenario::skirmish(), None);
    draft.seats[0].remote = true;
    draft.seats[1].remote = true;
    draft.seat_choice = 1;
    let scenario = draft_scenario(&draft, 0x1000).expect("builds");
    assert!(scenario.players.iter().all(|player| !player.bot));
    assert!(
        scenario
            .players
            .iter()
            .all(|player| player.bot_config.is_none())
    );
    draft.seats[0].remote = false;
    let scenario = draft_scenario(&draft, 0x1000).expect("builds");
    assert!(scenario.players[0].bot && scenario.players[0].bot_config.is_some());
    assert!(
        !scenario.players[1].bot,
        "the human's chair ignores its remote flag"
    );
}

#[test]
fn launch_materializes_each_opponents_exact_visible_config_and_seed() {
    let mut draft = team_draft();
    draft.seat_choice = 2;
    draft.seats[0].difficulty = oxide_sim::scenario::BotDifficulty::Prime;
    draft.seats[0].stance = oxide_sim::scenario::BotStance::Aggressive;
    draft.seats[5].difficulty = oxide_sim::scenario::BotDifficulty::Scrapheap;
    draft.seats[5].stance = oxide_sim::scenario::BotStance::Turtle;
    let game = launch(&draft, 0x1000).expect("launches");
    let players = &game.scenario.players;
    assert!(!players[2].bot, "the chosen chair is the human's");
    assert_eq!(game.presentation.human, oxide_sim::PlayerId(2));
    for (i, p) in players.iter().enumerate() {
        if i == 2 {
            assert!(p.bot_config.is_none());
            continue;
        }
        assert!(p.bot, "every other seat is a bot");
        assert_eq!(
            p.bot_config,
            Some(oxide_sim::scenario::BotConfig::new(
                draft.seats[i].difficulty,
                draft.seats[i].stance,
                0x1000 + i as u64,
            )),
            "every opponent receives its exact draft and seat seed"
        );
    }
    // Auto chips keep the seat's authored faction.
    assert_eq!(players[2].faction, oxide_sim::Faction::Ferrous);
}

#[test]
fn personality_seed_sources_are_repeatable_and_reserve_distinct_windows() {
    let mut left = PersonalitySeedSource::from_seed(41);
    let mut right = PersonalitySeedSource::from_seed(41);
    let first = left.match_base();
    assert_eq!(first, right.match_base());
    assert_eq!(
        first,
        left.match_base(),
        "reading a base does not consume it"
    );
    assert_eq!(first & 0xF, 0, "a match base begins a 16-seat window");

    left.commit_launch();
    right.commit_launch();
    let second = left.match_base();
    assert_eq!(second, right.match_base());
    assert_eq!(second, first.wrapping_add(BOT_PERSONALITY_WINDOW));
    let first_seeds: Vec<u64> = (0..16).map(|seat| first + seat).collect();
    let second_seeds: Vec<u64> = (0..16).map(|seat| second + seat).collect();
    assert!(
        first_seeds.iter().all(|seed| !second_seeds.contains(seed)),
        "consecutive launches cannot share an opponent identity"
    );

    let other = PersonalitySeedSource::from_seed(42);
    assert_ne!(first, other.match_base());
}

#[test]
fn automation_uses_one_repeatable_seed_stream_but_ordinary_sessions_use_entropy() {
    let automated = PersonalitySeedSource::for_session_with_entropy(true, || {
        panic!("automation must not consult ambient entropy")
    });
    assert_eq!(
        automated.match_base(),
        PersonalitySeedSource::for_session_with_entropy(true, || {
            panic!("automation must not consult ambient entropy")
        })
        .match_base()
    );

    let ordinary = PersonalitySeedSource::for_session_with_entropy(false, || {
        PersonalitySeedSource::from_seed(41)
    });
    assert_eq!(
        ordinary.match_base(),
        PersonalitySeedSource::from_seed(41).match_base(),
        "ordinary New Match sessions retain their entropy-selected stream"
    );
    assert_ne!(ordinary.match_base(), automated.match_base());
}

#[test]
fn launch_seed_scope_is_exact_and_reproducible() {
    let mut draft = team_draft();
    draft.seat_choice = 1;
    draft.seats[0].difficulty = oxide_sim::scenario::BotDifficulty::Veteran;
    draft.seats[0].stance = oxide_sim::scenario::BotStance::Turtle;
    draft.seats[4].difficulty = oxide_sim::scenario::BotDifficulty::Prime;
    draft.seats[4].stance = oxide_sim::scenario::BotStance::Aggressive;

    let first = launch(&draft, 0xABC0).expect("first launch");
    let repeated = launch(&draft, 0xABC0).expect("repeated launch");
    assert_eq!(first.scenario, repeated.scenario);
    assert_eq!(first.hash_hex(), repeated.hash_hex());
    assert_eq!(first.recorder.setup, repeated.recorder.setup);

    let rerolled = launch(&draft, 0xDEF0).expect("rerolled launch");
    assert_eq!(first.hash_hex(), rerolled.hash_hex());
    for seat in 0..first.scenario.players.len() {
        let left = &first.scenario.players[seat];
        let right = &rerolled.scenario.players[seat];
        assert_eq!(left.name, right.name);
        assert_eq!(left.faction, right.faction);
        assert_eq!(left.team, right.team);
        assert_eq!(left.scrap, right.scrap);
        assert_eq!(left.bot, right.bot);
        match (left.bot_config, right.bot_config) {
            (Some(left), Some(right)) => {
                assert_eq!(left.difficulty, right.difficulty);
                assert_eq!(left.stance, right.stance);
                assert_ne!(left.personality_seed, right.personality_seed);
            }
            (None, None) => {}
            mismatch => panic!("controller presence changed: {mismatch:?}"),
        }
    }
}

#[test]
fn restart_rematch_and_replay_keep_exact_opponent_identities() {
    let mut draft = team_draft();
    draft.seats[1].difficulty = oxide_sim::scenario::BotDifficulty::Prime;
    draft.seats[1].stance = oxide_sim::scenario::BotStance::Aggressive;
    let game = launch(&draft, 0xCAFE_F000).expect("launch");
    let expected = game.scenario.clone();

    // Restart and Rematch both use this exact construction path.
    let restarted = Game::new(game.scenario.clone()).expect("restart");
    assert_eq!(restarted.scenario, expected);
    assert_eq!(restarted.recorder.setup, expected);
    assert_eq!(restarted.hash_hex(), game.hash_hex());

    let resumed = Game::from_replay(game.recorder.clone()).expect("resume replay");
    assert_eq!(resumed.scenario, expected);
    assert_eq!(resumed.recorder.setup, expected);
    assert_eq!(resumed.hash_hex(), game.hash_hex());
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
fn a_zero_seat_map_refuses_to_launch_instead_of_panicking() {
    // Discovery lists any parseable JSON, so a `players: []` file must
    // refuse instead of underflowing the seat clamp.
    let mut scenario = Scenario::skirmish();
    scenario.players.clear();
    scenario.units.clear();
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(scenario, None);
    assert!(
        launch(&draft, 0x4000).is_err(),
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
    let game = launch(&draft, 0x5000).expect("a legitimate faction choice never refuses to launch");
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
    let game = launch(&draft, 0x6000).expect("launches");
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
    let game = launch(&draft, 0x7000).expect("launches");
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
    let game = launch(&draft, 0x8000).expect("launches");
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
            .all(|i| game.state.player(oxide_sim::PlayerId(i.fit::<u8>())).team != alone),
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
    assert!(launch(&draft, 0x9000).is_err(), "one team can never launch");
}

#[test]
fn a_stale_draft_fails_the_launch_instead_of_the_process() {
    // The caller shows launch errors on a menu notice; the fn's
    // contract is Err, never panic, on a draft out of step.
    let mut draft = team_draft();
    draft.seats.truncate(2);
    assert!(launch(&draft, 0xA000).is_err());
}

#[test]
fn soundtrack_context_tracks_pause_victory_and_surrender() {
    let mut won = Game::new(Scenario::skirmish()).expect("game");
    assert_eq!(
        match_soundtrack_scene(&won.view(), false),
        crate::soundtrack::Scene::Match
    );
    assert_eq!(
        match_soundtrack_scene(&won.view(), true),
        crate::soundtrack::Scene::Pause
    );
    won.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: oxide_sim::Command::Surrender,
    }]);
    assert_eq!(
        match_soundtrack_scene(&won.view(), false),
        crate::soundtrack::Scene::Victory
    );

    let mut lost = Game::new(Scenario::skirmish()).expect("game");
    lost.state.tick(&[oxide_sim::PlayerCommand {
        player: lost.presentation.human,
        command: oxide_sim::Command::Surrender,
    }]);
    assert_eq!(
        match_soundtrack_scene(&lost.view(), false),
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
