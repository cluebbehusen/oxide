//! Gym-interface contracts: a scripted action sequence reproduces
//! bit-identically (training rollouts must be replayable), and the
//! masked menu is honest enough to play a real game through.

use chassis::rng::Pcg32;
use oxide_sim::bot::{
    ACTION_HEADS, Action, ActionPlan, Brain, CONSTRUCTION_PLAN_TIMEOUT_TICKS, Difficulty,
    FEATURE_NAMES, GymBot, Level, NeuralBot,
};
use oxide_sim::state::GameResult;
use oxide_sim::{BuildingKind, Command, PlayerId, Scenario, UnitKind};

#[test]
fn action_plan_decoding_is_head_safe() {
    assert_eq!(
        ActionPlan::from_indices([
            Action::TrainSentinel as usize,
            Action::BuildArray as usize,
            Action::Push as usize,
        ]),
        ActionPlan {
            production: Action::TrainSentinel,
            construction: Action::BuildArray,
            operation: Action::Push,
        }
    );
    assert_eq!(
        ActionPlan::from_indices([
            Action::BuildTurret as usize,
            Action::TrainLancer as usize,
            usize::MAX,
        ]),
        ActionPlan::default(),
        "wrong-head and out-of-range values fold independently"
    );
    assert_eq!(
        ActionPlan::default().indices(),
        [
            Action::Idle as usize,
            Action::NoConstruction as usize,
            Action::NoOperation as usize,
        ]
    );
}

#[test]
fn production_intentions_are_visible_before_affordability() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 0;
    let state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let decision = gym.decision(&state);
    assert!(
        decision.mask[Action::TrainSentinel as usize],
        "an open Foundry exposes the intention while the bank is empty"
    );
    let commands = gym.step_plan(
        &state,
        ActionPlan {
            production: Action::TrainSentinel,
            ..ActionPlan::default()
        },
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command.command, Command::Train { .. })),
        "lowering waits instead of emitting an unaffordable command"
    );
}

#[test]
fn construction_plans_reserve_scrap_and_lower_before_production() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 100;
    let low_state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let decision = gym.decision(&low_state);
    assert!(
        decision.mask[Action::BuildArray as usize],
        "a feasible build can be selected before its 120-scrap price is banked"
    );
    let commands = gym.step_plan(
        &low_state,
        ActionPlan {
            production: Action::TrainHarvester,
            construction: Action::BuildArray,
            operation: Action::NoOperation,
        },
    );
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::Build { .. } | Command::Train { .. }
        )),
        "the saved 100 scrap cannot leak into a cheaper unit"
    );
    let decision = gym.decision(&low_state);
    let feature = |name: &str| {
        let index = FEATURE_NAMES
            .iter()
            .position(|entry| *entry == name)
            .unwrap();
        decision.features[index]
    };
    assert_eq!(feature("construction_plan"), 5);
    assert_eq!(feature("construction_reserve"), 120);

    scenario.players[0].scrap =
        BuildingKind::Array.stats().construction.unwrap().cost + UnitKind::Harvester.stats().cost;
    let funded_state = scenario.build().unwrap();
    let commands = gym.step_plan(
        &funded_state,
        ActionPlan {
            production: Action::TrainHarvester,
            ..ActionPlan::default()
        },
    );
    let build = commands
        .iter()
        .position(|command| {
            matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Array,
                    ..
                }
            )
        })
        .expect("the preserved plan founds its Array");
    let train = commands
        .iter()
        .position(|command| {
            matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Harvester,
                    ..
                }
            )
        })
        .expect("the bank above the reserve buys the unit");
    assert!(build < train, "construction owns the shared budget first");
}

#[test]
fn selected_maintenance_defers_an_affordable_saved_build() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 100;
    let low_state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    gym.step(&low_state, Action::BuildArray);

    scenario.players[0].scrap = BuildingKind::Array.stats().construction.unwrap().cost + 20;
    let funded_state = scenario.build().unwrap();
    let mut value = serde_json::to_value(funded_state).unwrap();
    let patient = value["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|unit| unit["player"] == 0 && unit["kind"] == "harvester")
        .and_then(|unit| unit["id"].as_u64())
        .map(|id| oxide_sim::UnitId(id as u32))
        .unwrap();
    value["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|unit| unit["id"] == patient.0)
        .unwrap()["hp"] = 20.into();
    let funded_state: oxide_sim::State = serde_json::from_value(value).unwrap();

    let decision = gym.decision(&funded_state);
    assert!(
        decision.mask[Action::RepairUnit as usize],
        "the bank above the saved Array reserve can fund a weld"
    );
    let commands = gym.step_plan(
        &funded_state,
        ActionPlan {
            construction: Action::RepairUnit,
            ..ActionPlan::default()
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == patient
        )),
        "the sampled maintenance verb must reach lowering: {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Array,
                ..
            }
        )),
        "the affordable saved Array waits one think instead of relabeling the weld"
    );
}

#[test]
fn stale_unfunded_plans_cancel_and_release_the_economy() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 100;
    let state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    gym.step(&state, Action::BuildArray);
    let reserved = gym.decision(&state);
    let feature = |decision: &oxide_sim::bot::Decision, name: &str| {
        let index = FEATURE_NAMES
            .iter()
            .position(|entry| *entry == name)
            .unwrap();
        decision.features[index]
    };
    assert_eq!(feature(&reserved, "construction_plan"), 5);
    assert_eq!(feature(&reserved, "construction_reserve"), 120);

    let mut value = serde_json::to_value(&state).unwrap();
    value["tick"] = CONSTRUCTION_PLAN_TIMEOUT_TICKS.into();
    let expired: oxide_sim::State = serde_json::from_value(value).unwrap();
    let decision = gym.decision(&expired);
    assert_eq!(feature(&decision, "construction_plan"), 0);
    assert_eq!(feature(&decision, "construction_reserve"), 0);
    assert!(
        !decision.mask[Action::BuildArray as usize],
        "the timed-out plan cannot immediately re-arm and freeze the same bank"
    );
    let commands = gym.step_plan(
        &expired,
        ActionPlan {
            production: Action::TrainHarvester,
            ..ActionPlan::default()
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "expiry releases the formerly reserved 100 scrap"
    );
}

#[test]
fn a_saved_construction_plan_cannot_be_kept_young_by_switching_kinds() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 100;
    let state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    gym.step(&state, Action::BuildArray);

    let feature = |decision: &oxide_sim::bot::Decision, name: &str| {
        let index = FEATURE_NAMES
            .iter()
            .position(|entry| *entry == name)
            .unwrap();
        decision.features[index]
    };
    for (tick, action, expected_plan) in [
        (300, Action::BuildReclaimer, 6),
        (600, Action::BuildArray, 5),
        (900, Action::BuildReclaimer, 6),
    ] {
        let mut value = serde_json::to_value(&state).unwrap();
        value["tick"] = tick.into();
        let later: oxide_sim::State = serde_json::from_value(value).unwrap();
        let decision = gym.decision(&later);
        assert!(
            decision.mask[action as usize],
            "a policy may revise the kind without refreshing the plan's age"
        );
        gym.step(&later, action);
        assert_eq!(
            feature(&gym.decision(&later), "construction_plan"),
            expected_plan
        );
    }

    let mut value = serde_json::to_value(&state).unwrap();
    value["tick"] = CONSTRUCTION_PLAN_TIMEOUT_TICKS.into();
    let expired: oxide_sim::State = serde_json::from_value(value).unwrap();
    let decision = gym.decision(&expired);
    assert_eq!(feature(&decision, "construction_plan"), 0);
    assert_eq!(feature(&decision, "construction_reserve"), 0);
    for action in [Action::BuildArray, Action::BuildReclaimer] {
        assert!(
            !decision.mask[action as usize],
            "expiry gives production a global spending window before any capital plan re-arms"
        );
    }
    let commands = gym.step_plan(
        &expired,
        ActionPlan {
            production: Action::TrainHarvester,
            construction: Action::BuildArray,
            operation: Action::NoOperation,
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "even an immediate different build choice cannot reclaim the released bank"
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command.command, Command::Build { .. })),
        "all construction remains masked during the global retry window"
    );
}

fn state_with_far_remembered_scrap(bank: u32) -> oxide_sim::State {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = bank;
    scenario.map = scenario
        .map
        .iter()
        .map(|row| row.replace(['s', 'S'], "."))
        .collect();
    let state = scenario.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    let vision = &mut value["vision"][0];
    for visible in vision["visible"]["cells"].as_array_mut().unwrap() {
        *visible = false.into();
    }
    for explored in vision["explored"]["cells"].as_array_mut().unwrap() {
        *explored = true.into();
    }
    let width = scenario.map[0].len();
    let far = 12 * width + 20;
    vision["remembered_scrap"]["cells"][far] = 400.into();
    serde_json::from_value(value).unwrap()
}

#[test]
fn unpaid_founding_claims_reserve_only_unspent_capital_and_expire() {
    let mut state = state_with_far_remembered_scrap(150);
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::BuildTurret);
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Turret,
                defer: true,
                ..
            }
        )),
        "remembered far ground must create an unpaid walking claim: {commands:?}"
    );
    let report = state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. })),
        "the deferred claim must be accepted: {:?}",
        report.events
    );
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        150,
        "deferred founding has not paid for a site yet"
    );
    let founder = state
        .units()
        .iter()
        .find(|unit| matches!(unit.order, oxide_sim::state::Order::Found { .. }))
        .map(|unit| unit.id)
        .expect("one harvester is walking the claim");

    let decision = gym.decision(&state);
    let feature = |decision: &oxide_sim::bot::Decision, name: &str| {
        let index = FEATURE_NAMES
            .iter()
            .position(|entry| *entry == name)
            .unwrap();
        decision.features[index]
    };
    assert_eq!(
        feature(&decision, "construction_site_value"),
        0,
        "unpaid intent is not paid construction potential"
    );
    assert_eq!(feature(&decision, "my_construction_sites"), 1);
    assert_eq!(feature(&decision, "construction_plan"), 0);
    assert_eq!(
        feature(&decision, "construction_reserve"),
        i64::from(BuildingKind::Turret.stats().construction.unwrap().cost)
    );
    assert!(
        !decision.mask[Action::BuildReclaimer as usize],
        "a second deferred structure cannot overcommit the same bank"
    );

    let commands = gym.step_plan(
        &state,
        ActionPlan {
            production: Action::TrainHarvester,
            construction: Action::BuildReclaimer,
            operation: Action::NoOperation,
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "the 50 scrap above the 100-scrap claim remains spendable"
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command.command, Command::Build { .. })),
        "direct lowering cannot bypass the one-unpaid-claim guard"
    );

    let mut value = serde_json::to_value(&state).unwrap();
    value["tick"] = (state.current_tick() + CONSTRUCTION_PLAN_TIMEOUT_TICKS).into();
    let expired: oxide_sim::State = serde_json::from_value(value).unwrap();
    let decision = gym.decision(&expired);
    assert_eq!(feature(&decision, "construction_site_value"), 0);
    assert_eq!(
        feature(&decision, "construction_reserve"),
        0,
        "a stale unpaid claim releases its capital"
    );
    assert!(
        !decision.mask[Action::BuildReclaimer as usize],
        "a stale walking claim opens a global spending window, not another capital claim"
    );
    let commands = gym.step_plan(
        &expired,
        ActionPlan {
            production: Action::TrainHarvester,
            construction: Action::BuildReclaimer,
            operation: Action::NoOperation,
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            &command.command,
            Command::Stop { units } if units.contains(&founder)
        )),
        "expiry deterministically cancels the walking founder"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "the released claim can fund production before another structure"
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command.command, Command::Build { .. })),
        "a different structure cannot immediately replace the stale claim"
    );
}

#[test]
fn every_committed_build_cap_masks_redundant_structures() {
    use oxide_sim::scenario::BuildingSpec;

    for (action, kind, cap) in [
        (Action::BuildFabricator, BuildingKind::Fabricator, 1),
        (Action::BuildTurret, BuildingKind::Turret, 2),
        (Action::BuildFlak, BuildingKind::FlakTurret, 2),
        (Action::BuildBastion, BuildingKind::Bastion, 2),
        (Action::BuildArray, BuildingKind::Array, 1),
        (Action::BuildReclaimer, BuildingKind::Reclaimer, 2),
        (Action::BuildRepairBay, BuildingKind::RepairBay, 1),
    ] {
        let mut scenario = Scenario::skirmish();
        for index in 0..cap {
            scenario.buildings.push(BuildingSpec {
                player: 0,
                kind,
                x: 11 + index * 4,
                y: 3,
            });
        }
        let state = scenario
            .build()
            .unwrap_or_else(|error| panic!("fixture for {kind:?}: {error}"));
        let mut gym = GymBot::new(PlayerId(0));
        assert!(
            !gym.decision(&state).mask[action as usize],
            "{kind:?} must mask at its committed cap"
        );
        assert!(
            !gym.step(&state, action).iter().any(|command| matches!(
                command.command,
                Command::Build {
                    kind: emitted,
                    ..
                } if emitted == kind
            )),
            "direct compatibility lowering cannot bypass the {kind:?} cap"
        );
    }
}

#[test]
fn fabricator_waits_for_an_economy_and_home_screen() {
    use oxide_sim::scenario::UnitSpec;

    let mut scenario = Scenario::skirmish();
    let mut gym = GymBot::new(PlayerId(0));
    assert!(
        !gym.decision(&scenario.build().unwrap()).mask[Action::BuildFabricator as usize],
        "the three-worker, one-Sentinel opening is not ready to spend its defense"
    );

    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 8,
        y: 8,
    });
    for (x, y) in [(9, 8), (10, 8), (11, 8), (12, 8)] {
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x,
            y,
        });
    }
    assert!(
        gym.decision(&scenario.build().unwrap()).mask[Action::BuildFabricator as usize],
        "four workers and five Sentinels make the capital plan viable"
    );
}

#[test]
fn a_visible_home_intruder_forces_a_local_defense_order() {
    use oxide_sim::scenario::BuildingSpec;

    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 9,
        y: 3,
    });
    let intruder = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
        .unwrap();
    (intruder.x, intruder.y) = (16, 5);
    let intruder_tile = chassis::grid::TilePos::new(intruder.x, intruder.y);
    let state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));

    let decision = gym.decision(&state);
    let legal_operations: Vec<_> = oxide_sim::bot::OPERATION_ACTIONS
        .iter()
        .copied()
        .filter(|action| decision.mask[*action])
        .collect();
    assert_eq!(
        legal_operations,
        vec![Action::FormArmy as usize],
        "idle defenders form instead of scouting or doing nothing"
    );
    gym.step(&state, Action::FormArmy);

    let decision = gym.decision(&state);
    let legal_operations: Vec<_> = oxide_sim::bot::OPERATION_ACTIONS
        .iter()
        .copied()
        .filter(|action| decision.mask[*action])
        .collect();
    assert_eq!(
        legal_operations,
        vec![Action::Push as usize],
        "the staged body has one coherent response"
    );
    let commands = gym.step(&state, Action::Push);
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::AttackMove { goal, .. } if goal == intruder_tile
        )),
        "the push intercepts the intruder instead of marching past it toward the enemy base"
    );
}

#[test]
fn home_defense_does_not_lose_its_only_fighter_to_field_repair() {
    use oxide_sim::scenario::BuildingSpec;

    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 9,
        y: 3,
    });
    let intruder = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
        .unwrap();
    (intruder.x, intruder.y) = (16, 5);
    let state = scenario.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    let patient = value["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|unit| unit["player"] == 0 && unit["kind"] == "sentinel")
        .and_then(|unit| unit["id"].as_u64())
        .map(|id| oxide_sim::UnitId(id as u32))
        .unwrap();
    value["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|unit| unit["id"] == patient.0)
        .unwrap()["hp"] = 20.into();
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();
    let mut gym = GymBot::new(PlayerId(0));

    let decision = gym.decision(&state);
    assert!(
        !decision.mask[Action::RepairUnit as usize],
        "an intruder makes the wounded defender unavailable for maintenance"
    );
    let commands = gym.step_plan(
        &state,
        ActionPlan {
            production: Action::Idle,
            construction: Action::RepairUnit,
            operation: Action::FormArmy,
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            &command.command,
            Command::AttackMove { units, .. } if units.contains(&patient)
        )),
        "the only fighter must answer the intruder: {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == patient
        )),
        "out-of-contract maintenance lowering cannot steal the defender"
    );
}

#[test]
fn push_masks_only_for_a_staging_army() {
    use oxide_sim::scenario::BuildingSpec;

    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 9,
        y: 3,
    });
    let enemy = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
        .unwrap();
    // Visible through the Array, but one tile outside the immediate
    // home-defense radius. These lifecycle tests exercise ordinary
    // Push/Recall coexistence; the intruder case must defer welding.
    (enemy.x, enemy.y) = (17, 5);
    let state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    gym.step(&state, Action::FormArmy);
    assert!(
        gym.decision(&state).mask[Action::Push as usize],
        "a staged fighter may be committed"
    );
    gym.step(&state, Action::Push);
    let decision = gym.decision(&state);
    assert!(
        !decision.mask[Action::Push as usize],
        "Push is not a repeated order while the army is already out"
    );
    assert!(
        decision.mask[Action::Recall as usize],
        "Recall is the explicit transition back"
    );
}

#[test]
fn strategic_features_price_fog_honest_resources_commitments_and_health() {
    use oxide_sim::scenario::{BuildingSpec, UnitSpec};

    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 0;
    scenario.buildings.extend([
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 12,
            y: 3,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::RepairBay,
            x: 16,
            y: 3,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 7,
            y: 9,
        },
    ]);
    scenario.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 11,
        y: 5,
    });
    let state = scenario.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    let units = value["units"].as_array_mut().unwrap();
    let harvester = units
        .iter_mut()
        .find(|unit| unit["player"] == 0 && unit["kind"] == "harvester")
        .unwrap();
    harvester["carrying"] = 7.into();
    let sentinel = units
        .iter_mut()
        .find(|unit| unit["player"] == 0 && unit["kind"] == "sentinel")
        .unwrap();
    sentinel["hp"] = (UnitKind::Sentinel.stats().max_hp / 2).into();
    let buildings = value["buildings"].as_array_mut().unwrap();
    let foundry = buildings
        .iter_mut()
        .find(|building| building["player"] == 0 && building["kind"] == "foundry")
        .unwrap();
    foundry["queue"] = serde_json::json!(["sentinel"]);
    let bay = buildings
        .iter_mut()
        .find(|building| building["player"] == 0 && building["kind"] == "repair_bay")
        .unwrap();
    bay["hp"] = (BuildingKind::RepairBay.stats().max_hp / 2).into();
    let array = buildings
        .iter_mut()
        .find(|building| building["player"] == 0 && building["kind"] == "array")
        .unwrap();
    array["built"] = false.into();
    array["hp"] = (BuildingKind::Array.stats().max_hp / 5).into();
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();

    let mut gym = GymBot::new(PlayerId(0));
    let decision = gym.decision(&state);
    let feature = |name: &str| {
        let index = FEATURE_NAMES
            .iter()
            .position(|entry| *entry == name)
            .unwrap();
        decision.features[index]
    };
    assert_eq!(feature("known_salvage_value"), 1600);
    assert_eq!(feature("near_home_salvage_value"), 1600);
    assert_eq!(feature("nearest_salvage_distance"), 4);
    assert_eq!(feature("idle_harvesters"), 3);
    assert_eq!(feature("carried_scrap"), 7);
    assert_eq!(
        feature("queued_unit_value"),
        i64::from(UnitKind::Sentinel.stats().cost)
    );
    assert_eq!(feature("construction_site_value"), 120);
    let unit_health = 3 * i64::from(UnitKind::Harvester.stats().cost)
        + i64::from(UnitKind::Sentinel.stats().cost) / 2;
    assert_eq!(feature("my_unit_health_value"), unit_health);
    assert_eq!(
        feature("my_building_health_value"),
        i64::from(BuildingKind::Bastion.stats().construction.unwrap().cost)
            + i64::from(BuildingKind::RepairBay.stats().construction.unwrap().cost / 2)
    );
    assert_eq!(feature("my_bastions_built"), 1);
    assert_eq!(feature("my_repair_bays_built"), 1);
    assert_eq!(feature("my_construction_sites"), 1);
    assert!(feature("home_enemy_pressure") > 0);
    assert_eq!(feature("nearest_enemy_distance"), 8);
    assert_eq!(feature("construction_plan"), 0);
    assert_eq!(feature("construction_reserve"), 0);

    gym.step(&state, Action::BuildReclaimer);
    let planned = gym.decision(&state);
    let feature = |name: &str| {
        let index = FEATURE_NAMES
            .iter()
            .position(|entry| *entry == name)
            .unwrap();
        planned.features[index]
    };
    assert_eq!(feature("construction_plan"), 6);
    assert_eq!(feature("construction_reserve"), 150);
}

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
            let mut picks = ActionPlan::default().indices();
            for (head_index, head) in ACTION_HEADS.iter().enumerate() {
                let legal: Vec<usize> = head
                    .iter()
                    .copied()
                    .filter(|action| decision.mask[*action])
                    .collect();
                picks[head_index] = legal[rng.next_below(legal.len() as u32) as usize];
            }
            commands.extend(gym.step_plan(&state, ActionPlan::from_indices(picks)));
        }
        commands.extend(opponent.act(&state));
        state.tick(&commands);
        if state.result().is_some() {
            break;
        }
    }
    (state.hash(), state.result())
}

fn stranded_scenario(scrap: u32) -> Scenario {
    use oxide_sim::BuildingKind;
    use oxide_sim::scenario::BuildingSpec;

    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = scrap;
    scenario
        .units
        .retain(|u| u.player != 0 || u.kind != UnitKind::Harvester);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 9,
        y: 3,
    });
    scenario
}

#[test]
fn recovery_reserves_partial_scrap_and_overrides_a_wrong_macro_action() {
    let state = stranded_scenario(UnitKind::Scuttler.stats().cost)
        .build()
        .unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let decision = gym.decision(&state);
    let legal: Vec<usize> = decision
        .mask
        .iter()
        .enumerate()
        .filter_map(|(index, legal)| legal.then_some(index))
        .collect();
    assert_eq!(
        legal,
        vec![
            Action::Idle as usize,
            Action::NoConstruction as usize,
            Action::NoOperation as usize,
        ],
        "the Scuttler-priced partial reserve must not be spendable"
    );
    assert!(
        gym.step(&state, Action::TrainScuttler).is_empty(),
        "an out-of-contract action cannot bypass the reserve"
    );

    let state = stranded_scenario(UnitKind::Harvester.stats().cost)
        .build()
        .unwrap();
    let decision = gym.decision(&state);
    let legal: Vec<usize> = decision
        .mask
        .iter()
        .enumerate()
        .filter_map(|(index, legal)| legal.then_some(index))
        .collect();
    assert_eq!(
        legal,
        vec![
            Action::TrainHarvester as usize,
            Action::NoConstruction as usize,
            Action::NoOperation as usize,
        ],
        "the completed reserve has exactly one legal use"
    );
    let commands = gym.step(&state, Action::TrainScuttler);
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));
    assert!(!commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Scuttler,
            ..
        }
    )));
}

#[test]
fn scripted_and_neural_commanders_take_the_recovery_harvester() {
    let scenario = stranded_scenario(UnitKind::Harvester.stats().cost);
    let state = scenario.build().unwrap();
    let assert_recovery = |commands: &[oxide_sim::PlayerCommand]| {
        assert!(commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )));
        assert!(!commands.iter().any(|command| matches!(
            command.command,
            Command::Train { kind, .. } if kind != UnitKind::Harvester
        )));
    };

    let mut scripted = Brain::for_tier(PlayerId(0), scenario.seed, Difficulty::Prime);
    assert_recovery(&scripted.act(&state));

    let mut neural = NeuralBot::ladder(
        PlayerId(0),
        scenario.seed,
        Level::Expert,
        Some(500),
        scenario.players[0].faction,
    );
    assert_recovery(&neural.act(&state));
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
            let production = if harvesters < 4 && d.mask[Action::TrainHarvester as usize] {
                Action::TrainHarvester
            } else if d.mask[Action::TrainSentinel as usize] {
                Action::TrainSentinel
            } else {
                Action::Idle
            };
            let operation = if d.mask[Action::Push as usize] && staging_size >= 5 {
                Action::Push
            } else if d.mask[Action::FormArmy as usize] {
                Action::FormArmy
            } else if d.mask[Action::Scout as usize] && tick % 1024 == 0 {
                Action::Scout
            } else {
                Action::NoOperation
            };
            formed |= staging_size > 0;
            commands.extend(gym.step_plan(
                &state,
                ActionPlan {
                    production,
                    construction: Action::NoConstruction,
                    operation,
                },
            ));
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
    // The emergency verb stays off while resources remain, then
    // liquidates the cheapest eligible defense after the economy is
    // genuinely exhausted.
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
    let funded = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    assert!(
        !gym.decision(&funded).mask[Action::Salvage as usize],
        "standing defenses are not sold while bank or field salvage remains"
    );

    scenario.players[0].scrap = 0;
    scenario.map = scenario
        .map
        .iter()
        .map(|row| row.replace(['s', 'S'], "."))
        .collect();
    let mut state = scenario.build().unwrap();
    let d = gym.decision(&state);
    assert!(
        d.mask[Action::Salvage as usize],
        "an exhausted economy may liquidate static defense"
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

/// A long corridor for the v6 weld verb: p0's foundry west, p1's east,
/// far enough apart that a welder can stand outside the patient leash.
fn weld_arena(units: Vec<oxide_sim::scenario::UnitSpec>) -> Scenario {
    let mut map = vec![format!("#{}#", ".".repeat(38)); 9];
    map[0] = "#".repeat(40);
    map[8] = "#".repeat(40);
    map[1] = format!("#1{}#", ".".repeat(37));
    map[6] = format!("#{}2{}#", ".".repeat(35), ".".repeat(2));
    Scenario {
        name: "weld-corridor".into(),
        seed: 42,
        map,
        players: (0..2)
            .map(|i| oxide_sim::scenario::PlayerSpec {
                name: ["Ferrous", "Cupric"][i].into(),
                faction: [oxide_sim::Faction::Ferrous, oxide_sim::Faction::Cupric][i],
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn spec(player: u8, kind: oxide_sim::UnitKind, x: i32, y: i32) -> oxide_sim::scenario::UnitSpec {
    oxide_sim::scenario::UnitSpec { player, kind, x, y }
}

/// Chews the patient down below `floor` hp with the raider, then walks
/// the raider home so the wound sits still for the decision under test.
fn wound(
    state: &mut oxide_sim::State,
    patient: oxide_sim::UnitId,
    raider: oxide_sim::UnitId,
    floor: u32,
) {
    use chassis::grid::TilePos;
    let orders = |units: Vec<oxide_sim::UnitId>, goal: TilePos| {
        vec![oxide_sim::PlayerCommand {
            player: PlayerId(1),
            command: oxide_sim::Command::Move {
                units,
                goal,
                queue: false,
            },
        }]
    };
    state.tick(&orders(vec![raider], TilePos::new(6, 2)));
    for _ in 0..2_000 {
        state.tick(&[]);
        if state.unit(patient).unwrap().hp <= floor {
            break;
        }
    }
    state.tick(&orders(vec![raider], TilePos::new(34, 4)));
    for _ in 0..2_000 {
        state.tick(&[]);
        if state.unit(raider).unwrap().tile() == TilePos::new(34, 4) {
            break;
        }
    }
    let hp = state.unit(patient).unwrap().hp;
    let max = state.unit(patient).unwrap().kind.stats().max_hp;
    assert!(hp > 0 && hp < max, "test premise: a live, weldable wound");
}

#[test]
fn repair_unit_masks_on_the_leash_and_welds_the_wound() {
    use chassis::grid::TilePos;
    use oxide_sim::UnitKind;
    // Patient near home, welder parked far beyond the leash.
    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2),  // patient
        spec(0, UnitKind::Harvester, 28, 2), // welder, out on the corridor
        spec(0, UnitKind::Harvester, 28, 4), // economy worker
        spec(1, UnitKind::Scuttler, 34, 4),  // raider
    ]);
    let mut state = scenario.build().unwrap();
    let (patient, welder, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[3].id,
    );
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "no wound, no weld verb"
    );
    assert_eq!(d.features[64], 0, "an unwounded roster locks no scrap");

    wound(&mut state, patient, raider, 40);
    let hurt = state.unit(patient).unwrap().hp;
    let stats = UnitKind::Harvester.stats();
    let d = gym.decision(&state);
    assert_eq!(
        d.features[64],
        i64::from(stats.cost) * i64::from(stats.max_hp - hurt) / i64::from(stats.max_hp),
        "the v6 feature prices the wound whether or not the verb is legal"
    );
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "a welder 24 tiles away is not on the leash — masking it on would lie"
    );

    // Walk the welder home; the mask must flip on with the geometry.
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: oxide_sim::Command::Move {
            units: vec![welder],
            goal: TilePos::new(10, 2),
            queue: false,
        },
    }]);
    for _ in 0..2_000 {
        state.tick(&[]);
        if state.unit(welder).unwrap().tile() == TilePos::new(10, 2) {
            break;
        }
    }
    let d = gym.decision(&state);
    assert!(
        d.mask[Action::RepairUnit as usize],
        "wound plus a welder inside the leash arms the verb"
    );
    let commands = gym.step(&state, Action::RepairUnit);
    assert!(
        commands.iter().any(|c| matches!(
            &c.command,
            oxide_sim::Command::RepairUnit { units, target, .. }
                if *target == patient && units.contains(&welder)
        )),
        "the lowering sends the welder at the patient: {commands:?}"
    );
    // And the sim honors it end to end: the wound closes. Only the
    // weld ticks — the chore channel may prospect the idle patient,
    // and a walking patient outpaces the proof, not the verb.
    let weld: Vec<_> = commands
        .into_iter()
        .filter(|c| matches!(&c.command, oxide_sim::Command::RepairUnit { .. }))
        .collect();
    state.tick(&weld);
    for _ in 0..1_000 {
        state.tick(&[]);
        if state.unit(patient).unwrap().hp > hurt {
            return;
        }
    }
    panic!("the weld never landed an hp");
}

#[test]
fn repair_unit_keeps_its_patient_out_of_a_same_think_army_draft() {
    use oxide_sim::UnitKind;

    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2),
        spec(0, UnitKind::Harvester, 5, 2),
        spec(0, UnitKind::Lancer, 6, 2), // patient and otherwise draftable fighter
    ]);
    let state = scenario.build().unwrap();
    let patient = state.units()[2].id;
    let mut json = serde_json::to_value(state).unwrap();
    json["units"][2]["hp"] = serde_json::json!(20);
    let state: oxide_sim::State = serde_json::from_value(json).unwrap();

    let mut gym = GymBot::new(PlayerId(0));
    let decision = gym.decision(&state);
    assert!(decision.mask[Action::RepairUnit as usize]);
    assert!(decision.mask[Action::FormArmy as usize]);
    let commands = gym.step_plan(
        &state,
        ActionPlan {
            construction: Action::RepairUnit,
            operation: Action::FormArmy,
            ..ActionPlan::default()
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == patient
        )),
        "the maintenance head must issue the weld: {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            &command.command,
            Command::AttackMove { units, .. } if units.contains(&patient)
        )),
        "the operations head cannot replace the patient's stationary weld order"
    );
}

#[test]
fn repair_unit_keeps_its_patient_out_of_a_same_think_scout_order() {
    use oxide_sim::UnitKind;

    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2), // patient and scout's first choice
        spec(0, UnitKind::Harvester, 5, 2),
        spec(0, UnitKind::Harvester, 6, 2),
    ]);
    let state = scenario.build().unwrap();
    let patient = state.units()[0].id;
    let mut json = serde_json::to_value(state).unwrap();
    json["units"][0]["hp"] = serde_json::json!(20);
    let state: oxide_sim::State = serde_json::from_value(json).unwrap();

    let mut gym = GymBot::new(PlayerId(0));
    let decision = gym.decision(&state);
    assert!(decision.mask[Action::RepairUnit as usize]);
    assert!(decision.mask[Action::Scout as usize]);
    let commands = gym.step_plan(
        &state,
        ActionPlan {
            construction: Action::RepairUnit,
            operation: Action::Scout,
            ..ActionPlan::default()
        },
    );
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == patient
        )),
        "the maintenance head must issue the weld: {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            &command.command,
            Command::Move { units, .. } if units.contains(&patient)
        )),
        "the operations head cannot replace the patient's stationary weld order"
    );
}

#[test]
fn repair_unit_stops_an_existing_order_before_the_weld() {
    use chassis::grid::TilePos;
    use oxide_sim::UnitKind;

    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2),
        spec(0, UnitKind::Harvester, 5, 2),
        spec(0, UnitKind::Lancer, 6, 2),
    ]);
    let state = scenario.build().unwrap();
    let patient = state.units()[2].id;
    let mut value = serde_json::to_value(state).unwrap();
    value["units"][2]["hp"] = serde_json::json!(20);
    let mut state: oxide_sim::State = serde_json::from_value(value).unwrap();
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::AttackMove {
            units: vec![patient],
            goal: TilePos::new(30, 2),
            queue: false,
        },
    }]);

    let hurt = state.unit(patient).unwrap().hp;
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::RepairUnit);
    let stop = commands
        .iter()
        .position(|command| {
            matches!(
                &command.command,
                Command::Stop { units } if units.contains(&patient)
            )
        })
        .expect("the patient gets a stationary order");
    let weld = commands
        .iter()
        .position(|command| {
            matches!(
                command.command,
                Command::RepairUnit { target, .. } if target == patient
            )
        })
        .expect("the welder follows the stop");
    assert!(stop < weld, "the stop must execute before weld validation");

    state.tick(&commands);
    for _ in 0..1_000 {
        state.tick(&[]);
        if state.unit(patient).unwrap().hp > hurt {
            return;
        }
    }
    panic!("the moving patient was never welded");
}

#[test]
fn a_wounded_rear_harvester_stays_off_the_scrap_line_until_healed() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 200;
    let state = scenario.build().unwrap();
    let patient = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .map(|unit| unit.id)
        .unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|unit| unit["id"] == patient.0)
        .unwrap()["hp"] = 20.into();
    let mut state: oxide_sim::State = serde_json::from_value(value).unwrap();
    let mut gym = GymBot::new(PlayerId(0));

    let commands = gym.step(&state, Action::RepairUnit);
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::RepairUnit { target, .. } if target == patient
    )));
    state.tick(&commands);

    for _ in 0..3 {
        let commands = gym.step_plan(&state, ActionPlan::default());
        assert!(
            !commands.iter().any(|command| matches!(
                &command.command,
                Command::Harvest { units, .. } if units.contains(&patient)
            )),
            "a wounded patient cannot be pulled away from its welder: {commands:?}"
        );
        state.tick(&commands);
        for _ in 0..15 {
            state.tick(&[]);
        }
        if state.unit(patient).unwrap().hp == UnitKind::Harvester.stats().max_hp {
            break;
        }
    }
}

fn wounded_enlisted_patient() -> (oxide_sim::State, oxide_sim::UnitId) {
    use oxide_sim::scenario::BuildingSpec;

    let mut scenario = Scenario::skirmish();
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 9,
        y: 3,
    });
    let enemy = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
        .unwrap();
    (enemy.x, enemy.y) = (17, 5);
    let state = scenario.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    let sentinel = value["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|unit| unit["player"] == 0 && unit["kind"] == "sentinel")
        .and_then(|unit| unit["id"].as_u64())
        .map(|id| oxide_sim::UnitId(id as u32))
        .unwrap();
    value["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|unit| unit["id"] == sentinel.0)
        .unwrap()["hp"] = (UnitKind::Sentinel.stats().max_hp - 1).into();
    (serde_json::from_value(value).unwrap(), sentinel)
}

fn assert_patient_not_moved(commands: &[oxide_sim::PlayerCommand], patient: oxide_sim::UnitId) {
    assert!(
        !commands.iter().any(|command| match &command.command {
            Command::Move { units, .. }
            | Command::AttackMove { units, .. }
            | Command::Attack { units, .. } => units.contains(&patient),
            _ => false,
        }),
        "the army operation must not replace the patient's weld: {commands:?}"
    );
}

#[test]
fn repair_unit_rotates_a_patient_out_before_a_same_think_push() {
    let (state, patient) = wounded_enlisted_patient();
    let mut gym = GymBot::new(PlayerId(0));
    gym.step(&state, Action::FormArmy);
    let decision = gym.decision(&state);
    assert!(decision.mask[Action::RepairUnit as usize]);
    assert!(decision.mask[Action::Push as usize]);
    let commands = gym.step_plan(
        &state,
        ActionPlan {
            construction: Action::RepairUnit,
            operation: Action::Push,
            ..ActionPlan::default()
        },
    );
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::RepairUnit { target, .. } if target == patient
    )));
    assert_patient_not_moved(&commands, patient);
}

#[test]
fn repair_unit_rotates_a_patient_out_before_a_same_think_recall() {
    let (state, patient) = wounded_enlisted_patient();
    let mut gym = GymBot::new(PlayerId(0));
    gym.step(&state, Action::FormArmy);
    gym.step(&state, Action::Push);
    let decision = gym.decision(&state);
    assert!(decision.mask[Action::RepairUnit as usize]);
    assert!(decision.mask[Action::Recall as usize]);
    let commands = gym.step_plan(
        &state,
        ActionPlan {
            construction: Action::RepairUnit,
            operation: Action::Recall,
            ..ActionPlan::default()
        },
    );
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::RepairUnit { target, .. } if target == patient
    )));
    assert_patient_not_moved(&commands, patient);
}

#[test]
fn a_patient_is_never_its_own_welder() {
    use oxide_sim::UnitKind;
    // One harvester total: wounded, site-free, and by the builder_free
    // rule a "free harvester" — but it cannot crew its own weld, so the
    // mask must stay off.
    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2), // patient, the only harvester
        spec(1, UnitKind::Scuttler, 34, 4), // raider
    ]);
    let mut state = scenario.build().unwrap();
    let (patient, raider) = (state.units()[0].id, state.units()[1].id);
    wound(&mut state, patient, raider, 40);
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "the wounded harvester is a patient, not a crew"
    );
}

#[test]
fn repair_unit_preserves_one_economy_worker() {
    use oxide_sim::UnitKind;

    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 5, 2), // sole economy worker
        spec(0, UnitKind::Lancer, 6, 2),    // patient
    ]);
    let state = scenario.build().unwrap();
    let mut json = serde_json::to_value(state).unwrap();
    json["units"][1]["hp"] = serde_json::json!(20);
    let state: oxide_sim::State = serde_json::from_value(json).unwrap();

    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "the last economy worker must not be drafted as a welder"
    );
    assert!(
        !gym.step(&state, Action::RepairUnit)
            .iter()
            .any(|command| matches!(command.command, Command::RepairUnit { .. })),
        "lowering must keep the same economy reserve as the mask"
    );
}

#[test]
fn repair_unit_requires_its_first_paid_step() {
    use oxide_sim::UnitKind;

    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2),
        spec(0, UnitKind::Harvester, 5, 2),
        spec(0, UnitKind::Sentinel, 6, 2), // patient: opening debit is 2
    ]);
    let state_at = |scrap| {
        let mut scenario = scenario.clone();
        scenario.players[0].scrap = scrap;
        let state = scenario.build().unwrap();
        let mut json = serde_json::to_value(state).unwrap();
        json["units"][2]["hp"] = serde_json::json!(20);
        serde_json::from_value::<oxide_sim::State>(json).unwrap()
    };

    let mut gym = GymBot::new(PlayerId(0));
    let state = state_at(1);
    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::RepairUnit as usize],
        "one scrap cannot reach a Sentinel weld's first paid hp step"
    );
    assert!(
        !gym.step(&state, Action::RepairUnit)
            .iter()
            .any(|command| matches!(command.command, Command::RepairUnit { .. })),
        "lowering must keep the same affordability check as the mask"
    );

    let state = state_at(2);
    assert!(
        gym.decision(&state).mask[Action::RepairUnit as usize],
        "the exact opening debit arms the action"
    );
    assert!(
        gym.step(&state, Action::RepairUnit)
            .iter()
            .any(|command| matches!(command.command, Command::RepairUnit { .. })),
        "the exact opening debit lowers to a weld"
    );
}

#[test]
fn repair_unit_prefers_recoverable_value_over_raw_hp_deficit() {
    use oxide_sim::UnitKind;

    let scenario = weld_arena(vec![
        spec(0, UnitKind::Harvester, 4, 2), // cheap, deeply wounded patient
        spec(0, UnitKind::Harvester, 5, 2), // full-health welder
        spec(0, UnitKind::Lancer, 6, 2),    // pricier, shallower wound
    ]);
    let state = scenario.build().unwrap();
    let (cheap, valuable) = (state.units()[0].id, state.units()[2].id);
    let cheap_hp = 10;
    let valuable_hp = 30;
    let mut json = serde_json::to_value(state).unwrap();
    json["units"][0]["hp"] = serde_json::json!(cheap_hp);
    json["units"][2]["hp"] = serde_json::json!(valuable_hp);
    let state: oxide_sim::State = serde_json::from_value(json).unwrap();

    let cheap_stats = UnitKind::Harvester.stats();
    let valuable_stats = UnitKind::Lancer.stats();
    let cheap_deficit = cheap_stats.max_hp - cheap_hp;
    let valuable_deficit = valuable_stats.max_hp - valuable_hp;
    assert!(
        cheap_deficit > valuable_deficit,
        "test premise: raw hp points favor the cheap patient"
    );
    assert!(
        cheap_stats.cost * cheap_deficit / cheap_stats.max_hp
            < valuable_stats.cost * valuable_deficit / valuable_stats.max_hp,
        "test premise: recoverable purchase value favors the Lancer"
    );

    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::RepairUnit);
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == valuable
        )),
        "the smaller, more valuable wound should win: {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::RepairUnit { target, .. } if target == cheap
        )),
        "raw hp deficit must not choose the cheaper wound"
    );
}

#[test]
fn build_repair_bay_lowers_to_an_accepted_foundation() {
    use oxide_sim::stats::BuildingKind;
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 400;
    let mut state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let d = gym.decision(&state);
    assert!(
        d.mask[Action::BuildRepairBay as usize],
        "scrap and a free builder arm the bay slot"
    );
    let commands = gym.step(&state, Action::BuildRepairBay);
    assert!(
        commands.iter().any(|c| matches!(
            &c.command,
            oxide_sim::Command::Build { kind, .. } if *kind == BuildingKind::RepairBay
        )),
        "the training slot founds a bay: {commands:?}"
    );
    let report = state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. })),
        "the lowered foundation validates: {:?}",
        report.events
    );
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0)),
        "the site stands"
    );
    let bay = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0))
        .unwrap()
        .id;

    let d = gym.decision(&state);
    assert!(
        !d.mask[Action::BuildRepairBay as usize],
        "an in-progress Bay blocks a duplicate"
    );
    let mut duplicate_probe = state.clone();
    duplicate_probe.tick(&gym.step(&state, Action::BuildRepairBay));
    assert_eq!(
        duplicate_probe
            .buildings()
            .iter()
            .filter(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0))
            .count(),
        1,
        "direct lowering cannot bypass the in-progress guard"
    );

    for _ in 0..500 {
        state.tick(&[]);
        if state.building(bay).is_some_and(|b| b.built) {
            break;
        }
    }
    assert!(
        state.building(bay).is_some_and(|b| b.built),
        "test premise: the first Bay completes"
    );
    let mut completed = GymBot::new(PlayerId(0));
    assert!(
        !completed.decision(&state).mask[Action::BuildRepairBay as usize],
        "a completed Bay blocks a duplicate"
    );
    let mut duplicate_probe = state.clone();
    duplicate_probe.tick(&completed.step(&state, Action::BuildRepairBay));
    assert_eq!(
        duplicate_probe
            .buildings()
            .iter()
            .filter(|b| b.kind == BuildingKind::RepairBay && b.player == PlayerId(0))
            .count(),
        1,
        "direct lowering cannot bypass the completed guard"
    );

    let salvager = state
        .units()
        .iter()
        .find(|u| u.player == PlayerId(0) && u.kind == UnitKind::Harvester)
        .unwrap()
        .id;
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Salvage {
            units: vec![salvager],
            building: bay,
            queue: false,
        },
    }]);
    for _ in 0..1_000 {
        state.tick(&[]);
        if state.building(bay).is_none() {
            break;
        }
    }
    assert!(
        state.building(bay).is_none(),
        "test premise: the original Bay is gone"
    );
    let mut rebuild = GymBot::new(PlayerId(0));
    assert!(
        rebuild.decision(&state).mask[Action::BuildRepairBay as usize],
        "once the old Bay is gone, rebuilding becomes legal"
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
