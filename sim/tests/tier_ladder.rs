//! The difficulty ladder holds: each tier beats the one below it,
//! measured over seat-swapped pairs (the sim's residual id-order micro
//! favors a seat in mirror-like fights; swapping seats scores the
//! matchup, not the chair).

use oxide_sim::bot::{Brain, Difficulty};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, Scenario};

/// Runs one match, `hi` tier in `hi_seat`; returns Some(true) if the
/// higher tier won.
fn class_match(hi: Difficulty, lo: Difficulty, hi_seat: u8, seed: u64) -> Option<bool> {
    let mut scenario = Scenario::skirmish();
    scenario.seed = seed;
    let mut state = scenario.build().unwrap();
    let mut a = Brain::for_tier(PlayerId(hi_seat), seed, hi);
    let mut b = Brain::for_tier(PlayerId(1 - hi_seat), seed, lo);
    for _ in 0..60_000u32 {
        let mut commands = a.act(&state);
        commands.extend(b.act(&state));
        state.tick(&commands);
        if let Some(GameResult::Victory { winner }) = state.result() {
            return Some(winner == PlayerId(hi_seat));
        }
    }
    None
}

fn ladder_rung(
    hi: Difficulty,
    lo: Difficulty,
    seeds: std::ops::RangeInclusive<u64>,
) -> (u32, u32, u32) {
    let (mut hi_wins, mut lo_wins, mut draws) = (0, 0, 0);
    for seed in seeds {
        for seat in [0u8, 1] {
            match class_match(hi, lo, seat, seed) {
                Some(true) => hi_wins += 1,
                Some(false) => lo_wins += 1,
                None => draws += 1,
            }
        }
    }
    (hi_wins, lo_wins, draws)
}

/// The cheap always-on gate: two seeds, both seats, every adjacent
/// rung — the higher tier must win strictly more than it loses. With
/// every tier fog-honest this holds all the way up (the full ten-seed
/// measurement below reads 20-0 per rung); mixing honest and
/// omniscient rungs is what used to break it.
#[test]
fn each_tier_beats_the_one_below() {
    for pair in Difficulty::LADDER.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let (hi_wins, lo_wins, draws) = ladder_rung(hi, lo, 1..=2);
        assert!(
            hi_wins > lo_wins,
            "{hi:?} vs {lo:?}: {hi_wins}-{lo_wins} ({draws} draws) — the ladder inverted"
        );
    }
}

/// The full measurement (slow; run in release with --ignored).
#[test]
#[ignore]
fn measure_tier_ladder() {
    for pair in Difficulty::LADDER.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let (hi_wins, lo_wins, draws) = ladder_rung(hi, lo, 1..=10);
        println!("{hi:?} vs {lo:?}: {hi_wins}-{lo_wins}, {draws} draws");
    }
}
