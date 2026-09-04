//! Fair cognitive, execution, and macro-competence limits for player-facing bot
//! difficulties.
//!
//! Difficulty never changes the game state directly. Scrapheap alone uses a
//! reduced decision cadence; the three competent rungs share one cadence and
//! separate through reaction time, attention, memory, commitment timing,
//! strength judgment, tactical coordination, and the ordinary ground force
//! protected before voluntary opening capital.

use crate::scenario::BotDifficulty;
use chassis::Tick;

/// Shared strategic observation boundary. Every ordinary decision cadence
/// divides this interval, so each difficulty can admit and freeze a new
/// operation from the same world tick before rung-specific latency begins.
pub const STRATEGIC_ADMISSION_CADENCE: Tick = 24;

/// Whether this tick is available to every player-facing difficulty rung.
pub const fn strategic_admission_tick(tick: Tick) -> bool {
    tick.is_multiple_of(STRATEGIC_ADMISSION_CADENCE)
}

/// First shared strategic boundary strictly after `tick`.
pub const fn next_strategic_admission_tick(tick: Tick) -> Tick {
    let remainder = tick % STRATEGIC_ADMISSION_CADENCE;
    tick.saturating_add(STRATEGIC_ADMISSION_CADENCE - remainder)
}

/// First shared strategic boundary at or after `tick`.
pub const fn strategic_admission_at_or_after(tick: Tick) -> Tick {
    let remainder = tick % STRATEGIC_ADMISSION_CADENCE;
    if remainder == 0 {
        tick
    } else {
        tick.saturating_add(STRATEGIC_ADMISSION_CADENCE - remainder)
    }
}

/// Stable knobs derived from one player-facing difficulty rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DifficultyTuning {
    /// Ticks between ordinary decisions. Standard, Veteran, and Prime share
    /// one cadence so additional controller APM cannot become a disadvantage.
    pub cadence: u64,
    /// Ticks between observing a strategic fact and acting on it.
    pub reaction_delay: u64,
    /// Discretionary candidates considered during one think.
    pub attention_slots: usize,
    /// Ordinary ground strength established before voluntary capital spending,
    /// measured in full-health Sentinel equivalents.
    pub minimum_core_equivalents: u32,
    /// Age after which remembered tactical evidence is ignored.
    pub tactical_memory: u64,
    /// How long a previously observed hostile ground force remains available
    /// to strategic planning. Voluntary attack timing uses one shared shorter
    /// horizon so retaining the fact cannot punish a higher rung's initiative.
    pub opponent_force_memory: u64,
    /// Fixed conservative error applied to the bot's own ground strength.
    /// Easier rungs miss marginal opportunities; personality never changes this
    /// competence limit.
    pub strength_underestimate_percent: u8,
    /// Whether an engaged army coordinates every member onto one legal target
    /// instead of relying on ordinary per-unit acquisition.
    pub coordinated_focus: bool,
    /// Whether static defenses coordinate through the same explicit focus-fire
    /// command available to a human player.
    pub coordinated_defense_focus: bool,
    /// Extra deterministic delay before a prepared operation commits.
    pub commitment_hesitation: u64,
}

impl DifficultyTuning {
    /// Derives fair cognitive, execution, and macro-competence limits. Prime is
    /// the reference behavior; lower rungs lose promptness, precision, and
    /// protected opening-core depth, never legal actions or game rules.
    pub const fn for_level(level: BotDifficulty) -> Self {
        match level {
            BotDifficulty::Scrapheap => Self {
                cadence: 24,
                reaction_delay: 100,
                attention_slots: 2,
                minimum_core_equivalents: 4,
                tactical_memory: 240,
                opponent_force_memory: 1_800,
                strength_underestimate_percent: 6,
                coordinated_focus: false,
                coordinated_defense_focus: false,
                commitment_hesitation: 120,
            },
            BotDifficulty::Standard => Self {
                cadence: 12,
                reaction_delay: 40,
                attention_slots: 3,
                minimum_core_equivalents: 5,
                tactical_memory: 420,
                opponent_force_memory: 3_600,
                strength_underestimate_percent: 4,
                coordinated_focus: false,
                coordinated_defense_focus: false,
                commitment_hesitation: 48,
            },
            BotDifficulty::Veteran => Self {
                cadence: 12,
                reaction_delay: 16,
                attention_slots: 4,
                minimum_core_equivalents: 6,
                tactical_memory: 540,
                opponent_force_memory: 8_400,
                strength_underestimate_percent: 2,
                coordinated_focus: true,
                coordinated_defense_focus: false,
                commitment_hesitation: 16,
            },
            BotDifficulty::Prime => Self {
                cadence: 12,
                reaction_delay: 0,
                attention_slots: 4,
                minimum_core_equivalents: 8,
                tactical_memory: 600,
                opponent_force_memory: 12_000,
                strength_underestimate_percent: 0,
                coordinated_focus: true,
                coordinated_defense_focus: true,
                commitment_hesitation: 0,
            },
        }
    }

    /// Applies one stable, bounded underestimate to own strength.
    pub fn underestimate_own(self, value: u64) -> u64 {
        let scale = 100_u64.saturating_sub(u64::from(self.strength_underestimate_percent));
        (u128::from(value) * u128::from(scale) / 100) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_rungs_degrade_cognition_without_adding_capabilities() {
        let rungs = [
            DifficultyTuning::for_level(BotDifficulty::Scrapheap),
            DifficultyTuning::for_level(BotDifficulty::Standard),
            DifficultyTuning::for_level(BotDifficulty::Veteran),
            DifficultyTuning::for_level(BotDifficulty::Prime),
        ];
        for pair in rungs.windows(2) {
            let [lower, higher] = pair else {
                unreachable!()
            };
            assert!(lower.cadence >= higher.cadence);
            assert!(lower.reaction_delay >= higher.reaction_delay);
            assert!(lower.attention_slots <= higher.attention_slots);
            assert!(lower.minimum_core_equivalents <= higher.minimum_core_equivalents);
            assert!(lower.tactical_memory <= higher.tactical_memory);
            assert!(lower.opponent_force_memory <= higher.opponent_force_memory);
            assert!(lower.strength_underestimate_percent >= higher.strength_underestimate_percent);
            assert!(!lower.coordinated_focus || higher.coordinated_focus);
            assert!(!lower.coordinated_defense_focus || higher.coordinated_defense_focus);
            assert!(lower.commitment_hesitation >= higher.commitment_hesitation);
            assert_eq!(lower.cadence % higher.cadence, 0);
            for tick in 0..=lower.cadence * 4 {
                if tick.is_multiple_of(lower.cadence) {
                    assert!(tick.is_multiple_of(higher.cadence));
                }
            }
        }
        assert_eq!(
            STRATEGIC_ADMISSION_CADENCE,
            DifficultyTuning::for_level(BotDifficulty::Scrapheap).cadence
        );
        assert_eq!(
            BotDifficulty::ALL.map(|difficulty| DifficultyTuning::for_level(difficulty).cadence),
            [24, 12, 12, 12],
            "only Scrapheap should trade controller cadence for difficulty"
        );
        assert_eq!(
            BotDifficulty::ALL
                .map(|difficulty| DifficultyTuning::for_level(difficulty).opponent_force_memory),
            [1_800, 3_600, 8_400, 12_000],
            "higher rungs should retain observed enemy-force scale longer"
        );
        assert_eq!(
            BotDifficulty::ALL
                .map(|difficulty| DifficultyTuning::for_level(difficulty).coordinated_focus),
            [false, false, true, true],
            "Veteran and Prime should coordinate target focus"
        );
        assert_eq!(
            BotDifficulty::ALL
                .map(|difficulty| DifficultyTuning::for_level(difficulty).attention_slots),
            [2, 3, 4, 4],
            "Prime must not split off an optional raid beside simultaneous air and lift work"
        );
        assert_eq!(
            BotDifficulty::ALL.map(|difficulty| {
                DifficultyTuning::for_level(difficulty).coordinated_defense_focus
            }),
            [false, false, false, true],
            "Prime alone should direct overlapping static defenses"
        );
        for tuning in rungs {
            for tick in 0..=STRATEGIC_ADMISSION_CADENCE * 4 {
                if strategic_admission_tick(tick) {
                    assert!(tick.is_multiple_of(tuning.cadence));
                }
            }
        }
        assert_eq!(next_strategic_admission_tick(0), 24);
        assert_eq!(next_strategic_admission_tick(24), 48);
        assert_eq!(strategic_admission_at_or_after(24), 24);
        assert_eq!(strategic_admission_at_or_after(25), 48);
    }

    #[test]
    fn opening_core_floor_uses_exact_difficulty_rungs() {
        assert_eq!(
            BotDifficulty::ALL.map(|difficulty| {
                DifficultyTuning::for_level(difficulty).minimum_core_equivalents
            }),
            [4, 5, 6, 8]
        );
    }

    #[test]
    fn fixed_strength_underestimate_uses_exact_rung_scales() {
        let cases = [
            (BotDifficulty::Scrapheap, 6, 9_400),
            (BotDifficulty::Standard, 4, 9_600),
            (BotDifficulty::Veteran, 2, 9_800),
            (BotDifficulty::Prime, 0, 10_000),
        ];

        for (difficulty, underestimate, own_scale) in cases {
            let tuning = DifficultyTuning::for_level(difficulty);
            assert_eq!(tuning.strength_underestimate_percent, underestimate);
            assert_eq!(tuning.underestimate_own(10_000), own_scale);
        }
    }

    #[test]
    fn fixed_own_strength_estimates_are_nested_across_rounding_boundaries() {
        let rungs = [
            DifficultyTuning::for_level(BotDifficulty::Scrapheap),
            DifficultyTuning::for_level(BotDifficulty::Standard),
            DifficultyTuning::for_level(BotDifficulty::Veteran),
            DifficultyTuning::for_level(BotDifficulty::Prime),
        ];
        let boundary_values = [u64::from(u32::MAX), u64::MAX / 2, u64::MAX - 1, u64::MAX];
        for value in (0..=10_000).chain(boundary_values) {
            let own = rungs.map(|tuning| tuning.underestimate_own(value));
            for pair in own.windows(2) {
                assert!(pair[0] <= pair[1], "own estimate inverted at {value}");
            }
        }
    }

    #[test]
    fn conservative_estimates_do_not_overflow_at_u64_max() {
        let rungs = [
            DifficultyTuning::for_level(BotDifficulty::Scrapheap),
            DifficultyTuning::for_level(BotDifficulty::Standard),
            DifficultyTuning::for_level(BotDifficulty::Veteran),
            DifficultyTuning::for_level(BotDifficulty::Prime),
        ];
        assert_eq!(
            rungs[0].underestimate_own(u64::MAX),
            17_339_939_429_286_978_518
        );
        let standard = DifficultyTuning::for_level(BotDifficulty::Standard);
        assert_eq!(
            standard.underestimate_own(u64::MAX),
            17_708_874_310_761_169_550
        );
        assert_eq!(standard.underestimate_own(10_000), 9_600);
        assert_eq!(rungs[3].underestimate_own(u64::MAX), u64::MAX);
    }

    #[test]
    fn higher_commitment_opens_whenever_an_easier_rung_would() {
        let rungs = [
            DifficultyTuning::for_level(BotDifficulty::Scrapheap),
            DifficultyTuning::for_level(BotDifficulty::Standard),
            DifficultyTuning::for_level(BotDifficulty::Veteran),
            DifficultyTuning::for_level(BotDifficulty::Prime),
        ];
        let mut strengths: Vec<_> = (0_u64..=256)
            .chain((1_u64..=256).map(|value| value * 1_000))
            .chain([
                u64::from(u32::MAX),
                u64::MAX / 4,
                u64::MAX / 2,
                u64::MAX - 1,
                u64::MAX,
            ])
            .collect();
        strengths.sort_unstable();
        strengths.dedup();

        for &own in &strengths {
            for &enemy in &strengths {
                for margin_num in 4_u64..=8 {
                    let opens = rungs.map(|tuning| {
                        tuning.underestimate_own(own).saturating_mul(4)
                            >= enemy.saturating_mul(margin_num)
                    });
                    for pair in opens.windows(2) {
                        assert!(!pair[0] || pair[1], "{own} vs {enemy} at {margin_num}:4");
                    }
                }
            }
        }
    }
}
