//! Utility-policy contracts: deterministic thinking and budget honesty.

use oxide_sim::bot::{Dials, Intent, Observation, UtilityPolicy};
use oxide_sim::{PlayerId, Scenario};

#[test]
fn identical_inputs_think_identical_intents() {
    let scenario = Scenario::skirmish();
    let state = scenario.build().unwrap();
    let obs = Observation::omniscient(&state, PlayerId(0));
    let dials = Dials::full();
    let mut first = UtilityPolicy::new();
    let mut second = UtilityPolicy::new();
    assert_eq!(
        first.think(&dials, &obs, &[], &[]),
        second.think(&dials, &obs, &[], &[]),
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
        let mut policy = UtilityPolicy::new();
        let intents = policy.think(&Dials::full(), &obs, &[], &[]);
        let planned: u32 = intents
            .iter()
            .map(|i| match i {
                Intent::TrainAt { kind, .. } => kind.stats().cost,
                Intent::Build { kind, .. } => kind.base_stats().construction.map_or(0, |c| c.cost),
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

#[test]
fn a_starved_commander_liquidates_its_walls_for_one_more_wave() {
    // The scripted salvage doctrine: bank starved, nothing known left
    // to mine or strip, a standing defense — the policy sells
    // cheapest-first. The learner meets the mechanic from the
    // receiving side of exactly this intent.
    use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
    use oxide_sim::stats::BuildingKind;
    let scenario = Scenario {
        name: "starved".into(),
        seed: 5,
        map: vec![
            "################".into(),
            "#..............#".into(),
            "#..1...........#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "F".into(),
                faction: oxide_sim::Faction::Ferrous,
                team: None,
                scrap: 3,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "C".into(),
                faction: oxide_sim::Faction::Cupric,
                team: None,
                scrap: 3,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![UnitSpec {
            player: 0,
            kind: oxide_sim::UnitKind::Harvester,
            x: 6,
            y: 2,
        }],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 8,
                y: 2,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Bastion,
                x: 10,
                y: 2,
            },
        ],
        meta: None,
    };
    let state = scenario.build().unwrap();
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::full(), &obs, &[], &[]);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, Intent::Salvage { building } if *building == turret)),
        "starved with dry ground: the cheapest wall goes first, got {intents:?}"
    );
}
