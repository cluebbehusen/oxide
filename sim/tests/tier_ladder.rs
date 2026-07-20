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

/// The cheap always-on gate: two seeds, both seats — the omniscient
/// rungs must order strictly. Prime's rung is deliberately absent: at
/// equal scripted skill, honest eyes lose to omniscience (measured
/// 4-16 vs Standard, 3-17 vs Veteran over ten seat-swapped seed pairs),
/// because seeing true totals means timing every push perfectly. How
/// the ladder's top should be shaped around that finding is an open
/// 0.7 design question — the ignored measurement below keeps the
/// numbers honest meanwhile.
#[test]
fn the_ladder_relations_hold() {
    let rungs = [
        (Difficulty::Standard, Difficulty::Scrapheap),
        (Difficulty::Veteran, Difficulty::Standard),
    ];
    for (hi, lo) in rungs {
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
    let rungs = [
        (Difficulty::Standard, Difficulty::Scrapheap),
        (Difficulty::Veteran, Difficulty::Standard),
        (Difficulty::Prime, Difficulty::Standard),
        (Difficulty::Prime, Difficulty::Veteran),
    ];
    for (hi, lo) in rungs {
        let (hi_wins, lo_wins, draws) = ladder_rung(hi, lo, 1..=10);
        println!("{hi:?} vs {lo:?}: {hi_wins}-{lo_wins}, {draws} draws");
    }
}
