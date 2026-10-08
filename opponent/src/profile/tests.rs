use super::*;

#[test]
fn resolution_is_repeatable_and_difficulty_does_not_change_personality() {
    let config = BotConfig::new(BotDifficulty::Prime, BotStance::Aggressive, 0xC0_FF_EE);
    let first = ResolvedProfile::resolve(config);
    let second = ResolvedProfile::resolve(config);
    assert_eq!(first, second);

    let lower = ResolvedProfile::resolve(BotConfig::new(
        BotDifficulty::Scrapheap,
        config.stance,
        config.personality_seed,
    ));
    assert_eq!(first.primary, lower.primary);
    assert_eq!(first.secondary, lower.secondary);
    assert_eq!(first.traits, lower.traits);
}

#[test]
fn every_profile_has_two_distinct_specialties_inside_its_stance_envelope() {
    for stance in [
        BotStance::Turtle,
        BotStance::Balanced,
        BotStance::Aggressive,
    ] {
        let bounds = envelope(stance);
        for seed in 0..2_000 {
            let profile =
                ResolvedProfile::resolve(BotConfig::new(BotDifficulty::Standard, stance, seed));
            assert_ne!(profile.primary, profile.secondary);
            for specialty in Specialty::ALL {
                let value = profile.traits.get(specialty);
                let bound = bounds[specialty.index()];
                assert!(
                    (bound.min..=bound.max).contains(&value),
                    "{stance:?} seed {seed} put {specialty:?} at {value} outside {}..={}",
                    bound.min,
                    bound.max
                );
            }
        }
    }
}

#[test]
fn stance_preserves_its_promised_identity_without_collapsing_guile() {
    let mut guile_ranges = [(u8::MAX, u8::MIN); 3];
    for seed in 0..2_000 {
        let turtle = ResolvedProfile::resolve(BotConfig::new(
            BotDifficulty::Prime,
            BotStance::Turtle,
            seed,
        ));
        assert!(turtle.traits.fortification >= 65);
        assert!(turtle.traits.support >= 50);
        guile_ranges[0].0 = guile_ranges[0].0.min(turtle.traits.guile);
        guile_ranges[0].1 = guile_ranges[0].1.max(turtle.traits.guile);

        let balanced = ResolvedProfile::resolve(BotConfig::new(
            BotDifficulty::Prime,
            BotStance::Balanced,
            seed,
        ));
        assert_eq!(turtle.traits.guile, balanced.traits.guile);
        guile_ranges[1].0 = guile_ranges[1].0.min(balanced.traits.guile);
        guile_ranges[1].1 = guile_ranges[1].1.max(balanced.traits.guile);

        let aggressive = ResolvedProfile::resolve(BotConfig::new(
            BotDifficulty::Prime,
            BotStance::Aggressive,
            seed,
        ));
        assert!(aggressive.traits.fortification <= 50);
        assert_eq!(balanced.traits.guile, aggressive.traits.guile);
        guile_ranges[2].0 = guile_ranges[2].0.min(aggressive.traits.guile);
        guile_ranges[2].1 = guile_ranges[2].1.max(aggressive.traits.guile);
    }

    for (minimum, maximum) in guile_ranges {
        assert!(minimum <= 34, "guile never produced a blunt strategist");
        assert!(maximum >= 80, "guile never produced a dedicated raider");
    }
}

#[test]
fn profiles_have_a_fixed_budget_and_truthful_ranked_specialties() {
    for stance in BotStance::ALL {
        for seed in 0..10_000 {
            let profile =
                ResolvedProfile::resolve(BotConfig::new(BotDifficulty::Prime, stance, seed));
            let total: u16 = Specialty::ALL
                .iter()
                .map(|specialty| u16::from(profile.traits.get(*specialty)))
                .sum();
            assert_eq!(total, TRAIT_BUDGET as u16, "{stance:?} seed {seed}");
            assert!(
                profile.traits.get(profile.primary) >= profile.traits.get(profile.secondary),
                "{stance:?} seed {seed} mislabeled its primary"
            );
            for specialty in Specialty::ALL {
                if specialty != profile.primary && specialty != profile.secondary {
                    assert!(
                        profile.traits.get(profile.secondary) >= profile.traits.get(specialty),
                        "{stance:?} seed {seed} mislabeled its secondary"
                    );
                }
            }
        }
    }
}

#[test]
fn golden_profiles_pin_full_width_seed_resolution() {
    let cases = [
        (
            BotDifficulty::Prime,
            BotStance::Turtle,
            0,
            Specialty::Fortification,
            Specialty::Guile,
            PersonalityTraits {
                air: 30,
                siege: 41,
                support: 53,
                fortification: 65,
                greed: 53,
                guile: 58,
            },
        ),
        (
            BotDifficulty::Veteran,
            BotStance::Balanced,
            0x8000_0000_0000_0001,
            Specialty::Guile,
            Specialty::Fortification,
            PersonalityTraits {
                air: 41,
                siege: 48,
                support: 50,
                fortification: 58,
                greed: 44,
                guile: 59,
            },
        ),
        (
            BotDifficulty::Scrapheap,
            BotStance::Aggressive,
            u64::MAX,
            Specialty::Air,
            Specialty::Siege,
            PersonalityTraits {
                air: 69,
                siege: 63,
                support: 39,
                fortification: 32,
                greed: 59,
                guile: 38,
            },
        ),
    ];

    for (difficulty, stance, seed, primary, secondary, traits) in cases {
        let profile = ResolvedProfile::resolve(BotConfig::new(difficulty, stance, seed));
        assert_eq!(
            profile,
            ResolvedProfile {
                difficulty,
                stance,
                personality_seed: seed,
                primary,
                secondary,
                traits,
            }
        );
    }

    let low =
        ResolvedProfile::resolve(BotConfig::new(BotDifficulty::Prime, BotStance::Balanced, 1));
    let high = ResolvedProfile::resolve(BotConfig::new(
        BotDifficulty::Prime,
        BotStance::Balanced,
        (1_u64 << 63) | 1,
    ));
    assert_ne!(
        low.traits, high.traits,
        "high seed bits must affect resolution"
    );
}

#[test]
fn independent_stream_ids_and_specialty_deals_cover_the_whole_surface() {
    let mut streams = vec![PRIMARY_STREAM, SECONDARY_STREAM, NORMALIZATION_STREAM];
    streams.extend((0..Specialty::ALL.len()).map(|index| TRAIT_STREAM_BASE + index as u64));
    streams.sort_unstable();
    streams.dedup();
    assert_eq!(streams.len(), 3 + Specialty::ALL.len());

    let mut primaries = [false; Specialty::ALL.len()];
    let mut secondaries = [false; Specialty::ALL.len()];
    for seed in 0..10_000 {
        let primary = choose_primary(seed);
        let secondary = choose_secondary(seed, primary);
        primaries[primary.index()] = true;
        secondaries[secondary.index()] = true;
    }
    assert!(primaries.into_iter().all(|seen| seen));
    assert!(secondaries.into_iter().all(|seen| seen));
}
