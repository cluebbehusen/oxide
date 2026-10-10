use super::*;

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
fn collided_names_take_ordinals() {
    let mut scenario = Scenario::skirmish();
    let repeated = scenario.players[0].name.clone();
    scenario.players[1].name.clone_from(&repeated);
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(scenario, None);
    let game = launch(&draft, 0x5000).expect("a repeated authored name never refuses to launch");
    let players = &game.scenario.players;
    assert_eq!(players[0].name, repeated, "the first seat keeps its name");
    assert_eq!(
        players[1].name,
        format!("{repeated} 2"),
        "the duplicate label took an ordinal"
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
    // team, a moved seat joins its new one.
    let mut draft = team_draft();
    draft.seats[0].team_choice = 0; // FFA
    draft.seats[3].team_choice = 1; // crosses to Team 1
    let game = launch(&draft, 0x8000).expect("launches");
    let players = &game.scenario.players;
    assert_eq!(players[0].team, None, "the FFA seat drops its team");
    assert_eq!(players[3].team, Some(0), "the moved seat joined Team 1");
    assert_eq!(players[1].team, Some(0));
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
