//! End-to-end contract coverage for `oxide-driver bot-eval`.

use serde_json::Value;
use std::collections::BTreeSet;
use std::process::Command;

#[test]
fn overseer_matrix_keeps_controller_faction_and_geometry_provenance_separate() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--against-overseer",
            "--overseer-policy-seed",
            "29",
            "--paired",
            "--scenario-seeds",
            "13",
            "--personality-seeds",
            "40",
            "--difficulty",
            "prime",
            "--stance",
            "aggressive",
            "--faction-cells",
            "fc,cf",
            "--geometries",
            "authored,rot180",
        ])
        .output()
        .expect("run controlled Overseer evaluation matrix");
    assert!(
        output.status.success(),
        "bot-eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("JSONL is UTF-8");
    let rows: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
        .collect();
    assert_eq!(rows.len(), 8, "two factions x two geometries x two legs");

    let mut cells = BTreeSet::new();
    let (paired_cells, remainder) = rows.as_chunks::<2>();
    assert!(remainder.is_empty());
    for legs in paired_cells {
        assert_eq!(legs[0]["leg"], "forward");
        assert_eq!(legs[1]["leg"], "swapped");
        assert_eq!(legs[0]["scenario_seed"], 13);
        assert_eq!(legs[1]["scenario_seed"], 13);
        assert_eq!(legs[0]["geometry"], legs[1]["geometry"]);
        assert_eq!(legs[0]["faction_cell"], legs[1]["faction_cell"]);
        assert_eq!(
            legs[0]["scenario_fingerprint"], legs[1]["scenario_fingerprint"],
            "controller exchange must not alter the transformed scenario"
        );
        assert_ne!(
            legs[0]["evaluation_fingerprint"], legs[1]["evaluation_fingerprint"],
            "controller seat is part of evaluation provenance"
        );

        let faction_cell = legs[0]["faction_cell"].as_str().unwrap();
        let geometry = legs[0]["geometry"].as_str().unwrap();
        cells.insert((faction_cell.to_string(), geometry.to_string()));
        let expected_factions = match faction_cell {
            "fc" => ["ferrous", "cupric"],
            "cf" => ["cupric", "ferrous"],
            other => panic!("unexpected faction cell {other}"),
        };

        for row in legs {
            assert_eq!(row["seats"][0]["faction"], expected_factions[0]);
            assert_eq!(row["seats"][1]["faction"], expected_factions[1]);
        }

        for (row, scripted_seat) in [(&legs[0], 0), (&legs[1], 1)] {
            let overseer_seat = 1 - scripted_seat;
            assert_eq!(row["seats"][scripted_seat]["controller"], "scripted");
            assert_eq!(row["seats"][overseer_seat]["controller"], "overseer");
            assert_eq!(
                row["seats"][scripted_seat]["config"]["personality_seed"],
                40
            );
            assert_eq!(row["seats"][scripted_seat]["config"]["difficulty"], "prime");
            assert_eq!(
                row["seats"][scripted_seat]["config"]["stance"],
                "aggressive"
            );
            assert!(row["seats"][scripted_seat]["profile"].is_object());
            assert!(row["seats"][overseer_seat]["config"].is_null());
            assert!(row["seats"][overseer_seat]["profile"].is_null());
            assert_eq!(row["seats"][overseer_seat]["overseer_policy_seed"], 29);
        }
    }

    assert_eq!(
        cells,
        BTreeSet::from([
            ("cf".to_string(), "authored".to_string()),
            ("cf".to_string(), "rot180".to_string()),
            ("fc".to_string(), "authored".to_string()),
            ("fc".to_string(), "rot180".to_string()),
        ])
    );
}

#[test]
fn overseer_seed_lists_form_a_cartesian_product() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--against-overseer",
            "--scenario-seeds",
            "13,17",
            "--personality-seeds",
            "40,42,44",
        ])
        .output()
        .expect("run explicit Overseer seed matrix");
    assert!(
        output.status.success(),
        "bot-eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("JSONL is UTF-8");
    let rows: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
        .collect();
    assert_eq!(rows.len(), 6, "two scenario seeds x three personalities");

    let observed: BTreeSet<(u64, u64)> = rows
        .iter()
        .map(|row| {
            assert_eq!(row["leg"], "single");
            assert_eq!(row["geometry"], "authored");
            assert_eq!(row["faction_cell"], "authored");
            assert_eq!(row["seats"][0]["controller"], "scripted");
            assert_eq!(row["seats"][1]["controller"], "overseer");
            (
                row["scenario_seed"].as_u64().unwrap(),
                row["seats"][0]["config"]["personality_seed"]
                    .as_u64()
                    .unwrap(),
            )
        })
        .collect();
    assert_eq!(
        observed,
        BTreeSet::from([(13, 40), (13, 42), (13, 44), (17, 40), (17, 42), (17, 44),])
    );
}

#[test]
fn overseer_mode_refuses_player_facing_opponent_knobs() {
    for (args, opponent_option) in [
        (
            &["--against-overseer", "--opponent-difficulty", "veteran"][..],
            "--opponent-difficulty",
        ),
        (
            &["--against-overseer", "--opponent-stance", "turtle"][..],
            "--opponent-stance",
        ),
        (
            &["--against-overseer", "--same-personality-seed"][..],
            "--same-personality-seed",
        ),
    ] {
        assert_skirmish_bot_eval_refuses(
            args,
            &["--against-overseer", opponent_option, "cannot be used with"],
        );
    }
}

#[test]
fn overseer_axes_require_the_mode_and_exact_seed_lists_refuse_bases() {
    for (args, overseer_option) in [
        (&["--scenario-seeds", "13"][..], "--scenario-seeds"),
        (&["--personality-seeds", "40"][..], "--personality-seeds"),
        (&["--faction-cells", "fc"][..], "--faction-cells"),
        (&["--geometries", "rot180"][..], "--geometries"),
        (
            &["--overseer-policy-seed", "29"][..],
            "--overseer-policy-seed",
        ),
    ] {
        assert_skirmish_bot_eval_refuses(
            args,
            &[overseer_option, "--against-overseer", "required"],
        );
    }

    for (args, exact_option, base_option) in [
        (
            &[
                "--against-overseer",
                "--scenario-seeds",
                "13",
                "--scenario-seed-base",
                "17",
            ][..],
            "--scenario-seeds",
            "--scenario-seed-base",
        ),
        (
            &[
                "--against-overseer",
                "--personality-seeds",
                "40",
                "--personality-seed-base",
                "44",
            ][..],
            "--personality-seeds",
            "--personality-seed-base",
        ),
    ] {
        assert_skirmish_bot_eval_refuses(args, &[exact_option, base_option, "cannot be used with"]);
    }

    assert_skirmish_bot_eval_refuses(
        &["--against-overseer", "--runs", "2"],
        &["--against-overseer", "--runs", "cannot be used with"],
    );
}

#[test]
fn overseer_axes_refuse_duplicate_cells() {
    for (args, option) in [
        (
            &["--against-overseer", "--scenario-seeds", "13,13"][..],
            "--scenario-seeds",
        ),
        (
            &["--against-overseer", "--personality-seeds", "40,40"][..],
            "--personality-seeds",
        ),
        (
            &["--against-overseer", "--faction-cells", "fc,fc"][..],
            "--faction-cells",
        ),
        (
            &["--against-overseer", "--geometries", "authored,authored"][..],
            "--geometries",
        ),
    ] {
        assert_skirmish_bot_eval_refuses(args, &[option, "contains a duplicate value"]);
    }
}

#[test]
fn overseer_matrix_refuses_nominal_cells_that_execute_identically() {
    assert_skirmish_bot_eval_refuses(
        &["--against-overseer", "--faction-cells", "authored,fc"],
        &["duplicate executable cells", "Authored", "Fc"],
    );
}

#[test]
fn replay_evidence_requires_its_jsonl_sidecar() {
    let dir = scratch("replay-sidecar-required");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--candidate",
            "candidate-a",
            "--replay-dir",
        ])
        .arg(dir.join("replays"))
        .output()
        .expect("run replay-only evaluation");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--out") && stderr.contains("required"),
        "replay-only refusal should name the required sidecar: {stderr}"
    );
    assert!(!dir.join("replays").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn overseer_mode_refuses_a_severed_ground_map() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios/severance.json"),
            "--ticks",
            "1",
            "--against-overseer",
        ])
        .output()
        .expect("run an Overseer evaluation on a severed map");
    assert!(
        !output.status.success(),
        "a severed map was accepted as a yardstick"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("share no ground route") && stderr.contains("frozen Overseer"),
        "refusal did not explain itself: {stderr}"
    );
}

#[test]
fn a_zero_stall_loop_limit_disables_the_early_stop() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--stall-loop-limit",
            "0",
        ])
        .output()
        .expect("run a bot evaluation without the loop stop");
    assert!(
        output.status.success(),
        "bot-eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("JSONL is UTF-8");
    let row: Value = serde_json::from_str(stdout.lines().next().expect("one row")).unwrap();
    assert_eq!(row["stall_loop_limit"], Value::Null);
    assert_eq!(row["termination"], "tick_limit");
}

#[test]
fn overseer_mode_refuses_a_non_two_seat_scenario() {
    let scenario =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios/compass-grand.json");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("bot-eval")
        .arg(scenario)
        .args(["--ticks", "1", "--against-overseer"])
        .output()
        .expect("run invalid multiseat Overseer evaluation");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("scripted-versus-Overseer evaluations require exactly two seats"),
        "multiseat refusal was unclear: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_skirmish_bot_eval_refuses(args: &[&str], expected: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(["bot-eval", "skirmish", "--ticks", "1"])
        .args(args)
        .output()
        .expect("run invalid bot evaluation");
    assert!(!output.status.success(), "invalid options were accepted");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for fragment in expected {
        assert!(
            stderr.contains(fragment),
            "refusal did not contain {fragment:?}: {stderr}"
        );
    }
}

#[test]
fn paired_cell_emits_two_compact_rows_with_profiles_exchanged() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--scenario-seed-base",
            "13",
            "--personality-seed-base",
            "40",
            "--difficulty",
            "prime",
            "--stance",
            "aggressive",
            "--paired",
        ])
        .output()
        .expect("run paired bot evaluation");
    assert!(
        output.status.success(),
        "bot-eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("JSONL is UTF-8");
    let rows: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["leg"], "forward");
    assert_eq!(rows[1]["leg"], "swapped");
    for row in &rows {
        assert_eq!(row["candidate"], "ad-hoc");
        assert_eq!(row["scenario_seed"], 13);
        assert_eq!(row["tick_limit"], 1);
        assert!(
            row["scenario_fingerprint"]
                .as_str()
                .unwrap()
                .starts_with("fnv1a64:")
        );
        assert_eq!(row["duration_ticks"], 1);
        assert_eq!(row["termination"], "tick_limit");
        assert!(row["result"].is_null());
        assert_eq!(row["winner_seats"], serde_json::json!([]));
        assert!(row["final_hash"].as_str().unwrap().starts_with("0x"));
        assert_eq!(row["evidence"].as_array().unwrap().len(), 2);
    }

    let seed = |row: usize, seat: usize| {
        rows[row]["seats"][seat]["config"]["personality_seed"]
            .as_u64()
            .unwrap()
    };
    assert_eq!((seed(0, 0), seed(0, 1)), (40, 41));
    assert_eq!((seed(1, 0), seed(1, 1)), (41, 40));
    assert_eq!(
        rows[0]["seats"][0]["profile"], rows[1]["seats"][1]["profile"],
        "the complete resolved profile moves with its seed"
    );
    assert_eq!(
        rows[0]["seats"][1]["profile"], rows[1]["seats"][0]["profile"],
        "the complete resolved profile moves with its seed"
    );
}

#[test]
fn cross_difficulty_cells_share_personality_and_swap_complete_configs() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--runs",
            "2",
            "--scenario-seed-base",
            "13",
            "--personality-seed-base",
            "40",
            "--difficulty",
            "prime",
            "--opponent-difficulty",
            "scrapheap",
            "--same-personality-seed",
            "--paired",
        ])
        .output()
        .expect("run cross-difficulty bot evaluation");
    assert!(
        output.status.success(),
        "bot-eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("JSONL is UTF-8");
    let rows: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
        .collect();
    assert_eq!(rows.len(), 4);

    let (cells, remainder) = rows.as_chunks::<2>();
    assert!(remainder.is_empty());
    for (run, legs) in cells.iter().enumerate() {
        let expected_scenario_seed = 13 + run as u64;
        let expected_personality_seed = 40 + run as u64;
        assert_eq!(legs[0]["leg"], "forward");
        assert_eq!(legs[1]["leg"], "swapped");
        assert_eq!(legs[0]["scenario_seed"], expected_scenario_seed);
        assert_eq!(legs[1]["scenario_seed"], expected_scenario_seed);

        let config = |leg: usize, seat: usize| &legs[leg]["seats"][seat]["config"];
        assert_eq!(config(0, 0)["difficulty"], "prime");
        assert_eq!(config(0, 1)["difficulty"], "scrapheap");
        assert_eq!(config(1, 0)["difficulty"], "scrapheap");
        assert_eq!(config(1, 1)["difficulty"], "prime");
        for leg in 0..2 {
            for seat in 0..2 {
                assert_eq!(
                    config(leg, seat)["personality_seed"],
                    expected_personality_seed
                );
            }
        }
        assert_eq!(
            legs[0]["seats"][0]["profile"], legs[1]["seats"][1]["profile"],
            "the complete primary profile moves to the opposite seat"
        );
        assert_eq!(
            legs[0]["seats"][1]["profile"], legs[1]["seats"][0]["profile"],
            "the complete opponent profile moves to the opposite seat"
        );
    }
}

#[test]
fn every_persisted_row_replays_to_its_reported_terminal_state() {
    let dir = scratch("replay-integrity");
    let replay_dir = dir.join("replays");
    let out = dir.join("rows.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "3",
            "--runs",
            "2",
            "--scenario-seed-base",
            "13",
            "--personality-seed-base",
            "40",
            "--difficulty",
            "prime",
            "--stance",
            "aggressive",
            "--opponent-difficulty",
            "veteran",
            "--opponent-stance",
            "turtle",
            "--paired",
            "--candidate",
            "candidate-a",
            "--out",
        ])
        .arg(&out)
        .arg("--replay-dir")
        .arg(&replay_dir)
        .output()
        .expect("run evidence-producing evaluation");
    assert!(
        output.status.success(),
        "bot-eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let rows: Vec<Value> = String::from_utf8(std::fs::read(&out).unwrap())
        .expect("JSONL is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
        .collect();
    assert_eq!(rows.len(), 4, "two runs times two paired legs");

    for row in &rows {
        let replay_path = row["replay"]
            .as_str()
            .map(std::path::PathBuf::from)
            .expect("every persisted row points to its replay");
        assert!(replay_path.is_file(), "missing replay: {replay_path:?}");
        let replay = oxide_kit::load_replay(&replay_path).expect("published replay loads");

        assert_eq!(replay.setup.name, row["scenario"]);
        assert_eq!(replay.setup.seed, row["scenario_seed"]);
        assert_eq!(
            oxide_driver::bot_eval::scenario_fingerprint(&replay.setup).unwrap(),
            row["scenario_fingerprint"]
        );
        assert_eq!(replay.meta.ticks, row["duration_ticks"].as_u64());

        let row_seats = row["seats"].as_array().expect("seat configurations");
        assert_eq!(row_seats.len(), replay.setup.players.len());
        for (seat, player) in replay.setup.players.iter().enumerate() {
            assert_eq!(row_seats[seat]["seat"], seat as u64);
            assert_eq!(
                row_seats[seat]["config"],
                serde_json::to_value(player.bot_config).unwrap(),
                "seat {seat} config differs from the associated replay"
            );
        }

        let state = oxide_kit::runner::run_replay(&replay, None, false)
            .expect("published replay reproduces");
        assert_eq!(state.current_tick(), row["duration_ticks"]);
        assert_eq!(
            oxide_protocol::hash_hex(state.hash()),
            row["final_hash"],
            "row hash must come from its associated replay"
        );
        assert_eq!(serde_json::to_value(state.result()).unwrap(), row["result"]);
        assert_eq!(
            state
                .winners()
                .into_iter()
                .map(|seat| seat.0)
                .collect::<Vec<_>>(),
            serde_json::from_value::<Vec<u8>>(row["winner_seats"].clone()).unwrap()
        );
    }

    assert_eq!(std::fs::read_dir(&replay_dir).unwrap().count(), rows.len());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn overseer_replay_and_sidecar_preserve_exact_controller_identity() {
    let dir = scratch("overseer-provenance");
    let replay_dir = dir.join("replays");
    let out = dir.join("rows.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--against-overseer",
            "--overseer-policy-seed",
            "29",
            "--scenario-seeds",
            "13",
            "--personality-seeds",
            "40",
            "--candidate",
            "candidate-a",
            "--out",
        ])
        .arg(&out)
        .arg("--replay-dir")
        .arg(&replay_dir)
        .output()
        .expect("run persisted Overseer evaluation");
    assert!(
        output.status.success(),
        "bot-eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let row: Value = serde_json::from_slice(
        String::from_utf8(std::fs::read(&out).unwrap())
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(row["seats"][1]["controller"], "overseer");
    assert_eq!(row["seats"][1]["overseer_policy_seed"], 29);
    assert!(
        row["execution_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("fnv1a64:")
    );
    assert!(
        row["command_hash"]
            .as_str()
            .unwrap()
            .starts_with("fnv1a64:")
    );

    let replay_path = std::path::PathBuf::from(row["replay"].as_str().unwrap());
    let replay = oxide_kit::load_replay(&replay_path).unwrap();
    let description = replay.meta.description.as_deref().unwrap();
    assert!(description.contains("\"kind\":\"scripted\""));
    assert!(description.contains("\"kind\":\"overseer\",\"policy_seed\":29"));
    assert_eq!(
        oxide_driver::bot_eval::command_hash(&replay).unwrap(),
        row["command_hash"]
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn opponent_override_refuses_an_ambiguous_multiseat_scenario() {
    let scenario =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios/compass-grand.json");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("bot-eval")
        .arg(scenario)
        .args(["--ticks", "1", "--opponent-difficulty", "prime"])
        .output()
        .expect("run invalid multiseat bot evaluation");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("require exactly two seats"),
        "multiseat refusal was unclear: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("oxide-bot-eval-cli-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn persisted_evidence_requires_an_explicit_candidate() {
    let dir = scratch("candidate-required");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(["bot-eval", "skirmish", "--ticks", "1", "--out"])
        .arg(dir.join("rows.jsonl"))
        .output()
        .expect("run persisted bot evaluation");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--candidate is required"));
    assert!(!dir.join("rows.jsonl").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_later_invalid_scenario_publishes_no_partial_evidence() {
    let dir = scratch("prevalidation");
    let invalid = dir.join("invalid.json");
    let shipped =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios/skirmish.json");
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(shipped).unwrap()).expect("shipped JSON");
    value["players"] = serde_json::json!([]);
    std::fs::write(&invalid, serde_json::to_vec(&value).unwrap()).unwrap();
    let replay_dir = dir.join("replays");
    let out = dir.join("rows.jsonl");

    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(["bot-eval", "skirmish"])
        .arg(&invalid)
        .args(["--ticks", "1", "--candidate", "candidate-a", "--out"])
        .arg(&out)
        .arg("--replay-dir")
        .arg(&replay_dir)
        .output()
        .expect("run invalid evaluation batch");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("needs 1 to 16 players"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.exists(), "the JSONL index was not published");
    assert!(
        !replay_dir.exists() || std::fs::read_dir(&replay_dir).unwrap().next().is_none(),
        "the valid first scenario left replay or staging evidence behind"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn overflowing_seed_ranges_publish_no_partial_evidence() {
    let cases = [
        (
            "scenario",
            "--scenario-seed-base",
            u64::MAX.to_string(),
            "scenario seed range overflows",
        ),
        (
            "personality",
            "--personality-seed-base",
            (u64::MAX - 1).to_string(),
            "personality seed range overflows",
        ),
    ];

    for (name, option, seed, expected) in cases {
        let dir = scratch(&format!("{name}-seed-overflow"));
        let replay_dir = dir.join("replays");
        let out = dir.join("rows.jsonl");
        let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
            .args([
                "bot-eval",
                "skirmish",
                "--ticks",
                "1",
                "--runs",
                "2",
                option,
                &seed,
                "--candidate",
                "candidate-a",
                "--out",
            ])
            .arg(&out)
            .arg("--replay-dir")
            .arg(&replay_dir)
            .output()
            .expect("run overflowing evaluation batch");

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "unexpected {name} seed refusal: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!out.exists(), "the JSONL index was not published");
        assert!(
            !replay_dir.exists() || std::fs::read_dir(&replay_dir).unwrap().next().is_none(),
            "the completed first seed cell left evidence behind"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn rerunning_one_exact_candidate_refuses_to_replace_its_evidence() {
    let dir = scratch("no-overwrite");
    let replay_dir = dir.join("replays");
    let out = dir.join("rows.jsonl");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
            .args([
                "bot-eval",
                "skirmish",
                "--ticks",
                "1",
                "--candidate",
                "candidate-a",
                "--out",
            ])
            .arg(&out)
            .arg("--replay-dir")
            .arg(&replay_dir)
            .output()
            .expect("run evidence-producing evaluation")
    };

    let first = run();
    assert!(
        first.status.success(),
        "first evaluation failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let original_index = std::fs::read(&out).unwrap();
    let replay_paths: Vec<_> = std::fs::read_dir(&replay_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(replay_paths.len(), 1);
    let original_replay = std::fs::read(&replay_paths[0]).unwrap();

    let second = run();
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(std::fs::read(&out).unwrap(), original_index);
    assert_eq!(std::fs::read(&replay_paths[0]).unwrap(), original_replay);
    assert_eq!(std::fs::read_dir(&replay_dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn decision_trace_requires_a_persisted_index_and_candidate() {
    let dir = scratch("trace-contract");
    let trace = dir.join("trace.jsonl");
    let out = dir.join("rows.jsonl");

    let without_index = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--candidate",
            "candidate-a",
            "--decision-trace-out",
        ])
        .arg(&trace)
        .output()
        .expect("run trace-only evaluation");
    assert!(!without_index.status.success());
    let stderr = String::from_utf8_lossy(&without_index.stderr);
    assert!(
        stderr.contains("--decision-trace-out")
            && stderr.contains("--out")
            && stderr.contains("required"),
        "trace-only refusal was unclear: {stderr}"
    );

    let without_candidate = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(["bot-eval", "skirmish", "--ticks", "1", "--out"])
        .arg(&out)
        .arg("--decision-trace-out")
        .arg(&trace)
        .output()
        .expect("run persisted trace without a candidate");
    assert!(!without_candidate.status.success());
    assert!(String::from_utf8_lossy(&without_candidate.stderr).contains("--candidate is required"));
    assert!(!out.exists());
    assert!(!trace.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn decision_trace_is_deterministic_opt_in_evidence_and_omits_overseer() {
    let first = scratch("trace-determinism-first");
    let second = scratch("trace-determinism-second");

    let run = |dir: &std::path::Path| {
        let out = dir.join("rows.jsonl");
        let trace = dir.join("trace.jsonl");
        let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
            .args([
                "bot-eval",
                "skirmish",
                "--ticks",
                "25",
                "--against-overseer",
                "--paired",
                "--scenario-seeds",
                "13",
                "--personality-seeds",
                "40",
                "--candidate",
                "candidate-a",
                "--out",
            ])
            .arg(&out)
            .arg("--decision-trace-out")
            .arg(&trace)
            .output()
            .expect("run traced Overseer evaluation");
        assert!(
            output.status.success(),
            "traced bot-eval failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (std::fs::read(out).unwrap(), std::fs::read(trace).unwrap())
    };

    let (first_rows, first_trace) = run(&first);
    let (second_rows, second_trace) = run(&second);
    assert_eq!(
        first_rows, second_rows,
        "compact rows must stay deterministic"
    );
    assert_eq!(
        first_trace, second_trace,
        "decision trace JSONL must stay deterministic"
    );

    let without_trace = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "25",
            "--against-overseer",
            "--paired",
            "--scenario-seeds",
            "13",
            "--personality-seeds",
            "40",
            "--candidate",
            "candidate-a",
        ])
        .output()
        .expect("run ordinary Overseer evaluation");
    assert!(without_trace.status.success());
    assert_eq!(
        without_trace.stdout, first_rows,
        "enabling trace output must not change normal evaluation rows"
    );

    let rows: Vec<Value> = String::from_utf8(first_rows)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let traces: Vec<Value> = String::from_utf8(first_trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(!traces.is_empty());
    assert!(rows.iter().all(|row| row.get("trace").is_none()));
    for trace in &traces {
        let leg = trace["leg"].as_str().unwrap();
        let expected_scripted_seat = match leg {
            "forward" => 0,
            "swapped" => 1,
            other => panic!("unexpected traced leg {other}"),
        };
        let evaluation = rows.iter().find(|row| row["leg"] == trace["leg"]).unwrap();
        assert_eq!(trace["version"], 1);
        assert_eq!(trace["candidate"], evaluation["candidate"]);
        assert_eq!(
            trace["evaluation_fingerprint"],
            evaluation["evaluation_fingerprint"]
        );
        assert_eq!(trace["seat"], expected_scripted_seat);
        assert_eq!(trace["trace"]["player"], expected_scripted_seat);
        assert_eq!(trace["tick"], trace["trace"]["tick"]);
        assert!(trace["tick"].as_u64().unwrap() < evaluation["duration_ticks"].as_u64().unwrap());
    }

    std::fs::remove_dir_all(first).unwrap();
    std::fs::remove_dir_all(second).unwrap();
}

#[test]
fn decision_trace_refuses_overwrite_before_publishing_other_evidence() {
    let dir = scratch("trace-no-overwrite");
    let out = dir.join("rows.jsonl");
    let trace = dir.join("trace.jsonl");
    std::fs::write(&trace, b"earlier trace").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--candidate",
            "candidate-a",
            "--out",
        ])
        .arg(&out)
        .arg("--decision-trace-out")
        .arg(&trace)
        .output()
        .expect("run evaluation against existing trace evidence");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("refusing to overwrite"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(&trace).unwrap(), b"earlier trace");
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn staged_trace_is_cleaned_up_when_replay_staging_fails() {
    let dir = scratch("trace-stage-cleanup");
    let out = dir.join("rows.jsonl");
    let trace = dir.join("trace.jsonl");
    let replay_blocker = dir.join("not-a-directory");
    std::fs::write(&replay_blocker, b"keep me").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "bot-eval",
            "skirmish",
            "--ticks",
            "1",
            "--candidate",
            "candidate-a",
            "--out",
        ])
        .arg(&out)
        .arg("--decision-trace-out")
        .arg(&trace)
        .arg("--replay-dir")
        .arg(&replay_blocker)
        .output()
        .expect("run evaluation with an invalid replay directory");
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&replay_blocker).unwrap(), b"keep me");
    assert!(!out.exists());
    assert!(!trace.exists());
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "private trace staging files must be removed on another evidence failure"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
