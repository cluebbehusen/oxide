use super::*;
use oxide_sim::command::{Command, PlayerCommand, RejectReason};
use oxide_sim::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{Faction, StallReason, UnitKind};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REPLAY_ID: AtomicU64 = AtomicU64::new(0);

fn firing_squad() -> Scenario {
    let ground = ".".repeat(16);
    let mut anchored: Vec<char> = ground.chars().collect();
    anchored[1] = '1';
    anchored[11] = '2';
    let mut units = Vec::new();
    for x in [8, 9] {
        for y in [1, 2, 3, 4] {
            units.push(UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x,
                y,
            });
        }
    }
    Scenario {
        mode: ScenarioMode::Match,
        name: "firing-squad".into(),
        seed: 7,
        map: vec![
            ground.clone(),
            ground.clone(),
            anchored.into_iter().collect(),
            ground.clone(),
            ground.clone(),
            ground,
        ],
        players: vec![
            PlayerSpec {
                name: "attacker".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 100,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "victim".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 100,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn prime_config() -> BotConfig {
    BotConfig::new(BotDifficulty::Prime, BotStance::Balanced, 8_100)
}

fn prime_matchup() -> ProfileMatchup {
    ProfileMatchup {
        difficulty: BotDifficulty::Prime,
        stance: BotStance::Balanced,
        opponent_difficulty: Some(BotDifficulty::Standard),
        opponent_stance: Some(BotStance::Balanced),
        same_personality_seed: false,
    }
}

fn opponent_config() -> BotConfig {
    BotConfig::new(
        BotDifficulty::Standard,
        BotStance::Balanced,
        prime_config().personality_seed + 1,
    )
}

fn opponent_controller() -> EvaluationController {
    EvaluationController::configured(opponent_config())
}

fn one_evaluation_trace() -> EvaluationTraceRow {
    let plan = configured_matchup_plans(
        &Scenario::skirmish(),
        73,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Authored,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);
    let mut trace = None;
    evaluate_plan_artifact_traced_with(&plan, 1, None, "candidate-a", |row| {
        if row.seat == 0 {
            assert!(trace.replace(row.clone()).is_none());
        }
        Ok(())
    })
    .unwrap();
    trace.expect("Prime emits one trace at tick zero")
}

#[test]
fn profile_pair_swaps_only_controllers_on_one_transformed_scenario() {
    let source = Scenario::skirmish();
    let plans = configured_matchup_plans(
        &source,
        91,
        prime_matchup(),
        prime_config().personality_seed,
        true,
        EvaluationFactionCell::Cf,
        EvaluationGeometry::Rot180,
    )
    .unwrap();

    assert_eq!(plans.len(), 2);
    let (forward, swapped) = (&plans[0], &plans[1]);
    assert_eq!(forward.leg, EvaluationLeg::Forward);
    assert_eq!(swapped.leg, EvaluationLeg::Swapped);
    assert_eq!(forward.geometry, EvaluationGeometry::Rot180);
    assert_eq!(swapped.geometry, EvaluationGeometry::Rot180);
    assert_eq!(forward.faction_cell, EvaluationFactionCell::Cf);
    assert_eq!(swapped.faction_cell, EvaluationFactionCell::Cf);
    assert_eq!(forward.scenario, swapped.scenario);
    assert_eq!(forward.scenario.seed, 91);
    assert_eq!(
        forward
            .scenario
            .players
            .iter()
            .map(|player| player.faction)
            .collect::<Vec<_>>(),
        [Faction::Cupric, Faction::Ferrous]
    );
    assert!(
        forward
            .scenario
            .players
            .iter()
            .all(|player| !player.bot && player.bot_config.is_none())
    );
    assert_eq!(
        forward.controllers,
        [
            Some(EvaluationController::configured(prime_config())),
            Some(opponent_controller()),
        ]
    );
    assert_eq!(
        swapped.controllers,
        [
            Some(opponent_controller()),
            Some(EvaluationController::configured(prime_config())),
        ]
    );
    assert_eq!(source, Scenario::skirmish(), "planning mutated the source");
}

#[test]
fn controlled_faction_cells_retint_players_and_starting_rosters() {
    let source = Scenario::skirmish();
    let fc = configured_matchup_plans(
        &source,
        source.seed,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Fc,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);
    let cf = configured_matchup_plans(
        &source,
        source.seed,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Cf,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);

    assert_eq!(
        fc.scenario
            .players
            .iter()
            .map(|player| player.faction)
            .collect::<Vec<_>>(),
        [Faction::Ferrous, Faction::Cupric]
    );
    assert_eq!(fc.scenario.units, source.units);
    assert_eq!(
        cf.scenario
            .players
            .iter()
            .map(|player| player.faction)
            .collect::<Vec<_>>(),
        [Faction::Cupric, Faction::Ferrous]
    );
    for (actual, authored) in cf.scenario.units.iter().zip(&source.units) {
        let faction = cf.scenario.players[usize::from(actual.player)].faction;
        assert_eq!(actual.player, authored.player);
        assert_eq!((actual.x, actual.y), (authored.x, authored.y));
        assert_eq!(actual.kind, authored.kind.role().unit_for(faction));
    }
}

#[test]
fn controlled_geometry_records_and_applies_the_exact_half_turn() {
    let source = Scenario::skirmish();
    let authored = configured_matchup_plans(
        &source,
        31,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Cf,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);
    let rotated = configured_matchup_plans(
        &source,
        31,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Cf,
        EvaluationGeometry::Rot180,
    )
    .unwrap()
    .remove(0);

    assert_eq!(authored.geometry, EvaluationGeometry::Authored);
    assert_eq!(rotated.geometry, EvaluationGeometry::Rot180);
    assert_ne!(authored.scenario.units, rotated.scenario.units);
    assert_eq!(
        crate::rotation::rotate_180(&rotated.scenario).unwrap(),
        authored.scenario,
        "the recorded rot180 cell must be the exact involutive transform"
    );
}

#[test]
fn configured_evaluation_records_exact_controller_and_roster_provenance() {
    let plans = configured_matchup_plans(
        &Scenario::skirmish(),
        73,
        prime_matchup(),
        prime_config().personality_seed,
        true,
        EvaluationFactionCell::Cf,
        EvaluationGeometry::Authored,
    )
    .unwrap();

    for plan in plans {
        let (row, replay) = evaluate_plan_artifact(&plan, 1, "candidate-a").unwrap();
        let expected = match plan.leg {
            EvaluationLeg::Forward => [
                (Faction::Cupric, Some(prime_config())),
                (Faction::Ferrous, Some(opponent_config())),
            ],
            EvaluationLeg::Swapped => [
                (Faction::Cupric, Some(opponent_config())),
                (Faction::Ferrous, Some(prime_config())),
            ],
            EvaluationLeg::Single => panic!("paired plans cannot contain a single leg"),
        };

        assert_eq!(row.leg, plan.leg);
        assert_eq!(row.geometry, EvaluationGeometry::Authored);
        assert_eq!(row.faction_cell, EvaluationFactionCell::Cf);
        assert_eq!(
            row.scenario_fingerprint,
            scenario_fingerprint(&plan.scenario).unwrap()
        );
        assert_eq!(
            row.evaluation_fingerprint,
            evaluation_fingerprint(&plan).unwrap()
        );
        assert_eq!(replay.setup, plan.scenario);
        for (seat_index, (seat, (faction, config))) in row.seats.iter().zip(expected).enumerate() {
            assert_eq!(seat.seat, u8::try_from(seat_index).unwrap());
            assert_eq!(seat.faction, faction);
            assert_eq!(seat.config, config);
            assert_eq!(seat.profile, config.map(ResolvedProfile::resolve));
        }
        assert!(
            row.evidence.iter().all(|seat| seat.commands > 0),
            "both explicit evaluation controllers must actually run"
        );
        let description = replay.meta.description.as_deref().unwrap();
        assert!(description.contains(&format!("evaluation={}", row.evaluation_fingerprint)));
        assert!(description.contains("\"config\""));
        assert!(description.contains(&serde_json::to_string(&opponent_controller()).unwrap()));
    }
}

#[test]
fn traced_evaluation_is_deterministic_and_does_not_change_authoritative_evidence() {
    let plan = configured_matchup_plans(
        &Scenario::skirmish(),
        73,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Cf,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);

    let (ordinary_row, ordinary_replay) =
        evaluate_plan_artifact_with(&plan, 25, None, "candidate-a").unwrap();
    let mut traces = Vec::new();
    let (traced_row, traced_replay, trace_count) =
        evaluate_plan_artifact_traced_with(&plan, 25, None, "candidate-a", |row| {
            traces.push(row.clone());
            Ok(())
        })
        .unwrap();
    let mut repeated_traces = Vec::new();
    let (_, _, repeated_count) =
        evaluate_plan_artifact_traced_with(&plan, 25, None, "candidate-a", |row| {
            repeated_traces.push(row.clone());
            Ok(())
        })
        .unwrap();

    assert_eq!(traced_row, ordinary_row);
    assert_eq!(traced_row.final_hash, ordinary_row.final_hash);
    assert_eq!(traced_row.command_hash, ordinary_row.command_hash);
    assert_eq!(
        serde_json::to_vec(&traced_replay).unwrap(),
        serde_json::to_vec(&ordinary_replay).unwrap(),
        "trace capture must not enter replay metadata or commands"
    );
    assert!(!traces.is_empty(), "Prime should produce a decision trace");
    assert_eq!(trace_count, traces.len() as u64);
    assert_eq!(repeated_count, repeated_traces.len() as u64);
    assert_eq!(traces, repeated_traces);
    assert_eq!(
        serde_json::to_vec(&traces).unwrap(),
        serde_json::to_vec(&repeated_traces).unwrap()
    );
    for row in &traces {
        assert_eq!(row.version, EVALUATION_TRACE_ROW_VERSION);
        assert_eq!(row.candidate, "candidate-a");
        assert_eq!(
            row.evaluation_fingerprint,
            traced_row.evaluation_fingerprint
        );
        assert_eq!(row.leg, traced_row.leg);
        assert!(row.seat < 2);
        assert_eq!(row.seat, row.trace.player.0);
        assert_eq!(row.tick, row.trace.tick);
        assert!(row.tick < traced_row.duration_ticks);
    }
}

#[test]
fn nominal_axis_aliases_share_one_execution_identity_and_are_refused() {
    let source = Scenario::skirmish();
    let authored = configured_matchup_plans(
        &source,
        source.seed,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Authored,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);
    let explicit = configured_matchup_plans(
        &source,
        source.seed,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Fc,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);

    assert_eq!(authored.scenario, explicit.scenario);
    assert_eq!(authored.controllers, explicit.controllers);
    assert_ne!(
        evaluation_fingerprint(&authored).unwrap(),
        evaluation_fingerprint(&explicit).unwrap(),
        "the nominal provenance labels remain distinguishable"
    );
    assert_eq!(
        execution_fingerprint(&authored).unwrap(),
        execution_fingerprint(&explicit).unwrap(),
        "execution identity must ignore the aliasing faction label"
    );
    let error = ensure_unique_execution_plans([&authored, &explicit]).unwrap_err();
    assert!(
        error.to_string().contains("duplicate executable cells"),
        "alias refusal should identify the evidence problem: {error:#}"
    );
}

#[test]
fn command_hash_ignores_setup_seed_but_covers_ticks_and_commands() {
    let mut first = GameReplay::new(SIM_VERSION, Scenario::skirmish());
    let mut second_setup = Scenario::skirmish();
    second_setup.seed = second_setup.seed.wrapping_add(1);
    let mut second = GameReplay::new(SIM_VERSION, second_setup);
    let stop = PlayerCommand {
        player: PlayerId(0),
        command: Command::Stop { units: Vec::new() },
    };
    first.record(11, stop.clone());
    second.record(11, stop.clone());

    assert_eq!(
        command_hash(&first).unwrap(),
        command_hash(&second).unwrap()
    );
    second.record(12, stop);
    assert_ne!(
        command_hash(&first).unwrap(),
        command_hash(&second).unwrap()
    );
}

#[test]
fn controller_swaps_have_one_scenario_but_distinct_evidence_identities() {
    let plans = configured_matchup_plans(
        &Scenario::skirmish(),
        73,
        prime_matchup(),
        prime_config().personality_seed,
        true,
        EvaluationFactionCell::Fc,
        EvaluationGeometry::Rot180,
    )
    .unwrap();
    let (forward, swapped) = (&plans[0], &plans[1]);

    assert_eq!(forward.scenario, swapped.scenario);
    assert_eq!(
        scenario_fingerprint(&forward.scenario).unwrap(),
        scenario_fingerprint(&swapped.scenario).unwrap()
    );
    assert_ne!(
        evaluation_fingerprint(forward).unwrap(),
        evaluation_fingerprint(swapped).unwrap()
    );
    let forward_name =
        evaluation_replay_filename(0, 0, 73, 60_000, "candidate-a", forward).unwrap();
    let swapped_name =
        evaluation_replay_filename(0, 0, 73, 60_000, "candidate-a", swapped).unwrap();
    assert_ne!(forward_name, swapped_name);
    assert!(forward_name.contains("-forward-"));
    assert!(swapped_name.contains("-swapped-"));
}

#[test]
fn evaluation_plan_refuses_missing_or_excess_controller_slots() {
    let scenario = firing_squad();
    for controllers in [
        vec![Some(opponent_controller())],
        vec![
            Some(opponent_controller()),
            Some(opponent_controller()),
            Some(opponent_controller()),
        ],
    ] {
        let plan = EvaluationPlan {
            leg: EvaluationLeg::Single,
            scenario: scenario.clone(),
            controllers,
            geometry: EvaluationGeometry::Authored,
            faction_cell: EvaluationFactionCell::Authored,
        };

        let fingerprint_error = evaluation_fingerprint(&plan).unwrap_err();
        assert!(
            fingerprint_error
                .to_string()
                .contains("controllers for 2 seats"),
            "invalid provenance should fail clearly: {fingerprint_error:#}"
        );
        let evaluation_error = evaluate_plan_artifact(&plan, 1, "candidate-a").unwrap_err();
        assert!(
            evaluation_error
                .to_string()
                .contains("controllers for 2 seats"),
            "invalid execution should fail before building: {evaluation_error:#}"
        );
    }
}

#[test]
fn controlled_plans_refuse_non_duel_scenarios_before_transforming_them() {
    let mut too_few = firing_squad();
    too_few.players.pop();
    let mut too_many = firing_squad();
    too_many.players.push(too_many.players[0].clone());

    for scenario in [&too_few, &too_many] {
        let error = configured_matchup_plans(
            scenario,
            1,
            prime_matchup(),
            prime_config().personality_seed,
            true,
            EvaluationFactionCell::Fc,
            EvaluationGeometry::Authored,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("exactly two seats"),
            "non-duel refusal should name the controller shape: {error:#}"
        );
    }
}

#[test]
fn controlled_plans_refuse_two_seats_on_the_same_team() {
    let mut allied = firing_squad();
    allied.players[0].team = Some(4);
    allied.players[1].team = Some(4);

    let error = configured_matchup_plans(
        &allied,
        1,
        prime_matchup(),
        prime_config().personality_seed,
        true,
        EvaluationFactionCell::Fc,
        EvaluationGeometry::Authored,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("two opposing teams"),
        "allied-seat refusal should explain the competitive invariant: {error:#}"
    );
}

#[test]
fn replay_filename_refuses_a_seed_outside_its_plan() {
    let plan = configured_matchup_plans(
        &Scenario::skirmish(),
        73,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Fc,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);

    let error = evaluation_replay_filename(0, 0, 74, 60_000, "candidate-a", &plan).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not match evaluation scenario seed 73"),
        "filename provenance mismatch should fail clearly: {error:#}"
    );
}

#[test]
fn evaluation_stops_on_the_decision_instead_of_padding_frozen_ticks() {
    let row = evaluate(
        &firing_squad(),
        3_000,
        EvaluationLeg::Single,
        "test-build",
        None,
    )
    .unwrap();
    assert_eq!(row.termination, Termination::Decided);
    assert_eq!(row.result, Some(GameResult::Victory { team: 0 }));
    assert_eq!(row.winner_seats, [0]);
    assert!(row.duration_ticks < 3_000, "decision was not an early stop");
    assert_eq!(
        row.evidence.iter().map(|seat| seat.commands).sum::<u64>(),
        0
    );
}

#[test]
fn rows_record_teams_eliminations_detectors_income_and_provenance() {
    let row = evaluate(
        &firing_squad(),
        3_000,
        EvaluationLeg::Single,
        "test-build",
        None,
    )
    .unwrap();
    assert_eq!(
        row.seats.iter().map(|seat| seat.team).collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(row.evidence[0].eliminated_at, None);
    assert_eq!(row.evidence[1].eliminated_at, Some(row.duration_ticks - 1));
    assert!(
        row.evidence
            .iter()
            .all(|seat| seat.failures == SeatFailures::default()
                && seat.income.is_empty()
                && seat.ledger.is_none()
                && seat.attacks.is_none()),
        "seats without a controller are not watched"
    );
    assert_eq!(row.build, crate::build_identity());
    let json = serde_json::to_value(&row).unwrap();
    assert_eq!(json["seats"][1]["team"], 1);
    assert_eq!(
        json["evidence"][0]["eliminated_at"],
        serde_json::Value::Null
    );
    assert_eq!(
        json["evidence"][0]["failures"]["starved_production"]["incidents"],
        0
    );

    let plan = configured_matchup_plans(
        &Scenario::skirmish(),
        73,
        ProfileMatchup::uniform(BotDifficulty::Standard, BotStance::Balanced),
        8_100,
        false,
        EvaluationFactionCell::Authored,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);
    let (row, _) = evaluate_plan_artifact(&plan, INCOME_CHECKPOINTS[0], "test-build").unwrap();
    for seat in &row.evidence {
        let [sample] = seat.income.as_slice() else {
            panic!("one checkpoint is reached: {:?}", seat.income);
        };
        assert_eq!(sample.tick, INCOME_CHECKPOINTS[0]);
        assert!(sample.actual_per_minute > 0 && sample.saturation_per_minute > 0);
        let ledger = seat
            .ledger
            .as_ref()
            .expect("controlled seats keep a ledger");
        assert_eq!(
            ledger.worth.len() as u64,
            INCOME_CHECKPOINTS[0] / crate::ledger::WORTH_PERIOD
        );
        assert!(ledger.units["harvester"].harvested > 0);
    }

    let mut plan = plan;
    for controller in plan.controllers.iter_mut().flatten() {
        *controller = EvaluationController::configured(BotConfig::new(
            BotDifficulty::Standard,
            BotStance::Balanced,
            73,
        ));
    }
    let (row, _) = evaluate_plan_artifact(&plan, 600, "test-build").unwrap();
    assert!(
        row.evidence.iter().all(|seat| seat.attacks.is_some()),
        "oxide-opponent seats are calibrated"
    );
}

#[test]
fn identical_evaluations_produce_identical_rows() {
    let scenario = firing_squad();
    let a = evaluate(&scenario, 3_000, EvaluationLeg::Single, "test-build", None).unwrap();
    let b = evaluate(&scenario, 3_000, EvaluationLeg::Single, "test-build", None).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.candidate, "test-build");
    assert_eq!(a.tick_limit, 3_000);
    assert_eq!(
        a.scenario_fingerprint,
        scenario_fingerprint(&scenario).unwrap()
    );
}

#[test]
fn saved_replay_preserves_the_exact_compared_configs() {
    let matchup = ProfileMatchup {
        difficulty: BotDifficulty::Prime,
        stance: BotStance::Aggressive,
        opponent_difficulty: Some(BotDifficulty::Veteran),
        opponent_stance: Some(BotStance::Turtle),
        same_personality_seed: true,
    };
    let scenario = configured_matchup_legs(&firing_squad(), 91, matchup, 400, false)
        .unwrap()
        .remove(0)
        .1;
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let replay_path = std::env::temp_dir().join(format!(
        "oxide-bot-eval-{}-{replay_id}.json",
        std::process::id()
    ));

    let row = evaluate(
        &scenario,
        1,
        EvaluationLeg::Single,
        "candidate-a",
        Some(&replay_path),
    )
    .unwrap();
    let replay = oxide_kit::load_replay(&replay_path).unwrap();
    std::fs::remove_file(&replay_path).unwrap();

    assert_eq!(row.replay.as_deref(), replay_path.to_str());
    assert_eq!(replay.setup.seed, 91);
    assert_eq!(replay.setup.players, scenario.players);
    let description = replay.meta.description.as_deref().unwrap();
    assert!(description.contains("candidate=candidate-a"));
    assert!(description.contains(&format!("scenario={}", row.scenario_fingerprint)));
    assert!(description.contains("tick_limit=1"));
}

#[test]
fn replay_filenames_distinguish_matchups_with_the_same_numeric_seed_cell() {
    let source = firing_squad();
    let prime_scrapheap = configured_matchup_legs(
        &source,
        13,
        ProfileMatchup {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            opponent_difficulty: Some(BotDifficulty::Scrapheap),
            opponent_stance: None,
            same_personality_seed: true,
        },
        40,
        true,
    )
    .unwrap();
    let veteran_standard = configured_matchup_legs(
        &source,
        13,
        ProfileMatchup {
            difficulty: BotDifficulty::Veteran,
            stance: BotStance::Balanced,
            opponent_difficulty: Some(BotDifficulty::Standard),
            opponent_stance: None,
            same_personality_seed: true,
        },
        40,
        true,
    )
    .unwrap();

    for leg in 0..2 {
        let (first_leg, first) = &prime_scrapheap[leg];
        let (second_leg, second) = &veteran_standard[leg];
        let first_name =
            replay_filename(0, 0, 13, 48_000, *first_leg, "candidate-a", first).unwrap();
        let second_name =
            replay_filename(0, 0, 13, 48_000, *second_leg, "candidate-a", second).unwrap();
        assert_ne!(first_name, second_name);
    }
}

#[test]
fn replay_filenames_distinguish_extended_tick_limits() {
    let scenario = configured_legs(
        &firing_squad(),
        13,
        BotDifficulty::Prime,
        BotStance::Balanced,
        40,
        false,
    )
    .unwrap()
    .remove(0)
    .1;
    let short =
        replay_filename(0, 0, 13, 1, EvaluationLeg::Single, "candidate-a", &scenario).unwrap();
    let extended =
        replay_filename(0, 0, 13, 2, EvaluationLeg::Single, "candidate-a", &scenario).unwrap();
    assert_ne!(short, extended);
}

#[test]
fn replay_filenames_distinguish_candidate_builds() {
    let scenario = firing_squad();
    let first = replay_filename(
        0,
        0,
        scenario.seed,
        1,
        EvaluationLeg::Single,
        "candidate-a",
        &scenario,
    )
    .unwrap();
    let second = replay_filename(
        0,
        0,
        scenario.seed,
        1,
        EvaluationLeg::Single,
        "candidate-b",
        &scenario,
    )
    .unwrap();
    assert_ne!(first, second);
}

#[test]
fn replay_evidence_never_overwrites_an_existing_file() {
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let replay_path = std::env::temp_dir().join(format!(
        "oxide-bot-eval-existing-{}-{replay_id}.json",
        std::process::id()
    ));
    std::fs::write(&replay_path, b"earlier evidence").unwrap();

    let error = evaluate(
        &firing_squad(),
        1,
        EvaluationLeg::Single,
        "candidate-a",
        Some(&replay_path),
    )
    .unwrap_err();
    assert!(error.to_string().contains("without overwriting"));
    assert_eq!(std::fs::read(&replay_path).unwrap(), b"earlier evidence");
    std::fs::remove_file(replay_path).unwrap();
}

#[test]
fn a_publication_collision_rolls_back_the_whole_staged_set() {
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-eval-batch-{}-{replay_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let first = dir.join("first.json");
    let second = dir.join("second.json");
    let (_, replay) =
        evaluate_artifact(&firing_squad(), 1, EvaluationLeg::Single, "candidate-a").unwrap();
    let mut batch = EvidenceBatch::default();
    batch.stage_replay(&replay, &first).unwrap();
    batch.stage_replay(&replay, &second).unwrap();
    std::fs::write(&second, b"racing evidence").unwrap();

    let error = batch.publish().unwrap_err();
    assert!(error.to_string().contains("without overwriting"));
    assert!(!first.exists(), "the earlier publication was rolled back");
    assert_eq!(std::fs::read(&second).unwrap(), b"racing evidence");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_trace_publication_collision_rolls_back_the_compact_index() {
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-trace-batch-{}-{replay_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let index_path = dir.join("rows.jsonl");
    let trace_path = dir.join("trace.jsonl");
    let plan = configured_matchup_plans(
        &Scenario::skirmish(),
        73,
        prime_matchup(),
        prime_config().personality_seed,
        false,
        EvaluationFactionCell::Authored,
        EvaluationGeometry::Authored,
    )
    .unwrap()
    .remove(0);
    let mut traces = Vec::new();
    let (row, _, trace_count) =
        evaluate_plan_artifact_traced_with(&plan, 1, None, "candidate-a", |row| {
            traces.push(row.clone());
            Ok(())
        })
        .unwrap();
    assert!(!traces.is_empty());
    assert_eq!(trace_count, traces.len() as u64);

    let mut batch = EvidenceBatch::default();
    batch
        .stage_jsonl(std::slice::from_ref(&row), &index_path)
        .unwrap();
    let mut writer = batch.stage_trace_jsonl(&trace_path).unwrap();
    for trace in &traces {
        writer.write_row(trace).unwrap();
    }
    assert_eq!(writer.finish().unwrap(), traces.len() as u64);
    std::fs::write(&trace_path, b"racing trace evidence").unwrap();

    let error = batch.publish().unwrap_err();
    assert!(error.to_string().contains("without overwriting"));
    assert!(!index_path.exists(), "the earlier index was rolled back");
    assert_eq!(
        std::fs::read(&trace_path).unwrap(),
        b"racing trace evidence"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_unfinished_trace_stream_cannot_be_published() {
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-trace-unfinished-{}-{replay_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let trace_path = dir.join("trace.jsonl");
    let mut batch = EvidenceBatch::default();
    let mut writer = batch.stage_trace_jsonl(&trace_path).unwrap();
    writer.write_row(&one_evaluation_trace()).unwrap();

    let error = batch.publish().unwrap_err();
    assert!(error.to_string().contains("must be finished"));
    assert!(!trace_path.exists(), "unfinished output was not published");
    drop(writer);
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "closing the unfinished writer removes its private staging file"
    );
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn an_unfinished_trace_writer_cleans_up_after_its_batch_is_dropped() {
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-trace-drop-order-{}-{replay_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let trace_path = dir.join("trace.jsonl");
    let writer = {
        let mut batch = EvidenceBatch::default();
        let writer = batch.stage_trace_jsonl(&trace_path).unwrap();
        drop(batch);
        writer
    };

    drop(writer);
    assert!(!trace_path.exists(), "a dropped stream is never published");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "the writer removes a staging file that outlives its batch"
    );
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn duplicate_evidence_destinations_are_refused_without_leaving_staging_files() {
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-eval-duplicate-{}-{replay_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let destination = dir.join("evidence.json");
    let row = evaluate(
        &firing_squad(),
        1,
        EvaluationLeg::Single,
        "candidate-a",
        None,
    )
    .unwrap();

    let preflight_error =
        preflight_destinations(&[destination.clone(), destination.clone()]).unwrap_err();
    assert!(preflight_error.to_string().contains("must be unique"));

    let mut batch = EvidenceBatch::default();
    batch
        .stage_jsonl(std::slice::from_ref(&row), &destination)
        .unwrap();
    let staging_error = batch
        .stage_jsonl(std::slice::from_ref(&row), &destination)
        .unwrap_err();
    assert!(staging_error.to_string().contains("duplicate"));
    drop(batch);

    assert!(!destination.exists(), "a staged payload is not published");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "dropping the refused batch removes its private staging file"
    );
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn an_invalid_evidence_parent_fails_cleanly_without_replacing_the_blocker() {
    let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-eval-parent-{}-{replay_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let blocker = dir.join("not-a-directory");
    std::fs::write(&blocker, b"keep me").unwrap();
    let destination = blocker.join("evidence.jsonl");
    let row = evaluate(
        &firing_squad(),
        1,
        EvaluationLeg::Single,
        "candidate-a",
        None,
    )
    .unwrap();

    let mut batch = EvidenceBatch::default();
    let error = batch
        .stage_jsonl(std::slice::from_ref(&row), &destination)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("creating bot evaluation evidence directory"),
        "bad output parents should retain actionable context: {error:#}"
    );
    assert_eq!(std::fs::read(&blocker).unwrap(), b"keep me");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn evidence_uses_stable_wire_names_and_is_attributed_to_the_emitting_seat() {
    let mut evidence = vec![SeatEvidence::new(0), SeatEvidence::new(1)];
    record_evidence_event(
        &mut evidence,
        &Event::CommandRejected {
            player: PlayerId(1),
            reason: RejectReason::BadSite,
        },
    );
    record_evidence_event(
        &mut evidence,
        &Event::OrderStalled {
            unit: oxide_sim::UnitId(4),
            player: PlayerId(0),
            pos: chassis::fx::Vec2Fx::ZERO,
            reason: StallReason::NoRoute,
        },
    );
    record_evidence_event(
        &mut evidence,
        &Event::CommandRejected {
            player: PlayerId(9),
            reason: RejectReason::BadSite,
        },
    );
    record_evidence_event(
        &mut evidence,
        &Event::GameOver {
            result: GameResult::Draw,
        },
    );

    assert_eq!(evidence[0].rejections, 0);
    assert_eq!(evidence[0].stalls, 1);
    assert_eq!(evidence[0].stall_reasons["no_route"], 1);
    assert_eq!(evidence[0].stall_units[&4]["no_route"], 1);
    assert_eq!(evidence[1].rejections, 1);
    assert_eq!(evidence[1].stalls, 0);
    assert_eq!(evidence[1].rejection_reasons["bad_site"], 1);
}

#[test]
fn candidate_provenance_refuses_empty_padded_control_and_overlong_values() {
    let overlong = "x".repeat(MAX_CANDIDATE_LEN + 1);
    let cases = [
        ("", "must not be empty"),
        (" padded", "surrounding whitespace"),
        ("padded ", "surrounding whitespace"),
        ("line\nbreak", "control characters"),
        (overlong.as_str(), "at most"),
    ];

    for (candidate, expected) in cases {
        let error =
            evaluate_artifact(&firing_squad(), 1, EvaluationLeg::Single, candidate).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "{candidate:?} produced an unclear refusal: {error:#}"
        );
    }
}

#[test]
fn paired_legs_exchange_profiles_without_changing_the_match_seed() {
    let legs = configured_legs(
        &firing_squad(),
        91,
        BotDifficulty::Prime,
        BotStance::Aggressive,
        400,
        true,
    )
    .unwrap();
    assert_eq!(legs.len(), 2);
    let (forward_leg, forward) = &legs[0];
    let (swapped_leg, swapped) = &legs[1];
    assert_eq!(*forward_leg, EvaluationLeg::Forward);
    assert_eq!(*swapped_leg, EvaluationLeg::Swapped);
    assert_eq!((forward.seed, swapped.seed), (91, 91));
    assert_eq!(forward.players[0].bot_config.unwrap().personality_seed, 400);
    assert_eq!(forward.players[1].bot_config.unwrap().personality_seed, 401);
    assert_eq!(swapped.players[0].bot_config, forward.players[1].bot_config);
    assert_eq!(swapped.players[1].bot_config, forward.players[0].bot_config);
    assert!(
        forward
            .players
            .iter()
            .chain(&swapped.players)
            .all(|player| player.bot)
    );
}

#[test]
fn cross_difficulty_legs_share_identity_and_swap_complete_configs() {
    let matchup = ProfileMatchup {
        difficulty: BotDifficulty::Prime,
        stance: BotStance::Balanced,
        opponent_difficulty: Some(BotDifficulty::Scrapheap),
        opponent_stance: None,
        same_personality_seed: true,
    };
    let source = firing_squad();
    let legs = configured_matchup_legs(&source, 91, matchup, 400, true).unwrap();
    let forward = &legs[0].1;
    let swapped = &legs[1].1;
    let prime = forward.players[0].bot_config.unwrap();
    let scrapheap = forward.players[1].bot_config.unwrap();

    assert_eq!(prime.difficulty, BotDifficulty::Prime);
    assert_eq!(scrapheap.difficulty, BotDifficulty::Scrapheap);
    assert_eq!(
        (prime.stance, scrapheap.stance),
        (BotStance::Balanced, BotStance::Balanced)
    );
    assert_eq!(
        (prime.personality_seed, scrapheap.personality_seed),
        (400, 400)
    );
    assert_eq!(
        ResolvedProfile::resolve(prime).traits,
        ResolvedProfile::resolve(scrapheap).traits
    );
    assert_eq!(swapped.players[0].bot_config, Some(scrapheap));
    assert_eq!(swapped.players[1].bot_config, Some(prime));

    for seat in 0..source.players.len() {
        assert_eq!(forward.players[seat].name, source.players[seat].name);
        assert_eq!(forward.players[seat].faction, source.players[seat].faction);
        assert_eq!(forward.players[seat].team, source.players[seat].team);
        assert_eq!(forward.players[seat].scrap, source.players[seat].scrap);
        assert_eq!(swapped.players[seat].name, source.players[seat].name);
        assert_eq!(swapped.players[seat].faction, source.players[seat].faction);
        assert_eq!(swapped.players[seat].team, source.players[seat].team);
        assert_eq!(swapped.players[seat].scrap, source.players[seat].scrap);
    }
    assert_eq!(forward.map, source.map);
    assert_eq!(swapped.map, source.map);
    assert_eq!(forward.units, source.units);
    assert_eq!(swapped.units, source.units);
}

#[test]
fn personality_seed_runs_are_dense_and_overflow_checked() {
    let distinct = ProfileMatchup::uniform(BotDifficulty::Standard, BotStance::Balanced);
    assert_eq!(
        distinct.personality_seed_base_for_run(100, 0, 2).unwrap(),
        100
    );
    assert_eq!(
        distinct.personality_seed_base_for_run(100, 1, 2).unwrap(),
        102
    );
    assert_eq!(
        distinct.personality_seed_base_for_run(100, 2, 2).unwrap(),
        104
    );

    let shared = ProfileMatchup {
        same_personality_seed: true,
        ..distinct
    };
    assert_eq!(
        shared.personality_seed_base_for_run(100, 0, 2).unwrap(),
        100
    );
    assert_eq!(
        shared.personality_seed_base_for_run(100, 1, 2).unwrap(),
        101
    );
    assert_eq!(
        shared.personality_seed_base_for_run(100, 2, 2).unwrap(),
        102
    );
    assert!(
        distinct
            .personality_seed_base_for_run(u64::MAX, 1, 2)
            .unwrap_err()
            .to_string()
            .contains("overflows")
    );
    assert!(
        shared
            .personality_seed_base_for_run(u64::MAX, 1, 2)
            .unwrap_err()
            .to_string()
            .contains("overflows")
    );
}

#[test]
fn comparison_only_options_refuse_ambiguous_multiseat_scenarios() {
    let mut scenario = firing_squad();
    scenario.players.push(scenario.players[0].clone());
    let uniform = ProfileMatchup::uniform(BotDifficulty::Standard, BotStance::Balanced);
    let cases = [
        (
            "opponent difficulty",
            ProfileMatchup {
                opponent_difficulty: Some(BotDifficulty::Prime),
                ..uniform
            },
            false,
        ),
        (
            "opponent stance",
            ProfileMatchup {
                opponent_stance: Some(BotStance::Aggressive),
                ..uniform
            },
            false,
        ),
        (
            "shared personality",
            ProfileMatchup {
                same_personality_seed: true,
                ..uniform
            },
            false,
        ),
        ("paired", uniform, true),
    ];
    for (label, matchup, paired) in cases {
        let error = configured_matchup_legs(&scenario, 1, matchup, 2, paired).unwrap_err();
        assert!(
            error.to_string().contains("exactly two seats"),
            "{label} produced an unclear error: {error:#}"
        );
    }
}

#[test]
fn paired_legs_refuse_a_shape_that_cannot_be_exchanged() {
    let mut scenario = firing_squad();
    scenario.players.push(scenario.players[0].clone());
    let error = configured_legs(
        &scenario,
        1,
        BotDifficulty::Standard,
        BotStance::Balanced,
        2,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("exactly two seats"));
}

#[test]
fn a_unit_stalling_the_same_way_repeatedly_is_a_counted_loop() {
    let mut evidence = vec![SeatEvidence::new(0), SeatEvidence::new(1)];
    let stall = Event::OrderStalled {
        unit: oxide_sim::UnitId(4),
        player: PlayerId(1),
        pos: chassis::fx::Vec2Fx::new(chassis::fx::Fx::from_num(3), chassis::fx::Fx::from_num(3)),
        reason: StallReason::NoRoute,
    };
    let mut last = None;
    for _ in 0..DEFAULT_STALL_LOOP_LIMIT {
        last = record_evidence_event(&mut evidence, &stall);
    }
    let sample = last.expect("a stall is a counted sample");
    assert_eq!(
        (sample.seat, sample.unit, sample.reason.as_str()),
        (1, 4, "no_route")
    );
    assert_eq!(sample.count, DEFAULT_STALL_LOOP_LIMIT);
    assert_eq!(evidence[1].stalls, DEFAULT_STALL_LOOP_LIMIT);
    let other = Event::OrderStalled {
        unit: oxide_sim::UnitId(9),
        player: PlayerId(1),
        pos: chassis::fx::Vec2Fx::new(chassis::fx::Fx::from_num(3), chassis::fx::Fx::from_num(3)),
        reason: StallReason::NoRoute,
    };
    let fresh = record_evidence_event(&mut evidence, &other).expect("counted");
    assert_eq!(fresh.count, 1, "the loop count is per unit, not per seat");
    assert!(
        record_evidence_event(
            &mut evidence,
            &Event::CommandRejected {
                player: PlayerId(0),
                reason: oxide_sim::command::RejectReason::NoValidUnits,
            }
        )
        .is_none(),
        "a rejection is not a stall sample"
    );
    assert_eq!(
        serde_json::to_value(Termination::StallLoop).unwrap(),
        serde_json::json!("stall_loop")
    );
}

#[test]
fn a_stall_loop_limit_of_zero_is_refused_and_none_disables_the_stop() {
    let plan = EvaluationPlan::from_scenario(firing_squad(), EvaluationLeg::Single);
    let error = evaluate_plan_artifact_with(&plan, 100, Some(0), "test-build").unwrap_err();
    assert!(error.to_string().contains("stall-loop limit"), "{error:#}");
    let (row, _) = evaluate_plan_artifact_with(&plan, 100, None, "test-build").unwrap();
    assert_eq!(row.stall_loop_limit, None);
    assert_eq!(row.stall_loop, None);
    let (row, _) = evaluate_plan_artifact(&plan, 100, "test-build").unwrap();
    assert_eq!(row.stall_loop_limit, Some(DEFAULT_STALL_LOOP_LIMIT));
    assert!(
        !serde_json::to_string(&row)
            .unwrap()
            .contains("\"stall_loop\":"),
        "an absent loop stays off the wire"
    );
}
