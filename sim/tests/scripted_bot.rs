//! Player-facing scripted opponent contracts.

use oxide_sim::bot::{Brain, Dials, seat_bots};
use oxide_sim::scenario::BotConfig;
use oxide_sim::{GameResult, PlayerId, Scenario};

#[test]
fn seating_keeps_scripted_and_empty_chairs_distinct() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::Scripted);
    scenario.players[1].bot = true;
    scenario.players[1].bot_config = Some(BotConfig::Scripted);

    let bots = seat_bots(&scenario);
    assert_eq!(bots.len(), 2);
    assert_eq!(bots[0].player(), PlayerId(0));
    assert_eq!(bots[1].player(), PlayerId(1));

    scenario.players[0].bot_config = None;
    let bots = seat_bots(&scenario);
    assert_eq!(
        bots.len(),
        1,
        "a config-less bot flag remains an empty chair"
    );
    assert_eq!(bots[0].player(), PlayerId(1));
}

#[test]
fn bot_config_writes_one_current_shape_and_reads_known_legacy_shapes() {
    let json = serde_json::to_string(&BotConfig::Scripted).expect("scripted config serializes");
    assert_eq!(json, r#"{"controller":"scripted"}"#);
    assert_eq!(
        serde_json::from_str::<BotConfig>(&json).expect("scripted config round-trips"),
        BotConfig::Scripted
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"controller":"scripted","level":"hard"}"#).is_err(),
        "retired controller settings are not silently ignored"
    );
    assert_eq!(
        serde_json::from_str::<BotConfig>(r#"{"level":"medium"}"#)
            .expect("the minimal legacy shape remains readable"),
        BotConfig::Scripted
    );
    assert_eq!(
        serde_json::from_str::<BotConfig>(
            r#"{"level":"expert","style":"aggressive","variant":2,"team_role":"vanguard"}"#
        )
        .expect("the complete legacy shape remains readable"),
        BotConfig::Scripted
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"level":"medium","mystery":1}"#).is_err(),
        "legacy compatibility does not admit unknown fields"
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"level":"impossible"}"#).is_err(),
        "legacy compatibility accepts only historical levels"
    );
    assert!(
        serde_json::from_str::<BotConfig>(
            r#"{"level":"medium","aggression":500,"style":"balanced"}"#
        )
        .is_err(),
        "mutually exclusive historical personality fields remain invalid"
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"level":"medium","variant":1}"#).is_err(),
        "a historical variant still requires its style"
    );
    assert!(
        serde_json::from_str::<BotConfig>(r#"{"level":"medium","aggression":1001}"#).is_err(),
        "historical aggression remains bounded"
    );
}

#[test]
fn balanced_is_the_full_fog_honest_tree_without_redefining_the_overseer() {
    let balanced = Dials::balanced();
    assert!(balanced.fog_honest);
    assert!(balanced.tech);
    assert!(balanced.turret_response);
    assert!(balanced.scouting);
    assert!(balanced.aa_response);
    assert!(balanced.radar);
    assert!(balanced.reclaimers);
    assert!(balanced.repair);
    assert!(balanced.air_harass);
    assert!(balanced.salvage);
    assert!(balanced.deep_tech);
    assert!(balanced.extractors);
    assert!(balanced.upgrades);
    assert!(balanced.expansion);
    assert!(balanced.ferry);
    assert!(balanced.mines);

    let scripted = Brain::balanced(PlayerId(1), 73);
    let overseer = Brain::overseer(PlayerId(1), 73);
    assert_eq!(scripted.dials(), overseer.dials());
}

#[test]
fn scripted_seat_is_deterministic_and_makes_progress_past_the_opening() {
    let mut scenario = Scenario::skirmish();
    scenario.players[1].bot = true;
    scenario.players[1].bot_config = Some(BotConfig::Scripted);
    let mut left = scenario.build().expect("skirmish builds");
    let mut right = scenario.build().expect("skirmish builds again");
    let mut left_bots = seat_bots(&scenario);
    let mut right_bots = seat_bots(&scenario);
    let starting_units = left.units().len();
    let starting_buildings = left.buildings().len();
    let mut active_thinks = 0_u32;

    // Four simulated minutes: enough to exercise
    // harvesting, production, construction, and the first strategic
    // transition rather than merely accepting an opening command.
    for _ in 0..4_800 {
        let left_commands: Vec<_> = left_bots
            .iter_mut()
            .flat_map(|bot| bot.act(&left))
            .collect();
        let right_commands: Vec<_> = right_bots
            .iter_mut()
            .flat_map(|bot| bot.act(&right))
            .collect();
        assert_eq!(left_commands, right_commands);
        active_thinks += u32::from(!left_commands.is_empty());
        left.tick(&left_commands);
        right.tick(&right_commands);
        assert_eq!(left.hash(), right.hash());
    }

    assert!(
        active_thinks > 10,
        "the scripted seat stopped issuing commands"
    );
    assert!(
        left.units().len() > starting_units || left.buildings().len() > starting_buildings,
        "the scripted seat never turned its economy into a unit or structure"
    );
}

#[test]
fn balanced_mirror_plays_a_complete_decisive_match() {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
        player.bot_config = Some(BotConfig::Scripted);
    }
    let mut state = scenario.build().expect("skirmish builds");
    let mut bots = seat_bots(&scenario);

    for _ in 0..30_000 {
        if state.result().is_some() {
            break;
        }
        let commands: Vec<_> = bots.iter_mut().flat_map(|bot| bot.act(&state)).collect();
        state.tick(&commands);
    }

    assert!(
        matches!(state.result(), Some(GameResult::Victory { .. })),
        "the player-facing mirror should finish a real game: {:?}",
        state.result()
    );
}
