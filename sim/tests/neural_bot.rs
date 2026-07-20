//! Integer-inference contracts: the quantized forward pass is exact
//! arithmetic (unit-tested synthetically), and a real exported
//! artifact drives a full deterministic match (env-gated — weights are
//! training output, not test fixtures).

use oxide_sim::bot::{Brain, Difficulty, NeuralBot, QuantNet};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, Scenario};

/// A hand-built artifact: identity-ish single layer, so argmax and
/// masking are checkable by inspection.
fn tiny_net() -> QuantNet {
    let features = 32;
    let actions = 11;
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
    let artifact = serde_json::json!({
        "gym_version": oxide_sim::bot::GYM_VERSION,
        "q_bits": 12,
        "features": features,
        "actions": actions,
        "recips": vec![1 << 24; features],
        "tanh_lut": lut,
        "layers": [{"w": trunk_w, "b": vec![0; 4]}],
        "head": {"w": head_w, "b": vec![0; actions]},
    });
    QuantNet::from_json(&artifact.to_string()).unwrap()
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
    assert_eq!(net.act(&d), Action::TrainSentinel); // index 2
    // Masking index 2 falls back to the next best, index 1.
    mask[2] = false;
    let d = Decision { features, mask };
    assert_eq!(net.act(&d), Action::TrainHarvester);
    // Nothing legal → Idle, not a panic.
    let d = Decision {
        features,
        mask: [false; ACTION_COUNT],
    };
    assert_eq!(net.act(&d), Action::Idle);
}

#[test]
fn shape_and_version_drift_is_refused() {
    let mut bad = serde_json::json!({
        "gym_version": 999, "q_bits": 12, "features": 32, "actions": 11,
        "recips": vec![1; 32], "tanh_lut": vec![0; 513],
        "layers": [], "head": {"w": vec![vec![0; 32]; 11], "b": vec![0; 11]},
    });
    assert!(QuantNet::from_json(&bad.to_string()).is_err(), "version");
    bad["gym_version"] = oxide_sim::bot::GYM_VERSION.into();
    bad["head"]["w"] = serde_json::json!(vec![vec![0; 7]; 11]);
    assert!(QuantNet::from_json(&bad.to_string()).is_err(), "shape");
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
        let mut neural = NeuralBot::new(PlayerId(0), 16, net.clone());
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
        Some(GameResult::Victory {
            winner: PlayerId(0)
        }),
        "the exported policy should beat Veteran"
    );
}
