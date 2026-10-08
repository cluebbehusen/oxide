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
