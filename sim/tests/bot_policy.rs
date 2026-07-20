//! Utility-policy contracts: deterministic thinking and budget honesty.

use oxide_sim::bot::{Dials, Executive, Intent, Observation, UtilityPolicy};
use oxide_sim::{PlayerId, Scenario};

#[test]
fn identical_inputs_think_identical_intents() {
    let scenario = Scenario::skirmish();
    let state = scenario.build().unwrap();
    let obs = Observation::omniscient(&state, PlayerId(0));
    let exec = Executive::new();
    let dials = Dials::full_omniscient();
    let mut first = UtilityPolicy::new();
    let mut second = UtilityPolicy::new();
    assert_eq!(
        first.think(&dials, &obs, &exec),
        second.think(&dials, &obs, &exec),
        "a policy is a function of (dials, observation, executive)"
    );
}

#[test]
fn a_think_never_plans_past_the_bank() {
    let scenario = Scenario::skirmish();
    let state = scenario.build().unwrap();
    for player in [0u8, 1] {
        let me = PlayerId(player);
        let obs = Observation::omniscient(&state, me);
        let exec = Executive::new();
        let mut policy = UtilityPolicy::new();
        let intents = policy.think(&Dials::full_omniscient(), &obs, &exec);
        let planned: u32 = intents
            .iter()
            .map(|i| match i {
                Intent::TrainAt { kind, .. } => kind.stats().cost,
                Intent::Build { kind, .. } => kind.stats().construction.map_or(0, |c| c.cost),
                _ => 0,
            })
            .sum();
        assert!(
            planned <= obs.scrap,
            "priced intents ({planned}) exceed the bank ({})",
            obs.scrap
        );
    }
}
