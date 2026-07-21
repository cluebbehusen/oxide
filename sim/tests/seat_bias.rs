//! Seat-bias quantification: mirror matches across many seeds. Ignored
//! by default (slow); run with `--ignored --nocapture` in release to
//! measure whether seat decides mirror outcomes.

use oxide_sim::bot::{Bot, Brain, Dials};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, Scenario};

fn classic_mirror(seed: u64) -> Option<PlayerId> {
    let mut scenario = Scenario::skirmish();
    scenario.seed = seed;
    scenario.players[0].bot = true;
    scenario.players[1].bot = true;
    let mut state = scenario.build().unwrap();
    let mut bots = Bot::for_scenario(&scenario);
    for _ in 0..40_000u32 {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        state.tick(&commands);
        if let Some(GameResult::Victory { team }) = state.result() {
            return Some(PlayerId(team));
        }
    }
    None
}

fn brain_mirror(seed: u64) -> Option<PlayerId> {
    let mut scenario = Scenario::skirmish();
    scenario.seed = seed;
    let mut state = scenario.build().unwrap();
    let mut b0 = Brain::new(PlayerId(0), seed, Dials::full_omniscient());
    let mut b1 = Brain::new(PlayerId(1), seed, Dials::full_omniscient());
    for _ in 0..60_000u32 {
        let mut commands = b0.act(&state);
        commands.extend(b1.act(&state));
        state.tick(&commands);
        if let Some(GameResult::Victory { team }) = state.result() {
            return Some(PlayerId(team));
        }
    }
    None
}

#[test]
#[ignore]
fn measure_seat_bias() {
    let mut classic = [0u32; 3]; // p0 wins, p1 wins, draws
    let mut brain = [0u32; 3];
    for seed in 1..=20u64 {
        match classic_mirror(seed) {
            Some(PlayerId(0)) => classic[0] += 1,
            Some(_) => classic[1] += 1,
            None => classic[2] += 1,
        }
        match brain_mirror(seed) {
            Some(PlayerId(0)) => brain[0] += 1,
            Some(_) => brain[1] += 1,
            None => brain[2] += 1,
        }
        println!("seed {seed}: classic {classic:?} brain {brain:?}");
    }
    println!("FINAL classic p0/p1/draw {classic:?}");
    println!("FINAL brain   p0/p1/draw {brain:?}");
}
