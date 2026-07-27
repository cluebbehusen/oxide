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

#[test]
fn salvage_masks_honestly_and_lowers_cheapest_first() {
    // v5's new verb: masked off with nothing eligible, on when an
    // eligible defense stands, lowering to the cheapest-and-least-
    // useful pick — and never the Fabricator or Foundry.
    use oxide_sim::scenario::BuildingSpec;
    use oxide_sim::stats::BuildingKind;
    let mut scenario = Scenario::skirmish();
    let mut gym = GymBot::new(PlayerId(0));
    let state = scenario.build().unwrap();
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::Salvage as usize],
        "nothing to strip at match start (a Foundry never counts)"
    );

    // Stand a turret and a bastion; the pick must be the turret.
    for (kind, x) in [(BuildingKind::Bastion, 9), (BuildingKind::Turret, 16)] {
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind,
            x,
            y: 3,
        });
    }
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        d.mask[Action::Salvage as usize],
        "a standing defense arms it"
    );
    let my_building_value = d.features[63];
    let expected = BuildingKind::Turret.stats().construction.unwrap().cost
        + BuildingKind::Bastion.stats().construction.unwrap().cost;
    assert_eq!(
        my_building_value,
        i64::from(expected),
        "the v5 feature prices the standing stock"
    );
    let commands = gym.step(&state, Action::Salvage);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    assert!(
        commands.iter().any(|c| matches!(
            &c.command,
            oxide_sim::Command::Salvage { building, .. } if *building == turret
        )),
        "cheapest-first: the turret goes before the bastion: {commands:?}"
    );
    // And the sim accepts what the lowering emitted.
    let report = state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. })),
        "the lowered command validates: {:?}",
        report.events
    );
}

#[test]
fn the_repair_channel_leaves_salvage_targets_alone() {
    // The two verbs must never share a target: with the only wounded
    // building under an own crew's salvage, Repair masks off.
    use oxide_sim::scenario::BuildingSpec;
    use oxide_sim::stats::BuildingKind;
    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 9,
        y: 6,
    });
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::Salvage);
    state.tick(&commands);
    // Let the crew walk over and leave first scars.
    for _ in 0..300 {
        state.tick(&[]);
        let turret = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Turret);
        if turret.is_some_and(|b| b.hp < b.kind.stats().max_hp) {
            break;
        }
    }
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .expect("still standing");
    assert!(
        turret.hp < turret.kind.stats().max_hp,
        "test premise: the strip left a wound repair would otherwise take"
    );
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::Repair as usize],
        "a building under own salvage is not a patient"
    );
}
