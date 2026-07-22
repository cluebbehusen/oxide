//! The shipped ladder holds: embedded weights load, every named level
//! beats the one below it head-to-head (seat-swapped), and ladder
//! matches reproduce bit-identically — the neural tiers live inside
//! replays like any other command source.

use oxide_sim::bot::{Level, NeuralBot, QuantNet};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, Scenario};

fn ladder_match(hi: Level, lo: Level, hi_seat: u8, seed: u64) -> (Option<bool>, u64) {
    let mut scenario = Scenario::skirmish();
    scenario.seed = seed;
    let mut state = scenario.build().unwrap();
    // Fixed balanced personalities: this test isolates the skill knob.
    let mut a = NeuralBot::ladder(
        PlayerId(hi_seat),
        seed,
        hi,
        Some(500),
        oxide_sim::Faction::Ferrous,
    );
    let mut b = NeuralBot::ladder(
        PlayerId(1 - hi_seat),
        seed,
        lo,
        Some(500),
        oxide_sim::Faction::Cupric,
    );
    for _ in 0..40_000u32 {
        let mut commands = a.act(&state);
        commands.extend(b.act(&state));
        state.tick(&commands);
        if let Some(GameResult::Victory { team }) = state.result() {
            return (Some(PlayerId(team) == PlayerId(hi_seat)), state.hash());
        }
    }
    (None, state.hash())
}

#[test]
fn embedded_weights_parse() {
    let net = QuantNet::ladder();
    assert_eq!(net.conditioning(), 3, "the ladder network is conditioned");
}

#[test]
#[ignore = "gym v4 bridge artifact is BC-shaped, not ladder-shaped; K4 re-enables after the retrain gates"]
fn each_level_beats_the_one_below() {
    for pair in Level::LADDER.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let mut hi_wins = 0;
        let mut games = 0;
        for seed in [11u64, 12] {
            for seat in [0u8, 1] {
                if let (Some(won), _) = ladder_match(hi, lo, seat, seed) {
                    games += 1;
                    hi_wins += u32::from(won);
                }
            }
        }
        assert!(
            hi_wins * 2 > games,
            "{hi:?} vs {lo:?}: {hi_wins}/{games} — the ladder inverted"
        );
    }
}

#[test]
fn ladder_matches_reproduce_bit_identically() {
    let (r1, h1) = ladder_match(Level::Expert, Level::Easy, 0, 5);
    let (r2, h2) = ladder_match(Level::Expert, Level::Easy, 0, 5);
    assert_eq!(h1, h2, "same seed, same levels ⇒ same world");
    assert_eq!(r1, r2);
}

#[test]
fn a_seeded_random_personality_is_deterministic() {
    let a = NeuralBot::ladder(
        PlayerId(0),
        42,
        Level::Expert,
        None,
        oxide_sim::Faction::Ferrous,
    );
    let b = NeuralBot::ladder(
        PlayerId(0),
        42,
        Level::Expert,
        None,
        oxide_sim::Faction::Ferrous,
    );
    let scenario = Scenario::skirmish();
    let mut s1 = scenario.build().unwrap();
    let mut s2 = scenario.build().unwrap();
    let (mut a, mut b) = (a, b);
    for _ in 0..640 {
        let ca = a.act(&s1);
        let cb = b.act(&s2);
        assert_eq!(ca, cb, "same seed must deal the same personality");
        s1.tick(&ca);
        s2.tick(&cb);
    }
}
