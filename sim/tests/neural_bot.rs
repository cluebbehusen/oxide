//! Integer-inference contracts: the quantized forward pass is exact
//! arithmetic (unit-tested synthetically), the loader is a trust
//! boundary with a numeric contract, and a real exported artifact
//! drives a full deterministic match (env-gated — weights are training
//! output, not test fixtures).

use oxide_sim::bot::{Brain, Difficulty, NeuralBot, QuantNet};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, Scenario};

/// A hand-built artifact: identity-ish single layer, so argmax and
/// masking are checkable by inspection.
fn tiny_artifact() -> serde_json::Value {
    let features = oxide_sim::bot::FEATURE_COUNT;
    let actions = oxide_sim::bot::ACTION_COUNT;
    // recips = 2^24 / 1 (scale 1 per feature); trunk: one 4-wide layer
    // reading features 0..4; head maps those 4 to the first 4 actions.
    // The "tanh" is a monotone integer ramp — ordering is all the
    // argmax assertions need, and the crate rightly denies floats.
    let lut: Vec<i32> = (0..=512).map(|i| (i - 256) * 16).collect();
    let trunk_w: Vec<Vec<i32>> = (0..4)
        .map(|o| {
            let mut row = vec![0; features];
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
        "actions": actions,
        "recips": vec![1 << 24; features],
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
    use oxide_sim::bot::{ACTION_COUNT, Action, Decision, FEATURE_COUNT};
    let net = tiny_net();
    // Feature 2 is the largest → logit 2 wins where legal.
    let mut features = [0i64; FEATURE_COUNT];
    features[0] = 1;
    features[1] = 2;
    features[2] = 3;
    let mut mask = [true; ACTION_COUNT];
    let d = Decision { features, mask };
    assert_eq!(net.act(&d, &[]), Action::TrainSentinel); // index 2
    // Masking index 2 falls back to the next best, index 1.
    mask[2] = false;
    let d = Decision { features, mask };
    assert_eq!(net.act(&d, &[]), Action::TrainHarvester);
    // Nothing legal → Idle, not a panic.
    let d = Decision {
        features,
        mask: [false; ACTION_COUNT],
    };
    assert_eq!(net.act(&d, &[]), Action::Idle);
}

#[test]
fn shape_and_version_drift_is_refused() {
    let mut bad = serde_json::json!({
        "gym_version": 999, "q_bits": 12,
        "features": oxide_sim::bot::FEATURE_COUNT,
        "actions": oxide_sim::bot::ACTION_COUNT,
        "recips": vec![1; oxide_sim::bot::FEATURE_COUNT], "tanh_lut": vec![0; 513],
        "layers": [],
        "head": {
            "w": vec![vec![0; oxide_sim::bot::FEATURE_COUNT]; oxide_sim::bot::ACTION_COUNT],
            "b": vec![0; oxide_sim::bot::ACTION_COUNT],
        },
    });
    assert!(QuantNet::from_json(&bad.to_string()).is_err(), "version");
    bad["gym_version"] = oxide_sim::bot::GYM_VERSION.into();
    bad["head"]["w"] = serde_json::json!(vec![vec![0; 7]; 11]);
    assert!(QuantNet::from_json(&bad.to_string()).is_err(), "shape");
}

/// `--weights` loads files the sim did not write, so the loader is a
/// trust boundary: every tensor that feeds the integer kernel carries
/// a magnitude ceiling, and a breach names itself.
#[test]
fn numeric_drift_is_refused_and_the_offending_tensor_is_named() {
    let features = oxide_sim::bot::FEATURE_COUNT;
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
    art["layers"] = serde_json::json!([{"w": vec![vec![0; features]; wide], "b": vec![0; wide]}]);
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
    let actions = oxide_sim::bot::ACTION_COUNT;
    let lut: Vec<i32> = (0..=512)
        .map(|i| if i % 2 == 0 { 1 << 13 } else { -(1 << 13) })
        .collect();
    let mut layers = vec![serde_json::json!({
        "w": vec![vec![1 << 20; features]; 4], "b": vec![-(1 << 20); 4],
    })];
    layers.resize(
        16,
        serde_json::json!({"w": vec![vec![1 << 20; 4]; 4], "b": vec![-(1 << 20); 4]}),
    );
    let art = serde_json::json!({
        "gym_version": oxide_sim::bot::GYM_VERSION,
        "q_bits": 12,
        "features": features,
        "actions": actions,
        "recips": vec![1i64 << 26; features],
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
    let conditioning = 3;
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

/// The exporter's structural maxima against the loader's ceilings — a
/// future architecture that drifts toward a limit trips here, where
/// the checkpoint is still in hand, not at promotion time.
#[test]
fn the_shipped_artifact_clears_every_ceiling_with_room() {
    fn ints(v: &serde_json::Value) -> Vec<i64> {
        match v {
            serde_json::Value::Array(a) => a.iter().flat_map(ints).collect(),
            serde_json::Value::Number(n) => vec![n.as_i64().expect("integer tensor")],
            _ => Vec::new(),
        }
    }
    let peak = |v: &serde_json::Value| ints(v).into_iter().map(i64::abs).max().unwrap_or(0);
    let art: serde_json::Value =
        serde_json::from_str(include_str!("../src/bot/ladder_weights.json")).unwrap();

    let recip = peak(&art["recips"]);
    assert!(recip <= 1 << 26, "recip peak {recip} over 2^26");
    let lut = peak(&art["tanh_lut"]);
    assert!(lut <= 1 << 13, "lut peak {lut} over 2^13");
    let layers = art["layers"].as_array().unwrap();
    assert!(layers.len() <= 16, "{} trunk layers over 16", layers.len());
    for l in layers.iter().chain(std::iter::once(&art["head"])) {
        let rows = l["w"].as_array().unwrap();
        assert!(rows.len() <= 4096, "{} wide, over 4096", rows.len());
        let coeff = peak(&l["w"]).max(peak(&l["b"]));
        assert!(coeff <= 1 << 20, "coefficient peak {coeff} over 2^20");
    }
    // Printed so a drifting exporter is visible before it is fatal.
    println!("shipped peaks: recip {recip}, lut {lut}");
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
        let mut neural = NeuralBot::new(PlayerId(0), 16, net.clone(), oxide_sim::Faction::Ferrous);
        let mut veteran = Brain::for_tier(PlayerId(1), 7, Difficulty::Veteran);
        for _ in 0..40_000u32 {
            let mut commands = neural.act(&state);
            commands.extend(veteran.act(&state));
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
    println!("result: {r1:?}");
    assert_eq!(
        r1,
        Some(GameResult::Victory { team: 0 }),
        "the exported policy should beat Veteran"
    );
}
