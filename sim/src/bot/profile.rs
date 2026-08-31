//! Deterministic personality resolution for the player-facing bot.

use crate::scenario::{BotConfig, BotDifficulty, BotStance};
use chassis::rng::Pcg32;
use serde::Serialize;

const PRIMARY_STREAM: u64 = 0x0B07_1600;
const SECONDARY_STREAM: u64 = 0x0B07_1601;
const TRAIT_STREAM_BASE: u64 = 0x0B07_1610;
const NORMALIZATION_STREAM: u64 = 0x0B07_1620;
const TRAIT_JITTER: i16 = 7;
const GUILE_JITTER: i16 = 18;
const PRIMARY_BONUS: i16 = 16;
const SECONDARY_BONUS: i16 = 8;
/// Personality changes priorities, never the total amount of preference the
/// planner can spend. A fixed budget prevents a lucky seed from becoming an
/// accidental fifth difficulty level.
const TRAIT_BUDGET: i16 = 300;

/// A strategic preference that can become a seeded specialty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Specialty {
    /// Aircraft, escorts, and bombing operations.
    Air,
    /// Artillery, standoff pressure, and suppression.
    Siege,
    /// Repair, escorts, and preserving expensive forces.
    Support,
    /// Static defense, mines, and counterattacks.
    Fortification,
    /// Economy, expansion, upgrades, and technology.
    Greed,
    /// Raids, feints, target switching, and withdrawal.
    Guile,
}

impl Specialty {
    /// Every personality axis in stable wire and resolver order.
    pub const ALL: [Self; 6] = [
        Self::Air,
        Self::Siege,
        Self::Support,
        Self::Fortification,
        Self::Greed,
        Self::Guile,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Air => 0,
            Self::Siege => 1,
            Self::Support => 2,
            Self::Fortification => 3,
            Self::Greed => 4,
            Self::Guile => 5,
        }
    }

    const fn from_index(index: usize) -> Self {
        Self::ALL[index]
    }
}

/// The six bounded personality preferences resolved from one seed.
///
/// Values use a `0..=100` scale. They rank otherwise legal strategic choices;
/// they never change costs, vision, prerequisites, or unit strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PersonalityTraits {
    /// Preference for aircraft and air operations.
    pub air: u8,
    /// Preference for artillery and standoff pressure.
    pub siege: u8,
    /// Preference for repair, escorts, and sustain.
    pub support: u8,
    /// Preference for static defense and counterattack preparation.
    pub fortification: u8,
    /// Preference for economy, expansion, upgrades, and technology.
    pub greed: u8,
    /// Preference for asymmetric raids and opportunistic withdrawal.
    pub guile: u8,
}

impl PersonalityTraits {
    /// Returns one preference by its strategic axis.
    pub const fn get(self, specialty: Specialty) -> u8 {
        match specialty {
            Specialty::Air => self.air,
            Specialty::Siege => self.siege,
            Specialty::Support => self.support,
            Specialty::Fortification => self.fortification,
            Specialty::Greed => self.greed,
            Specialty::Guile => self.guile,
        }
    }
}

/// A complete, deterministic personality resolved before a match begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResolvedProfile {
    /// Skill rung, kept separate from personality preferences.
    pub difficulty: BotDifficulty,
    /// Player-selected posture whose envelope bounds the hidden traits.
    pub stance: BotStance,
    /// Seed from the authored match setup.
    pub personality_seed: u64,
    /// Highest-ranked strategic preference after stance normalization.
    pub primary: Specialty,
    /// Second-ranked strategic wrinkle, always distinct from `primary`.
    pub secondary: Specialty,
    /// Correlated and stance-bounded preference values.
    pub traits: PersonalityTraits,
}

impl ResolvedProfile {
    /// Resolves a config without consuming simulation randomness.
    ///
    /// Specialty selection and every trait use dedicated PCG streams. Changing
    /// one trait's sampling cannot shift any other trait or the chosen roles.
    pub fn resolve(config: BotConfig) -> Self {
        let dealt_primary = choose_primary(config.personality_seed);
        let dealt_secondary = choose_secondary(config.personality_seed, dealt_primary);
        let envelope = envelope(config.stance);
        let mut values = Specialty::ALL.map(|specialty| {
            resolve_trait(
                config.personality_seed,
                specialty,
                envelope[specialty.index()],
                dealt_primary,
                dealt_secondary,
            )
        });
        let non_guile_budget = TRAIT_BUDGET - i16::from(values[Specialty::Guile.index()]);
        normalize_non_guile(
            config.personality_seed,
            &mut values,
            &envelope,
            non_guile_budget,
        );
        let (primary, secondary) = ranked_specialties(&values, dealt_primary, dealt_secondary);

        Self {
            difficulty: config.difficulty,
            stance: config.stance,
            personality_seed: config.personality_seed,
            primary,
            secondary,
            traits: PersonalityTraits {
                air: values[0],
                siege: values[1],
                support: values[2],
                fortification: values[3],
                greed: values[4],
                guile: values[5],
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Envelope {
    center: u8,
    min: u8,
    max: u8,
}

const fn band(center: u8, min: u8, max: u8) -> Envelope {
    Envelope { center, min, max }
}

fn envelope(stance: BotStance) -> [Envelope; 6] {
    match stance {
        BotStance::Turtle => [
            band(44, 25, 72),
            band(58, 40, 84),
            band(65, 50, 90),
            band(76, 65, 96),
            band(55, 34, 78),
            band(50, 24, 86),
        ],
        BotStance::Balanced => [
            band(50, 30, 78),
            band(50, 30, 78),
            band(50, 32, 78),
            band(50, 30, 78),
            band(50, 30, 78),
            band(50, 24, 86),
        ],
        BotStance::Aggressive => [
            band(58, 38, 86),
            band(56, 36, 84),
            band(43, 25, 68),
            band(30, 14, 50),
            band(44, 25, 68),
            band(50, 24, 86),
        ],
    }
}

fn choose_primary(seed: u64) -> Specialty {
    let mut rng = Pcg32::new(seed, PRIMARY_STREAM);
    Specialty::from_index(rng.next_below(Specialty::ALL.len() as u32) as usize)
}

fn choose_secondary(seed: u64, primary: Specialty) -> Specialty {
    let mut rng = Pcg32::new(seed, SECONDARY_STREAM);
    let offset = rng.next_below((Specialty::ALL.len() - 1) as u32) as usize + 1;
    Specialty::from_index((primary.index() + offset) % Specialty::ALL.len())
}

fn resolve_trait(
    seed: u64,
    specialty: Specialty,
    envelope: Envelope,
    primary: Specialty,
    secondary: Specialty,
) -> u8 {
    let mut rng = Pcg32::new(seed, TRAIT_STREAM_BASE + specialty.index() as u64);
    let radius = if specialty == Specialty::Guile {
        GUILE_JITTER
    } else {
        TRAIT_JITTER
    };
    let jitter = rng.next_below((radius * 2 + 1) as u32) as i16 - radius;
    let specialty_bonus = if specialty == primary {
        PRIMARY_BONUS
    } else if specialty == secondary {
        SECONDARY_BONUS
    } else {
        0
    };
    (i16::from(envelope.center) + jitter + specialty_bonus)
        .clamp(i16::from(envelope.min), i16::from(envelope.max)) as u8
}

/// Rebalances the five stance-shaped axes around the independently sampled
/// guile score. The order is seed-derived, so normalization does not favor an
/// enum position when two axes have equal room.
fn normalize_non_guile(seed: u64, values: &mut [u8; 6], envelope: &[Envelope; 6], target: i16) {
    let mut order = [0usize, 1, 2, 3, 4];
    let mut rng = Pcg32::new(seed, NORMALIZATION_STREAM);
    let keys = order.map(|_| rng.next_u32());
    order.sort_by_key(|index| (keys[*index], *index));

    let mut total: i16 = values[..5].iter().map(|value| i16::from(*value)).sum();
    while total != target {
        let increase = total < target;
        let mut moved = false;
        for index in order {
            if total == target {
                break;
            }
            let bound = envelope[index];
            if increase && values[index] < bound.max {
                values[index] += 1;
                total += 1;
                moved = true;
            } else if !increase && values[index] > bound.min {
                values[index] -= 1;
                total -= 1;
                moved = true;
            }
        }
        assert!(
            moved,
            "stance envelope cannot satisfy the personality budget"
        );
    }
}

fn ranked_specialties(
    values: &[u8; 6],
    dealt_primary: Specialty,
    dealt_secondary: Specialty,
) -> (Specialty, Specialty) {
    let mut ranked = Specialty::ALL;
    ranked.sort_by_key(|specialty| {
        let deal_rank = if *specialty == dealt_primary {
            0
        } else if *specialty == dealt_secondary {
            1
        } else {
            2
        };
        (
            std::cmp::Reverse(values[specialty.index()]),
            deal_rank,
            specialty.index(),
        )
    });
    (ranked[0], ranked[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_is_repeatable_and_difficulty_does_not_change_personality() {
        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Aggressive, 0xC0FFEE);
        let first = ResolvedProfile::resolve(config);
        let second = ResolvedProfile::resolve(config);
        assert_eq!(first, second);

        let lower = ResolvedProfile::resolve(BotConfig::scripted(
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
                let profile = ResolvedProfile::resolve(BotConfig::scripted(
                    BotDifficulty::Standard,
                    stance,
                    seed,
                ));
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
            let turtle = ResolvedProfile::resolve(BotConfig::scripted(
                BotDifficulty::Prime,
                BotStance::Turtle,
                seed,
            ));
            assert!(turtle.traits.fortification >= 65);
            assert!(turtle.traits.support >= 50);
            guile_ranges[0].0 = guile_ranges[0].0.min(turtle.traits.guile);
            guile_ranges[0].1 = guile_ranges[0].1.max(turtle.traits.guile);

            let balanced = ResolvedProfile::resolve(BotConfig::scripted(
                BotDifficulty::Prime,
                BotStance::Balanced,
                seed,
            ));
            assert_eq!(turtle.traits.guile, balanced.traits.guile);
            guile_ranges[1].0 = guile_ranges[1].0.min(balanced.traits.guile);
            guile_ranges[1].1 = guile_ranges[1].1.max(balanced.traits.guile);

            let aggressive = ResolvedProfile::resolve(BotConfig::scripted(
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
                let profile = ResolvedProfile::resolve(BotConfig::scripted(
                    BotDifficulty::Prime,
                    stance,
                    seed,
                ));
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
            let profile = ResolvedProfile::resolve(BotConfig::scripted(difficulty, stance, seed));
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

        let low = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            1,
        ));
        let high = ResolvedProfile::resolve(BotConfig::scripted(
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
}
