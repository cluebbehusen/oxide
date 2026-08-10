//! Integer-inference contracts: the quantized forward pass is exact
//! arithmetic (unit-tested synthetically), the loader is a trust
//! boundary with a numeric contract, and a real exported artifact
//! drives a full deterministic match (env-gated — weights are training
//! output, not test fixtures).

use oxide_sim::bot::{Brain, FEATURE_COUNT, Level, NeuralBot, QuantNet, ladder_condition_values};
use oxide_sim::state::GameResult;
use oxide_sim::{Faction, PlayerId, Scenario};

/// A hand-built artifact: identity-ish single layer, so argmax and
/// masking are checkable by inspection.
fn tiny_artifact() -> serde_json::Value {
    let features = oxide_sim::bot::FEATURE_COUNT;
    let conditioning = oxide_sim::bot::CONDITIONING_COUNT;
    let inputs = features + conditioning;
    let actions = oxide_sim::bot::ACTION_COUNT;
    // recips = 2^24 / 1 (scale 1 per feature); trunk: one 4-wide layer
    // reading features 0..4; head maps those 4 to the first 4 actions.
    // The "tanh" is a monotone integer ramp — ordering is all the
    // argmax assertions need, and the crate rightly denies floats.
    let lut: Vec<i32> = (0..=512).map(|i| (i - 256) * 16).collect();
    let trunk_w: Vec<Vec<i32>> = (0..4)
        .map(|o| {
            let mut row = vec![0; inputs];
            row[o] = 4096;
            row
        })
        .collect();
    let head_w: Vec<Vec<i32>> = (0..actions)
        .map(|o| {
            let mut row = vec![0; 4];
            if o < 4 {
                row[o] = 4096;
            }
            row
        })
        .collect();
    serde_json::json!({
        "gym_version": oxide_sim::bot::GYM_VERSION,
        "q_bits": 12,
        "features": features,
        "conditioning": conditioning,
        "actions": actions,
        "recips": vec![1 << 24; inputs],
        "tanh_lut": lut,
        "layers": [{"w": trunk_w, "b": vec![0; 4]}],
        "head": {"w": head_w, "b": vec![0; actions]},
    })
}

fn tiny_net() -> QuantNet {
    QuantNet::from_json(&tiny_artifact().to_string()).unwrap()
}

#[test]
fn masked_argmax_is_exact_and_deterministic() {
    use oxide_sim::bot::{ACTION_COUNT, Action, ActionPlan, Decision};
    let net = tiny_net();
    // Feature 2 is the largest → logit 2 wins where legal.
    let mut features = [0i64; FEATURE_COUNT];
    features[0] = 1;
    features[1] = 2;
    features[2] = 3;
    let mut mask = [true; ACTION_COUNT];
    let d = Decision { features, mask };
    assert_eq!(
        net.act(&d, &[0; oxide_sim::bot::CONDITIONING_COUNT]),
        ActionPlan {
            production: Action::TrainSentinel,
            construction: Action::NoConstruction,
            upgrade: Action::NoUpgrade,
            operation: Action::NoOperation,
        }
    ); // production index 2
    // Masking index 2 falls back to the next best, index 1.
    mask[2] = false;
    let d = Decision { features, mask };
    assert_eq!(
        net.act(&d, &[0; oxide_sim::bot::CONDITIONING_COUNT])
            .production,
        Action::TrainHarvester
    );
    // Nothing legal → the all-head no-op plan, not a panic.
    let d = Decision {
        features,
        mask: [false; ACTION_COUNT],
    };
    assert_eq!(
        net.act(&d, &[0; oxide_sim::bot::CONDITIONING_COUNT]),
        ActionPlan::default()
    );
}

#[test]
fn shape_and_version_drift_is_refused() {
    let mut bad = serde_json::json!({
        "gym_version": 999, "q_bits": 12,
        "features": oxide_sim::bot::FEATURE_COUNT,
        "conditioning": oxide_sim::bot::CONDITIONING_COUNT,
        "actions": oxide_sim::bot::ACTION_COUNT,
        "recips": vec![1; oxide_sim::bot::FEATURE_COUNT + oxide_sim::bot::CONDITIONING_COUNT],
        "tanh_lut": vec![0; 513],
        "layers": [],
        "head": {
            "w": vec![vec![0; oxide_sim::bot::FEATURE_COUNT + oxide_sim::bot::CONDITIONING_COUNT]; oxide_sim::bot::ACTION_COUNT],
            "b": vec![0; oxide_sim::bot::ACTION_COUNT],
        },
    });
    assert!(QuantNet::from_json(&bad.to_string()).is_err(), "version");
    bad["gym_version"] = oxide_sim::bot::GYM_VERSION.into();
    bad["head"]["w"] = serde_json::json!(vec![vec![0; 7]; 11]);
    assert!(QuantNet::from_json(&bad.to_string()).is_err(), "shape");
}

/// Old artifacts are refused by version, plainly: the v9 retrain starts
/// from scratch, so there is no supported widening migration to name.
#[test]
fn a_stale_gym_version_artifact_is_refused_by_name() {
    let mut old = tiny_artifact();
    old["gym_version"] = 7.into();
    let err = QuantNet::from_json(&old.to_string()).expect_err("v7 is not silently accepted");
    assert!(err.contains("weights speak gym v7, sim speaks v9"), "{err}");
}

/// `--weights` loads files the sim did not write, so the loader is a
/// trust boundary: every tensor that feeds the integer kernel carries
/// a magnitude ceiling, and a breach names itself.
#[test]
fn numeric_drift_is_refused_and_the_offending_tensor_is_named() {
    let features = oxide_sim::bot::FEATURE_COUNT;
    let inputs = features + oxide_sim::bot::CONDITIONING_COUNT;
    let actions = oxide_sim::bot::ACTION_COUNT;
    let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();

    let mut art = tiny_artifact();
    art["recips"][0] = i64::MAX.into();
    cases.push(("recips[0]", art));

    // A zero recip is not merely out of range: it silently blanks a
    // feature column, which a shape check would wave through.
    let mut art = tiny_artifact();
    art["recips"][3] = 0.into();
    cases.push(("recips[3]", art));

    let mut art = tiny_artifact();
    art["recips"][7] = (-1).into();
    cases.push(("recips[7]", art));

    let mut art = tiny_artifact();
    art["tanh_lut"][100] = (1 << 20).into();
    cases.push(("tanh_lut[100]", art));

    let mut art = tiny_artifact();
    art["layers"][0]["w"][1][2] = i32::MAX.into();
    cases.push(("layer 0 w[1][2]", art));

    let mut art = tiny_artifact();
    art["layers"][0]["b"][0] = i32::MIN.into();
    cases.push(("layer 0 b[0]", art));

    let mut art = tiny_artifact();
    art["head"]["b"][5] = (1 << 21).into();
    cases.push(("head b[5]", art));

    // Seventeen layers, each shape-valid: only the count is wrong.
    let mut art = tiny_artifact();
    let mut layers = vec![serde_json::json!({
        "w": vec![vec![0; features]; 4], "b": vec![0; 4],
    })];
    layers.resize(
        17,
        serde_json::json!({"w": vec![vec![0; 4]; 4], "b": vec![0; 4]}),
    );
    art["layers"] = layers.into();
    cases.push(("trunk layers", art));

    // One layer past the width ceiling — the shape stays consistent
    // end to end, so only the ceiling refuses it.
    let wide = 4097;
    let mut art = tiny_artifact();
    art["layers"] = serde_json::json!([{"w": vec![vec![0; inputs]; wide], "b": vec![0; wide]}]);
    art["head"] = serde_json::json!({"w": vec![vec![0; wide]; actions], "b": vec![0; actions]});
    cases.push(("layer 0 is 4097 wide", art));

    // A hostile conditioning count must be refused BEFORE the input
    // width is computed: near usize::MAX the sum itself overflows —
    // a debug panic and a release wrap, the build-profile split this
    // boundary exists to close.
    let mut art = tiny_artifact();
    art["conditioning"] = serde_json::json!(u64::MAX);
    cases.push(("conditioning", art));

    for (named, art) in cases {
        let err = QuantNet::from_json(&art.to_string())
            .err()
            .unwrap_or_else(|| panic!("{named} should be refused"));
        assert!(err.contains(named), "{err:?} should name {named}");
    }
}

/// Values sitting exactly ON every ceiling are legal — the bounds are
/// the exporter's contract, not a fence one rounding step inside it.
#[test]
fn boundary_values_are_accepted() {
    let features = oxide_sim::bot::FEATURE_COUNT;
    let conditioning = oxide_sim::bot::CONDITIONING_COUNT;
    let actions = oxide_sim::bot::ACTION_COUNT;
    let lut: Vec<i32> = (0..=512)
        .map(|i| if i % 2 == 0 { 1 << 13 } else { -(1 << 13) })
        .collect();
    let mut layers = vec![serde_json::json!({
        "w": vec![vec![1 << 20; features + conditioning]; 4], "b": vec![-(1 << 20); 4],
    })];
    layers.resize(
        16,
        serde_json::json!({"w": vec![vec![1 << 20; 4]; 4], "b": vec![-(1 << 20); 4]}),
    );
    let art = serde_json::json!({
        "gym_version": oxide_sim::bot::GYM_VERSION,
        "q_bits": 12,
        "features": features,
        "conditioning": conditioning,
        "actions": actions,
        "recips": vec![1i64 << 26; features + conditioning],
        "tanh_lut": lut,
        "layers": layers,
        "head": {"w": vec![vec![1 << 20; 4]; actions], "b": vec![1 << 20; actions]},
    });
    assert!(QuantNet::from_json(&art.to_string()).is_ok());
}

/// The safety argument, exercised rather than argued: an artifact
/// maxed out on every accepted ceiling, fed saturated observations,
/// must infer without wrapping. Debug builds check overflow, so a
/// kernel whose accumulator bound is wrong panics here.
#[test]
fn a_maxed_out_artifact_infers_on_saturated_features_without_overflow() {
    let features = oxide_sim::bot::FEATURE_COUNT;
    let actions = oxide_sim::bot::ACTION_COUNT;
    let conditioning = oxide_sim::bot::CONDITIONING_COUNT;
    let width = 4096; // the accepted ceiling
    let coeff = 1 << 20;
    let row = |n: usize| -> Vec<i32> {
        (0..n)
            .map(|i| if i % 2 == 0 { coeff } else { -coeff })
            .collect()
    };
    let lut: Vec<i32> = (0..=512)
        .map(|i| if i % 2 == 0 { 1 << 13 } else { -(1 << 13) })
        .collect();
    let art = serde_json::json!({
        "gym_version": oxide_sim::bot::GYM_VERSION,
        "q_bits": 12,
        "features": features,
        "conditioning": conditioning,
        "actions": actions,
        "recips": vec![1i64 << 26; features + conditioning],
        "tanh_lut": lut,
        "layers": [{"w": vec![row(features + conditioning); width], "b": row(width)}],
        "head": {"w": vec![row(width); actions], "b": row(actions)},
    });
    let net = QuantNet::from_json(&art.to_string()).unwrap();
    let logits = net.logits(
        &[i64::MAX; oxide_sim::bot::FEATURE_COUNT],
        &[i64::MIN, i64::MAX, 1000],
    );
    assert_eq!(logits.len(), actions);
    // The head accumulates 4096 terms of |w| <= 2^20 times a tanh
    // output <= 2^13, shifted down by Q: comfortably inside 2^34.
    assert!(logits.iter().all(|l| l.abs() < 1 << 34), "{logits:?}");
}

/// The digest answers "which weights produced this number" — so it
/// must follow the coefficients, not the file's layout or the
/// metadata `ArtifactDto` ignores.
#[test]
fn the_digest_follows_coefficients_not_formatting() {
    let art = tiny_artifact();
    let base = QuantNet::from_json(&art.to_string()).unwrap().digest();
    let pretty = QuantNet::from_json(&serde_json::to_string_pretty(&art).unwrap())
        .unwrap()
        .digest();
    assert_eq!(base, pretty, "reformatting must not move the digest");

    let mut tagged = art.clone();
    tagged["arch"] = "deep".into();
    tagged["update"] = 1300.into();
    let tagged = QuantNet::from_json(&tagged.to_string()).unwrap().digest();
    assert_eq!(base, tagged, "metadata must not move the digest");

    let mut nudged = art.clone();
    nudged["layers"][0]["w"][0][0] = 4097.into();
    let nudged = QuantNet::from_json(&nudged.to_string()).unwrap().digest();
    assert_ne!(base, nudged, "one coefficient must move the digest");
}

#[test]
fn lineage_is_verified_but_does_not_enter_the_gameplay_digest() {
    let legacy = tiny_artifact();
    let base = QuantNet::from_json(&legacy.to_string()).unwrap().digest();

    // Generated by tools/train/lineage.py. The edge values pin the
    // cross-language canonical form: ensure_ascii strings, surrogate
    // pairs, signed zero, padded exponents, and both notation boundaries.
    let python_lineage: serde_json::Value = serde_json::from_str(
        r#"{"hyperparameters":{"below":1e-06,"label":"\u00e9\u007f","lower_fixed":0.0001,"lower_scientific":1e-05,"negative_zero":-0.0,"rounding_edge":9.999999999999999e-05,"upper_fixed":1000000000000000.0,"upper_scientific":1e+16},"inputs":{"source":{"content_sha256":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},"lineage_id":"sha256:d395b2633e76e8b5b38977b912cc396abb410c3a51802599afa0dd0a00e22d6b","phase":"revival-\ud83d\ude00","phase_start_update":0,"schema":1}"#,
    )
    .unwrap();
    let mut attributed = legacy;
    attributed["lineage"] = python_lineage;
    let attributed_digest = QuantNet::from_json(&attributed.to_string())
        .unwrap()
        .digest();
    assert_eq!(
        base, attributed_digest,
        "Python-canonicalized provenance must verify without changing gameplay identity"
    );

    attributed["lineage"]["phase"] = "forged-history".into();
    let error = QuantNet::from_json(&attributed.to_string()).unwrap_err();
    assert!(
        error.contains("lineage_id does not match"),
        "tampered provenance was refused for the wrong reason: {error}"
    );
}

/// The full pipeline: exported weights drive a deterministic match.
/// Run with OXIDE_WEIGHTS=/path/to/artifact.json — promotion tooling
/// does; CI has no weights and skips via ignore.
#[test]
#[ignore]
fn exported_weights_play_a_deterministic_match() {
    let path = std::env::var("OXIDE_WEIGHTS").expect("set OXIDE_WEIGHTS");
    let json = std::fs::read_to_string(&path).unwrap();
    let net = QuantNet::from_json(&json).unwrap();
    let run = || {
        let mut scenario = Scenario::skirmish();
        scenario.seed = 7;
        let mut state = scenario.build().unwrap();
        let mut neural = NeuralBot::new(PlayerId(0), 16, net.clone(), Faction::Ferrous);
        let mut overseer = Brain::overseer(PlayerId(1), 7);
        for _ in 0..40_000u32 {
            let mut commands = neural.act(&state);
            commands.extend(overseer.act(&state));
            state.tick(&commands);
            if state.result().is_some() {
                break;
            }
        }
        (state.hash(), state.result())
    };
    let (h1, r1) = run();
    let (h2, r2) = run();
    assert_eq!(h1, h2, "neural matches must reproduce bit-identically");
    assert_eq!(r1, r2);
    // Beating the Overseer is a promotion-battery question, not this
    // gate's: here the match must merely be decided-or-capped the same
    // way every run. The printed result feeds the promotion tooling.
    println!("result: {r1:?}");
}

// --- Candidate promotion gates (salvaged from the retired shipped-ladder
// suite): env-gated checks the retrain-era tooling runs against exported
// artifacts. CI has no weights and skips via ignore. ---

const RAW_AGGRESSION_CENTERS: [u32; 2] = [300, 550];
const YARDSTICK_SEEDS: [u64; 10] = [3000, 3001, 3002, 3003, 3004, 3005, 3006, 3007, 3008, 3009];

/// Wins and total victory ticks for `level` against the Overseer — the
/// one scripted yardstick left standing — at both pinned raw-aggression
/// centers over the seed set, seat-swapped.
/// A loss counts the full 40k-tick horizon toward the total, so the
/// tick sum subsumes the win count at the losing end and stays a
/// single monotone instrument. Every match is an independent
/// deterministic sim, so the slate fans out across threads; the
/// totals are order-free.
fn yardstick_with_net(level: Level, net: &QuantNet) -> (u32, u64) {
    let mut matches = Vec::new();
    for aggression in RAW_AGGRESSION_CENTERS {
        for seed in YARDSTICK_SEEDS {
            for seat in [0u8, 1] {
                matches.push((seed, seat, aggression));
            }
        }
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = matches
            .into_iter()
            .map(|(seed, seat, aggression)| {
                let net = net.clone();
                scope.spawn(move || {
                    let mut sc = Scenario::skirmish();
                    sc.seed = seed;
                    let mut state = sc.build().unwrap();
                    let faction = sc.players[seat as usize].faction;
                    let mut bot = NeuralBot::ladder_with_net(
                        PlayerId(seat),
                        seed,
                        level,
                        Some(aggression),
                        faction,
                        net,
                    );
                    let mut opp = Brain::overseer(PlayerId(1 - seat), seed);
                    let horizon = 40_000u32;
                    let mut end = u64::from(horizon);
                    for t in 0..horizon {
                        let mut commands = bot.act(&state);
                        commands.extend(opp.act(&state));
                        state.tick(&commands);
                        if state.result().is_some() {
                            end = u64::from(t);
                            break;
                        }
                    }
                    let won = matches!(state.result(), Some(GameResult::Victory { .. }))
                        && state.winners().contains(&PlayerId(seat));
                    if won {
                        (1u32, end)
                    } else {
                        (0u32, u64::from(horizon))
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a yardstick match panicked"))
            .fold((0u32, 0u64), |(w, t), (dw, dt)| (w + dw, t + dt))
    })
}

#[test]
#[ignore = "candidate gate: set OXIDE_LADDER_WEIGHTS to an exported artifact"]
fn candidate_orders_against_the_overseer_yardstick() {
    let path = std::env::var("OXIDE_LADDER_WEIGHTS").expect("OXIDE_LADDER_WEIGHTS");
    let json = std::fs::read_to_string(path).expect("candidate artifact");
    let net = QuantNet::from_json(&json).expect("valid candidate artifact");
    let totals: Vec<(u32, u64)> = Level::LADDER
        .iter()
        .map(|level| yardstick_with_net(*level, &net))
        .collect();
    let max = 40u32; // 2 styles x 10 seeds x 2 seats against the Overseer
    println!("\nCANDIDATE OVERSEER YARDSTICK  ·  skirmish  ·  {max} matches/rung  ·  40k horizon");
    for (level, (wins, ticks)) in Level::LADDER.iter().zip(&totals) {
        let rung = format!("{level:?}");
        println!("  {rung:<8} wins {wins:>2}/{max}  ·  tick total {ticks:>9}");
    }
    for pair in totals.windows(2) {
        assert!(
            pair[0].0 < pair[1].0,
            "a higher rung must win more of the same slate: {totals:?} of {max}"
        );
        assert!(
            pair[0].1 > pair[1].1,
            "a higher rung must put the same slate away faster: {totals:?} of {max}"
        );
    }
    for lower in &totals[..3] {
        assert!(
            lower.0 < totals[3].0,
            "Expert must hold the top win count outright: {totals:?}"
        );
    }
}

#[test]
#[ignore = "candidate gate: set OXIDE_PROFILE_WEIGHTS and OXIDE_PARENT_WEIGHTS"]
fn candidate_raw_aggression_path_matches_parent_exactly() {
    let load = |name: &str| {
        let path = std::env::var(name).unwrap_or_else(|_| panic!("{name}"));
        let json = std::fs::read_to_string(path).expect("candidate artifact");
        QuantNet::from_json(&json).expect("valid candidate artifact")
    };
    let candidate = load("OXIDE_PROFILE_WEIGHTS");
    let parent = load("OXIDE_PARENT_WEIGHTS");
    let feature_cases = [
        [0; FEATURE_COUNT],
        std::array::from_fn(|index| (index as i64 * 97) % 1_001),
        std::array::from_fn(|index| 1_000 - (index as i64 * 131) % 1_001),
    ];

    for features in feature_cases {
        for faction in [Faction::Ferrous, Faction::Cupric] {
            for aggression in 0..=1_000 {
                let knobs = ladder_condition_values(aggression, faction);
                assert_eq!(
                    candidate.logits(&features, &knobs),
                    parent.logits(&features, &knobs),
                    "raw aggression {aggression} for {faction:?} changed"
                );
            }
        }
    }
}
