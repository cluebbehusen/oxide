//! Gym-interface contracts: a scripted action sequence reproduces
//! bit-identically (training rollouts must be replayable), and the
//! masked menu is honest enough to play a real game through.

use chassis::rng::Pcg32;
use oxide_sim::bot::{Action, Brain, Difficulty, GymBot};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, Scenario};

/// Drives a full match: gym bot in seat 0 picks actions with a seeded
/// rng over the legal mask; a scripted tier drives seat 1. Returns the
/// final state hash and the result.
fn scripted_match(seed: u64) -> (u64, Option<GameResult>) {
    let mut scenario = Scenario::skirmish();
    scenario.seed = seed;
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let mut opponent = Brain::for_tier(PlayerId(1), seed, Difficulty::Standard);
    let mut rng = Pcg32::new(seed, 7777);
    for tick in 0..30_000u64 {
        let mut commands = Vec::new();
        if tick % gym.cadence() == 0 && state.result().is_none() {
            let decision = gym.decision(&state);
            let legal: Vec<usize> = decision
                .mask
                .iter()
                .enumerate()
                .filter(|(_, ok)| **ok)
                .map(|(i, _)| i)
                .collect();
            let pick = legal[rng.next_below(legal.len() as u32) as usize];
            commands.extend(gym.step(&state, Action::from_index(pick)));
        }
        commands.extend(opponent.act(&state));
        state.tick(&commands);
        if state.result().is_some() {
            break;
        }
    }
    (state.hash(), state.result())
}

#[test]
fn gym_rollouts_reproduce_bit_identically() {
    let (a_hash, a_result) = scripted_match(11);
    let (b_hash, b_result) = scripted_match(11);
    assert_eq!(a_hash, b_hash, "same seed + same actions ⇒ same world");
    assert_eq!(a_result, b_result);
}

#[test]
fn the_mask_supports_playing_an_actual_game() {
    // A tiny hand-rolled policy over the gym menu: keep the economy at
    // four, drip sentinels, form an army, push when it stands. It must
    // function — units get built, an army forms, the match ends or at
    // minimum a real army exists by the cap.
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let mut opponent = Brain::for_tier(PlayerId(1), scenario.seed, Difficulty::Scrapheap);
    let mut formed = false;
    for tick in 0..30_000u64 {
        let mut commands = Vec::new();
        if tick % gym.cadence() == 0 && state.result().is_none() {
            let d = gym.decision(&state);
            let harvesters = d.features[2];
            let staging_size = d.features[11];
            let want = if harvesters < 4 && d.mask[Action::TrainHarvester as usize] {
                Action::TrainHarvester
            } else if d.mask[Action::Push as usize] && staging_size >= 5 {
                Action::Push
            } else if d.mask[Action::FormArmy as usize] {
                Action::FormArmy
            } else if d.mask[Action::TrainSentinel as usize] {
                Action::TrainSentinel
            } else if d.mask[Action::Scout as usize] && tick % 1024 == 0 {
                Action::Scout
            } else {
                Action::Idle
            };
            formed |= staging_size > 0;
            commands.extend(gym.step(&state, want));
        }
        commands.extend(opponent.act(&state));
        state.tick(&commands);
        if let Some(GameResult::Victory { team }) = state.result() {
            assert_eq!(
                PlayerId(team),
                PlayerId(0),
                "the scripted gym line should beat Scrapheap"
            );
            assert!(formed, "it should have fought with a formed army");
            return;
        }
    }
    panic!("no decision against Scrapheap within the cap");
}
