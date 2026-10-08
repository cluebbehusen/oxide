//! Deterministic personality resolution for the player-facing bot.

use chassis::rng::Pcg32;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
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
        let (primary, secondary) = ranked_specialties(values, dealt_primary, dealt_secondary);

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
    Specialty::from_index(rng.next_index(Specialty::ALL.len()))
}

fn choose_secondary(seed: u64, primary: Specialty) -> Specialty {
    let mut rng = Pcg32::new(seed, SECONDARY_STREAM);
    let offset = rng.next_index(Specialty::ALL.len() - 1) + 1;
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
    let span = 2 * u32::from(radius.unsigned_abs()) + 1;
    let jitter = i16::try_from(rng.next_below(span)).expect("the jitter span fits in i16") - radius;
    let specialty_bonus = if specialty == primary {
        PRIMARY_BONUS
    } else if specialty == secondary {
        SECONDARY_BONUS
    } else {
        0
    };
    let value = (i16::from(envelope.center) + jitter + specialty_bonus)
        .clamp(i16::from(envelope.min), i16::from(envelope.max));
    u8::try_from(value).expect("clamped into a u8 envelope")
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
    values: [u8; 6],
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
mod tests;
