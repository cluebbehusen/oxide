use super::*;

#[test]
fn run_all_bots_is_an_explicit_complete_match_mode() {
    let cli = Cli::try_parse_from(["oxide-driver", "run", "skirmish", "--all-bots"])
        .expect("all-bots run parses");
    let Cmd::Run { bots, all_bots, .. } = cli.cmd else {
        panic!("run parsed as another command")
    };
    assert!(!bots);
    assert!(all_bots);
    assert!(
        Cli::try_parse_from(["oxide-driver", "run", "skirmish", "--bots", "--all-bots",]).is_err(),
        "scenario-configured and all-seat modes are mutually exclusive"
    );
}

#[test]
fn bot_eval_parses_exact_profile_and_paired_seed_cell() {
    let cli = Cli::try_parse_from([
        "oxide-driver",
        "bot-eval",
        "skirmish",
        "scenarios/powder-keg.json",
        "--ticks",
        "50000",
        "--personality-seed-base",
        "900",
        "--difficulty",
        "prime",
        "--stance",
        "aggressive",
        "--paired",
        "--candidate",
        "build-a",
    ])
    .expect("bot evaluation parses");
    let Cmd::BotEval {
        scenarios,
        ticks,
        runs,
        personality_seed_base,
        difficulty,
        stance,
        opponent_difficulty,
        opponent_stance,
        same_personality_seed,
        paired,
        candidate,
        ..
    } = cli.cmd
    else {
        panic!("bot-eval parsed as another command")
    };
    assert_eq!(scenarios, ["skirmish", "scenarios/powder-keg.json"]);
    assert_eq!((ticks, runs), (50_000, 1));
    assert_eq!(personality_seed_base, Some(900));
    assert_eq!(difficulty, oxide_sim::scenario::BotDifficulty::Prime);
    assert_eq!(stance, oxide_sim::scenario::BotStance::Aggressive);
    assert_eq!(opponent_difficulty, None);
    assert_eq!(opponent_stance, None);
    assert!(!same_personality_seed);
    assert!(paired);
    assert_eq!(candidate.as_deref(), Some("build-a"));
}

#[test]
fn bot_eval_parses_cross_difficulty_shared_personality_comparison() {
    let cli = Cli::try_parse_from([
        "oxide-driver",
        "bot-eval",
        "skirmish",
        "--difficulty",
        "prime",
        "--opponent-difficulty",
        "standard",
        "--opponent-stance",
        "turtle",
        "--same-personality-seed",
        "--paired",
    ])
    .expect("cross-difficulty bot evaluation parses");
    let Cmd::BotEval {
        difficulty,
        stance,
        opponent_difficulty,
        opponent_stance,
        same_personality_seed,
        paired,
        ..
    } = cli.cmd
    else {
        panic!("bot-eval parsed as another command")
    };
    assert_eq!(difficulty, oxide_sim::scenario::BotDifficulty::Prime);
    assert_eq!(stance, oxide_sim::scenario::BotStance::Balanced);
    assert_eq!(
        opponent_difficulty,
        Some(oxide_sim::scenario::BotDifficulty::Standard)
    );
    assert_eq!(
        opponent_stance,
        Some(oxide_sim::scenario::BotStance::Turtle)
    );
    assert!(same_personality_seed);
    assert!(paired);
}

#[test]
fn bot_pressure_defaults_to_the_shipped_scenarios() {
    let cli = Cli::try_parse_from(["oxide-driver", "bot-pressure"]).expect("parses");
    let Cmd::BotPressure {
        dir,
        json,
        replay_dir,
    } = cli.cmd
    else {
        panic!("bot-pressure parsed as another command")
    };
    assert_eq!(dir, PathBuf::from("driver/evaluation/pressure"));
    assert_eq!((json, replay_dir), (false, None));
}

#[test]
fn bot_summary_requires_rows() {
    assert!(Cli::try_parse_from(["oxide-driver", "bot-summary"]).is_err());
    let cli = Cli::try_parse_from(["oxide-driver", "bot-summary", "a.jsonl", "--json"])
        .expect("bot-summary parses");
    let Cmd::BotSummary { rows, json } = cli.cmd else {
        panic!("bot-summary parsed as another command")
    };
    assert_eq!(rows, [PathBuf::from("a.jsonl")]);
    assert!(json);
}

#[test]
fn tick_scan_ranks_hundred_tick_windows_by_default() {
    let cli = Cli::try_parse_from(["oxide-driver", "tick-scan", "match.json"])
        .expect("a replay alone parses");
    let Cmd::TickScan {
        replay,
        window,
        top,
        json,
    } = cli.cmd
    else {
        panic!("tick-scan parsed as another command")
    };
    assert_eq!(
        (replay.as_str(), window, top, json),
        ("match.json", 100, 10, false)
    );
    assert!(
        Cli::try_parse_from(["oxide-driver", "tick-scan", "match.json", "--window", "0"]).is_err()
    );
}

#[test]
fn tick_profile_defaults_to_one_tick_and_requires_its_start() {
    let cli = Cli::try_parse_from(["oxide-driver", "tick-profile", "match.json", "--from", "40"])
        .expect("a replay and start parse");
    let Cmd::TickProfile {
        replay,
        from,
        ticks,
        seconds,
        focus,
        json,
        ..
    } = cli.cmd
    else {
        panic!("tick-profile parsed as another command")
    };
    assert_eq!(
        (replay.as_str(), from, ticks, seconds),
        ("match.json", 40, 1, 5)
    );
    assert_eq!((focus, json), (None, false));
    assert!(Cli::try_parse_from(["oxide-driver", "tick-profile", "match.json"]).is_err());
    assert!(
        Cli::try_parse_from([
            "oxide-driver",
            "tick-profile",
            "match.json",
            "--from",
            "40",
            "--ticks",
            "0",
        ])
        .is_err()
    );
}

#[test]
fn bot_cost_takes_a_named_workload_or_a_scenario_with_a_window() {
    let cli = Cli::try_parse_from(["oxide-driver", "bot-cost", "mature-armies", "--json"])
        .expect("named workload parses");
    let Cmd::BotCost {
        workload,
        scenario,
        ticks,
        json,
        ..
    } = cli.cmd
    else {
        panic!("bot-cost parsed as another command")
    };
    assert_eq!(
        workload,
        Some(oxide_driver::bot_cost::Workload::MatureArmies)
    );
    assert_eq!((scenario, ticks), (None, None));
    assert!(json);

    assert!(
        Cli::try_parse_from([
            "oxide-driver",
            "bot-cost",
            "--scenario",
            "a.json",
            "--ticks",
            "9"
        ])
        .is_ok()
    );
    for invalid in [
        &["oxide-driver", "bot-cost"][..],
        &["oxide-driver", "bot-cost", "--scenario", "a.json"],
        &[
            "oxide-driver",
            "bot-cost",
            "duel",
            "--scenario",
            "a.json",
            "--ticks",
            "9",
        ],
        &["oxide-driver", "bot-cost", "marathon"],
    ] {
        assert!(Cli::try_parse_from(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn profile_shell_parses_an_exact_replay_window() {
    let cli = Cli::try_parse_from([
        "oxide-driver",
        "profile-shell",
        "match.json",
        "--from",
        "4500",
        "--to",
        "5750",
        "--speed",
        "8",
    ])
    .expect("profile command parses");
    let Cmd::ProfileShell {
        replay,
        from,
        to,
        speed,
        dev,
        ..
    } = cli.cmd
    else {
        panic!("profile-shell parsed as another command")
    };
    assert_eq!(replay, PathBuf::from("match.json"));
    assert_eq!((from, to), (4500, 5750));
    assert_eq!(speed, 8.0);
    assert!(!dev);
}
