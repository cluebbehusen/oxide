//! Phase-B health gate: the channel-based brain must beat the classic
//! 0.6 rule cascade head-to-head — from either seat (the known seat-1
//! reflex edge must not be what carries it), and both omnisciently and
//! fog-honestly.

use oxide_sim::bot::{Bot, Brain, Dials};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, Scenario};

/// Runs brain vs classic on the skirmish map; returns the winner.
fn duel(brain_seat: u8, dials: Dials) -> Option<PlayerId> {
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    let mut brain = Brain::new(PlayerId(brain_seat), scenario.seed, dials);
    let mut classic = Bot::new(PlayerId(1 - brain_seat), scenario.seed);
    for _ in 0..40_000u32 {
        let mut commands = brain.act(&state);
        commands.extend(classic.act(&state));
        state.tick(&commands);
        if let Some(GameResult::Victory { team }) = state.result() {
            return Some(PlayerId(team));
        }
    }
    None
}

#[test]
fn omniscient_brain_beats_classic_from_either_seat() {
    for seat in [0u8, 1] {
        let winner = duel(seat, Dials::full_omniscient());
        assert_eq!(
            winner,
            Some(PlayerId(seat)),
            "omniscient brain in seat {seat} should beat the classic bot"
        );
    }
}

#[test]
fn fog_honest_brain_beats_classic_from_either_seat() {
    // The hard version of the gate: the brain plays through its own
    // vision while the classic bot cheats — and still must win.
    for seat in [0u8, 1] {
        let winner = duel(seat, Dials::full());
        assert_eq!(
            winner,
            Some(PlayerId(seat)),
            "fog-honest brain in seat {seat} should beat the classic bot"
        );
    }
}
