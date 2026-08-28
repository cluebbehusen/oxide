//! Player-facing names for configured rules-based opponents.

use oxide_sim::scenario::{BotDifficulty, BotStance};

/// Where a bot description is being shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotLabelStyle {
    /// The descriptive label in the in-game selection panel.
    Controller,
    /// The full label in the post-match roster.
    Result,
    /// The shortened label used by dense cards and post-match rosters.
    Compact,
}

/// Formats the visible skill and stance of one opponent.
///
/// Personality seeds and resolved specialties are intentionally absent: they
/// are replay provenance, not information the ordinary UI should reveal.
pub fn bot_label(difficulty: BotDifficulty, stance: BotStance, style: BotLabelStyle) -> String {
    let difficulty = difficulty_name(difficulty);
    let stance = stance_name(stance);
    match style {
        BotLabelStyle::Controller => format!("{difficulty} / {stance} AI"),
        BotLabelStyle::Result => {
            format!(
                "{} / {} AI",
                difficulty.to_uppercase(),
                stance.to_uppercase()
            )
        }
        BotLabelStyle::Compact => format!(
            "{}/{}",
            compact_difficulty(difficulty),
            compact_stance(stance)
        ),
    }
}

/// Human-readable difficulty name used by controls that show one dial.
pub const fn difficulty_name(difficulty: BotDifficulty) -> &'static str {
    match difficulty {
        BotDifficulty::Scrapheap => "Scrapheap",
        BotDifficulty::Standard => "Standard",
        BotDifficulty::Veteran => "Veteran",
        BotDifficulty::Prime => "Prime",
    }
}

/// Human-readable stance name used by controls that show one dial.
pub const fn stance_name(stance: BotStance) -> &'static str {
    match stance {
        BotStance::Turtle => "Turtle",
        BotStance::Balanced => "Balanced",
        BotStance::Aggressive => "Aggressive",
    }
}

fn compact_difficulty(name: &'static str) -> &'static str {
    match name {
        "Scrapheap" => "SCRAP",
        "Standard" => "STD",
        "Veteran" => "VET",
        "Prime" => "PRIME",
        _ => name,
    }
}

fn compact_stance(name: &'static str) -> &'static str {
    match name {
        "Turtle" => "TURTLE",
        "Balanced" => "BAL",
        "Aggressive" => "AGGRO",
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_has_consistent_human_readable_labels() {
        let difficulties = [
            (BotDifficulty::Scrapheap, "Scrapheap", "SCRAP"),
            (BotDifficulty::Standard, "Standard", "STD"),
            (BotDifficulty::Veteran, "Veteran", "VET"),
            (BotDifficulty::Prime, "Prime", "PRIME"),
        ];
        let stances = [
            (BotStance::Turtle, "Turtle", "TURTLE"),
            (BotStance::Balanced, "Balanced", "BAL"),
            (BotStance::Aggressive, "Aggressive", "AGGRO"),
        ];

        for (difficulty, difficulty_label, compact_difficulty) in difficulties {
            assert_eq!(difficulty_name(difficulty), difficulty_label);
            for (stance, stance_label, compact_stance) in stances {
                assert_eq!(stance_name(stance), stance_label);
                let controller = bot_label(difficulty, stance, BotLabelStyle::Controller);
                let result = bot_label(difficulty, stance, BotLabelStyle::Result);
                let compact = bot_label(difficulty, stance, BotLabelStyle::Compact);

                assert_eq!(
                    controller,
                    format!("{difficulty_label} / {stance_label} AI")
                );
                assert_eq!(
                    result,
                    format!(
                        "{} / {} AI",
                        difficulty_label.to_uppercase(),
                        stance_label.to_uppercase()
                    )
                );
                assert_eq!(compact, format!("{compact_difficulty}/{compact_stance}"));
            }
        }
    }

    #[test]
    fn default_profile_is_no_longer_ambiguous_about_difficulty() {
        assert_eq!(
            bot_label(
                BotDifficulty::Standard,
                BotStance::Balanced,
                BotLabelStyle::Controller,
            ),
            "Standard / Balanced AI"
        );
        assert_eq!(
            bot_label(
                BotDifficulty::Standard,
                BotStance::Balanced,
                BotLabelStyle::Compact,
            ),
            "STD/BAL"
        );
    }
}
