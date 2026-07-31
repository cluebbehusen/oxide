//! Deterministic construction-time bot profile contract.

use oxide_sim::Faction;
use oxide_sim::PlayerId;
use oxide_sim::bot::{
    DECISION_STREAM_BASE, Level, NAMED_VARIANT_COUNT, NeuralBot, PROFILE_CONDITION_NAMES,
    PROFILE_ROLE_STREAM, PROFILE_STYLE_STREAM_BASE, PROFILE_VARIANT_STREAM_BASE,
    resolve_bot_profiles, seat_bots,
};
use oxide_sim::scenario::{BotConfig, NamedStyle, Scenario, TeamRole};
use std::collections::BTreeSet;

fn shipped(name: &str) -> Scenario {
    Scenario::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scenarios")
            .join(name),
    )
    .expect("shipped scenario")
}

fn named_config(style: Option<NamedStyle>) -> BotConfig {
    BotConfig {
        level: Level::Medium,
        aggression: None,
        style,
        variant: None,
        team_role: None,
    }
}

fn configure_every_seat(scenario: &mut Scenario, style: Option<NamedStyle>) {
    for player in &mut scenario.players {
        player.bot = true;
        player.bot_config = Some(named_config(style));
    }
}

#[test]
fn old_exact_aggression_json_keeps_its_contract() {
    let config: BotConfig =
        serde_json::from_str(r#"{"level":"hard","aggression":437}"#).expect("old JSON");
    assert_eq!(config.level, Level::Hard);
    assert_eq!(config.aggression, Some(437));
    assert_eq!(config.style, None);
    assert_eq!(config.variant, None);
    assert_eq!(config.team_role, None);
    assert_eq!(
        serde_json::to_value(config).unwrap(),
        serde_json::json!({"level": "hard", "aggression": 437}),
        "additive fields stay absent from an old exact config"
    );
}

#[test]
fn ambiguous_or_incomplete_profile_json_is_rejected_at_the_boundary() {
    for json in [
        r#"{"level":"medium","aggression":500,"style":"balanced"}"#,
        r#"{"level":"medium","variant":1}"#,
        r#"{"level":"medium","style":"turtle","variant":3}"#,
        r#"{"level":"medium","aggression":1001}"#,
    ] {
        assert!(
            serde_json::from_str::<BotConfig>(json).is_err(),
            "invalid profile must not survive parsing: {json}"
        );
    }
}

#[test]
fn automatic_profiles_are_reproducible_and_reach_every_curated_choice() {
    let mut styles = BTreeSet::new();
    let mut variants = BTreeSet::new();
    for seed in 0..256 {
        let mut scenario = Scenario::skirmish();
        scenario.seed = seed;
        configure_every_seat(&mut scenario, None);
        let first = resolve_bot_profiles(&scenario).expect("profiles");
        let second = resolve_bot_profiles(&scenario).expect("same profiles");
        assert_eq!(first, second, "seed {seed} must reproduce exactly");
        for profile in first.into_iter().flatten() {
            styles.insert(profile.style.expect("automatic style"));
            variants.insert(profile.variant.expect("automatic variant"));
        }
    }
    assert_eq!(
        styles,
        NamedStyle::ALL.into_iter().collect(),
        "Surprise Me reaches every named family"
    );
    assert_eq!(
        variants,
        (0..NAMED_VARIANT_COUNT).collect(),
        "automatic deals reach every curated variant"
    );
}

#[test]
fn every_named_variant_stays_inside_its_style_envelope() {
    assert_eq!(
        PROFILE_CONDITION_NAMES,
        [
            "profile_economy",
            "profile_air",
            "profile_siege",
            "profile_support",
            "profile_commitment",
        ]
    );
    for style in NamedStyle::ALL {
        let (min, max) = style.aggression_bounds();
        for variant in 0..NAMED_VARIANT_COUNT {
            let mut scenario = Scenario::skirmish();
            for player in &mut scenario.players {
                player.bot = true;
                player.bot_config = Some(BotConfig {
                    variant: Some(variant),
                    ..named_config(Some(style))
                });
            }
            for profile in resolve_bot_profiles(&scenario)
                .expect("named profiles")
                .into_iter()
                .flatten()
            {
                assert_eq!(profile.style, Some(style));
                assert_eq!(profile.variant, Some(variant));
                assert!((min..=max).contains(&profile.aggression));
                assert!(profile.variant_name().is_some());
                assert!(
                    profile
                        .facets
                        .conditions()
                        .into_iter()
                        .all(|condition| condition <= 1000)
                );
            }
        }
    }
}

#[test]
fn mirrored_hostile_pairs_share_variants_and_complementary_roles() {
    let mut scenario = shipped("gatework-array.json");
    configure_every_seat(&mut scenario, Some(NamedStyle::Balanced));
    let profiles = resolve_bot_profiles(&scenario).expect("profiles");
    for seat in 0..profiles.len() {
        let mirror = profiles.len() - 1 - seat;
        let mine = profiles[seat].expect("configured");
        let theirs = profiles[mirror].expect("configured");
        assert_eq!(
            mine.variant, theirs.variant,
            "seat {seat} and mirror {mirror} get the same strategic draw"
        );
        assert_eq!(
            mine.team_role, theirs.team_role,
            "seat {seat} and mirror {mirror} get the same team job"
        );
    }

    let team_roles = |team| {
        let mut roles: Vec<_> = scenario
            .players
            .iter()
            .enumerate()
            .filter(|(_, player)| player.team == Some(team))
            .map(|(seat, _)| profiles[seat].unwrap().team_role)
            .collect();
        roles.sort();
        roles
    };
    let west = team_roles(0);
    let east = team_roles(1);
    assert_eq!(west, east, "opposing teams receive the same role multiset");
    assert_eq!(
        west.into_iter().collect::<BTreeSet<_>>().len(),
        4,
        "a four-seat team receives all four complementary jobs"
    );
}

#[test]
fn one_authored_variant_and_role_are_mirrored_without_per_seat_duplication() {
    let mut scenario = shipped("gatework-array.json");
    configure_every_seat(&mut scenario, Some(NamedStyle::Turtle));
    scenario.players[0].bot_config = Some(BotConfig {
        variant: Some(2),
        team_role: Some(TeamRole::Siege),
        ..named_config(Some(NamedStyle::Turtle))
    });
    let profiles = resolve_bot_profiles(&scenario).expect("profiles");
    for seat in [0, 7] {
        let profile = profiles[seat].unwrap();
        assert_eq!(profile.variant, Some(2));
        assert_eq!(profile.team_role, TeamRole::Siege);
    }
}

#[test]
fn geometric_teammates_keep_complementary_explicit_profiles() {
    let mut scenario = shipped("gatework-array.json");
    configure_every_seat(&mut scenario, Some(NamedStyle::Turtle));
    // Deliberately author the 180-degree images of seats 0 and 7 onto
    // one team. Profile mirroring applies only across a hostile pair;
    // teammates must remain free to take complementary jobs.
    for (seat, team) in [0, 0, 1, 1, 1, 1, 0, 0].into_iter().enumerate() {
        scenario.players[seat].team = Some(team);
    }
    scenario.players[0].bot_config = Some(BotConfig {
        variant: Some(0),
        team_role: Some(TeamRole::Vanguard),
        ..named_config(Some(NamedStyle::Turtle))
    });
    scenario.players[7].bot_config = Some(BotConfig {
        variant: Some(2),
        team_role: Some(TeamRole::Industry),
        ..named_config(Some(NamedStyle::Turtle))
    });

    let profiles = resolve_bot_profiles(&scenario).expect("teammate profiles do not conflict");
    assert_eq!(profiles[0].unwrap().variant, Some(0));
    assert_eq!(profiles[0].unwrap().team_role, TeamRole::Vanguard);
    assert_eq!(profiles[7].unwrap().variant, Some(2));
    assert_eq!(profiles[7].unwrap().team_role, TeamRole::Industry);
}

#[test]
fn an_exact_aggression_override_bypasses_named_dealing_exactly() {
    let mut scenario = Scenario::skirmish();
    scenario.players[1].bot_config = Some(BotConfig {
        level: Level::Expert,
        aggression: Some(437),
        style: None,
        variant: None,
        team_role: Some(TeamRole::Generalist),
    });
    let profile = resolve_bot_profiles(&scenario).unwrap()[1].unwrap();
    assert_eq!(profile.level, Level::Expert);
    assert_eq!(profile.aggression, 437);
    assert_eq!(profile.style, None);
    assert_eq!(profile.variant, None);
    assert_eq!(profile.variant_name(), None);
    assert_eq!(
        profile.conditions(Faction::Cupric),
        oxide_sim::bot::ladder_condition_values(437, Faction::Cupric),
        "the resolved exact override feeds the existing v7 contract unchanged"
    );
}

#[test]
fn construction_time_profile_streams_do_not_shift_hesitation() {
    assert_ne!(PROFILE_STYLE_STREAM_BASE, DECISION_STREAM_BASE);
    assert_ne!(PROFILE_VARIANT_STREAM_BASE, DECISION_STREAM_BASE);
    assert_ne!(PROFILE_ROLE_STREAM, DECISION_STREAM_BASE);

    let mut scenario = Scenario::skirmish();
    scenario.seed = 9_871;
    scenario.players[1].bot_config = Some(BotConfig {
        level: Level::Medium,
        aggression: Some(550),
        style: None,
        variant: None,
        team_role: None,
    });
    let mut resolved_state = scenario.build().unwrap();
    let mut direct_state = resolved_state.clone();
    let mut resolved = seat_bots(&scenario).remove(0);
    let mut direct = NeuralBot::ladder(
        PlayerId(1),
        scenario.seed,
        Level::Medium,
        Some(550),
        scenario.players[1].faction,
    );

    for _ in 0..1_200 {
        let resolved_commands = resolved.act(&resolved_state);
        let direct_commands = direct.act(&direct_state);
        assert_eq!(
            resolved_commands, direct_commands,
            "profile resolution cannot advance the hesitation stream"
        );
        resolved_state.tick(&resolved_commands);
        direct_state.tick(&direct_commands);
        assert_eq!(resolved_state.hash(), direct_state.hash());
    }
}
