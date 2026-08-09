//! Gym-interface contracts: a scripted action sequence reproduces
//! bit-identically (training rollouts must be replayable), and the
//! masked menu is honest enough to play a real game through.

use chassis::rng::Pcg32;
use oxide_sim::bot::{
    ACTION_HEADS, Action, ActionPlan, Brain, CONSTRUCTION_PLAN_TIMEOUT_TICKS, Difficulty,
    FEATURE_NAMES, GymBot, Level, NeuralBot, Observation, seat_bots,
};
use oxide_sim::state::{GameResult, Order};
use oxide_sim::stats::FOUNDRY_RECOVERY_RESERVE;
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

fn defense_build_anchor(state: &oxide_sim::State, action: Action) -> chassis::grid::TilePos {
    let mut bot = GymBot::new(PlayerId(0));
    bot.step(state, action)
        .into_iter()
        .find_map(|command| match command.command {
            Command::Build { anchor, .. } => Some(anchor),
            _ => None,
        })
        .expect("the funded defense action should lower to a build")
}

#[test]
fn defense_placement_is_deterministic_and_tracks_the_visible_approach() {
    use oxide_sim::scenario::UnitSpec;

    let threatened = |y| {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 1_000;
        scenario.units.retain(|unit| unit.player == 0);
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 12,
            y,
        });
        scenario.build().unwrap()
    };
    let north = threatened(4);
    let south = threatened(11);
    let north_anchor = defense_build_anchor(&north, Action::BuildTurret);
    let north_repeat = defense_build_anchor(&north, Action::BuildTurret);
    let south_anchor = defense_build_anchor(&south, Action::BuildTurret);
    assert_eq!(
        north_anchor, north_repeat,
        "the same fog-honest state must choose the same scored site"
    );
    assert!(
        north_anchor.y < south_anchor.y,
        "the defense should move with the visible approach: north {north_anchor:?}, south \
         {south_anchor:?}"
    );
}

#[test]
fn unseen_enemy_units_cannot_steer_defense_placement() {
    use oxide_sim::scenario::UnitSpec;

    let mut quiet = Scenario::skirmish();
    quiet.players[0].scrap = 1_000;
    quiet.units.retain(|unit| unit.player == 0);
    let quiet = quiet.build().unwrap();

    let mut hidden = Scenario::skirmish();
    hidden.players[0].scrap = 1_000;
    hidden.units.retain(|unit| unit.player == 0);
    hidden.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 25,
        y: 10,
    });
    let hidden = hidden.build().unwrap();

    assert_eq!(
        defense_build_anchor(&quiet, Action::BuildBastion),
        defense_build_anchor(&hidden, Action::BuildBastion),
        "the scored site may read visible threats, shared sight, blips, and shells, never an \
         unseen enemy unit"
    );
}

#[test]
fn flak_placement_tracks_air_pressure_not_ground_noise() {
    use oxide_sim::scenario::UnitSpec;

    let threatened = |kind, y| {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 1_000;
        scenario.units.retain(|unit| unit.player == 0);
        scenario.units.push(UnitSpec {
            player: 1,
            kind,
            x: 12,
            y,
        });
        scenario.build().unwrap()
    };
    let ground_north = defense_build_anchor(&threatened(UnitKind::Sentinel, 4), Action::BuildFlak);
    let ground_south = defense_build_anchor(&threatened(UnitKind::Sentinel, 11), Action::BuildFlak);
    assert_eq!(
        ground_north, ground_south,
        "ground-only pressure must not pull an anti-air battery off its air-defense job"
    );

    let air_north = defense_build_anchor(&threatened(UnitKind::Darter, 4), Action::BuildFlak);
    let air_south = defense_build_anchor(&threatened(UnitKind::Darter, 11), Action::BuildFlak);
    assert!(
        air_north.y < air_south.y,
        "the flak battery should move with the visible air approach: north {air_north:?}, south \
         {air_south:?}"
    );
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

fn guarded_stranded_scenario(scrap: u32) -> Scenario {
    let mut scenario = stranded_scenario(scrap);
    scenario.units.retain(|unit| unit.player != 0);
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 10,
        y: 2,
    });
    scenario
}

fn turret_guarded_stranded_scenario(scrap: u32) -> Scenario {
    let mut scenario = stranded_scenario(scrap);
    scenario.units.retain(|unit| unit.player != 0);
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 1,
        kind: BuildingKind::Turret,
        x: 10,
        y: 1,
    });
    scenario
}

fn set_map_tile(scenario: &mut Scenario, tile: chassis::grid::TilePos, value: u8) {
    let row = scenario.map[tile.y as usize].as_bytes();
    let mut replaced = row.to_vec();
    replaced[tile.x as usize] = value;
    scenario.map[tile.y as usize] = String::from_utf8(replaced).unwrap();
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
fn guarded_recovery_buys_a_screen_before_its_harvester() {
    let state = guarded_stranded_scenario(FOUNDRY_RECOVERY_RESERVE)
        .build()
        .unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step_plan(&state, ActionPlan::default());
    let trained: Vec<UnitKind> = commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Train { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        trained,
        vec![UnitKind::Sentinel, UnitKind::Harvester],
        "a visibly guarded salvage line needs a cheap screen before the worker: {commands:?}"
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command.command, Command::Surrender)),
        "a healthy Foundry with symmetric fallback income is recoverable"
    );
}

#[test]
fn guarded_recovery_does_not_mistake_lone_artillery_for_a_screen() {
    let mut first = guarded_stranded_scenario(0);
    first.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Bombard,
        x: 5,
        y: 7,
    });
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let _ = gym.step_plan(&state, ActionPlan::default());

    first.players[0].scrap = UnitKind::Sentinel.stats().cost;
    first.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    let state = first.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(1);
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();
    let commands = gym.step_plan(&state, ActionPlan::default());
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Sentinel,
                ..
            }
        )),
        "artillery cannot directly contest a guarded salvage line: {commands:?}"
    );
}

#[test]
fn guarded_recovery_counts_a_queued_screen_before_training_another() {
    let first = guarded_stranded_scenario(0);
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let _ = gym.step_plan(&state, ActionPlan::default());

    let mut recovered = first;
    recovered.players[0].scrap = 2 * UnitKind::Sentinel.stats().cost;
    recovered.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    let state = recovered.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(1);
    let mut state: oxide_sim::State = serde_json::from_value(value).unwrap();

    let first_commands = gym.step_plan(&state, ActionPlan::default());
    assert_eq!(
        first_commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Sentinel,
                    ..
                }
            ))
            .count(),
        1,
        "the recovered Harvester needs one direct screen: {first_commands:?}"
    );
    state.tick(&first_commands);

    let second_commands = gym.step_plan(&state, ActionPlan::default());
    assert!(
        !second_commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Sentinel,
                ..
            }
        )),
        "the queued screen already satisfies recovery: {second_commands:?}"
    );
}

#[test]
fn guarded_recovery_cancels_a_naked_prepaid_harvester() {
    let scenario = guarded_stranded_scenario(2 * UnitKind::Harvester.stats().cost);
    let mut state = scenario.build().unwrap();
    let foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0))
        .unwrap()
        .id;
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    }]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        UnitKind::Harvester.stats().cost
    );

    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step_plan(&state, ActionPlan::default());
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::CancelTrain {
                building,
                index: 0
            } if building == foundry
        )),
        "the exposed worker is refunded so public recovery income can fund a coherent package: {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "the same naked worker must not be immediately re-queued"
    );
}

#[test]
fn guarded_recovery_keeps_the_earliest_queued_screen() {
    let mut state = guarded_stranded_scenario(500).build().unwrap();
    let foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .unwrap()
        .id;
    let queue = (0..3)
        .map(|_| oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: foundry,
                kind: UnitKind::Sentinel,
            },
        })
        .collect::<Vec<_>>();
    state.tick(&queue);

    let commands = GymBot::new(PlayerId(0)).step_plan(&state, ActionPlan::default());
    let cancelled: Vec<u8> = commands
        .iter()
        .filter_map(|command| match command.command {
            Command::CancelTrain { building, index } if building == foundry => Some(index),
            _ => None,
        })
        .collect();
    assert_eq!(
        cancelled,
        vec![2, 1],
        "later screens cancel in index-safe reverse order while the head keeps its progress"
    );
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            building,
            kind: UnitKind::Harvester,
        } if building == foundry
    )));
}

#[test]
fn recovery_reroutes_to_a_safe_known_source() {
    let mut first = guarded_stranded_scenario(UnitKind::Harvester.stats().cost);
    let guard = first
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel && unit.x == 10)
        .expect("the recovery fixture has a nearby guard");
    guard.x = 14;
    // This source lies in the Foundry's initial sight but outside the
    // witnessed guard's danger radius, with a danger-free route from home.
    let safe = chassis::grid::TilePos::new(2, 12);
    set_map_tile(&mut first, safe, b's');
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step_plan(&state, ActionPlan::default());
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));

    let mut second = first;
    second.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    let state = second.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(1);
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();
    let commands = gym.step_plan(&state, ActionPlan::default());
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Harvest { node, .. } if node == safe
        )),
        "danger memory must route the replacement away from the guarded home patch: {commands:?}"
    );
}

#[test]
fn recovery_releases_a_replacement_only_after_its_harvest_order_lands() {
    let mut scenario = guarded_stranded_scenario(UnitKind::Harvester.stats().cost);
    let guard = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel && unit.x == 10)
        .expect("the recovery fixture has a nearby guard");
    guard.x = 14;
    let safe = chassis::grid::TilePos::new(2, 12);
    set_map_tile(&mut scenario, safe, b's');
    let mut gym = GymBot::new(PlayerId(0));
    let initial = gym.step_plan(&scenario.build().unwrap(), ActionPlan::default());
    assert!(initial.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));

    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    value["tick"] = serde_json::json!(1);
    let replacement: oxide_sim::State = serde_json::from_value(value).unwrap();
    let assignment = gym.step_plan(&replacement, ActionPlan::default());
    let worker = assignment
        .iter()
        .find_map(|command| match &command.command {
            Command::Harvest { units, node, .. } if *node == safe => units.first().copied(),
            _ => None,
        })
        .expect("recovery assigns the replacement to the safe source");

    let mut accepted_state = replacement.clone();
    accepted_state.tick(&assignment);
    let working = accepted_state
        .units()
        .iter()
        .find(|unit| unit.id == worker)
        .expect("the replacement survives command application");
    assert!(matches!(
        working.order,
        Order::Harvest {
            node,
            anchor,
            retiring: false,
        } if anchor.unwrap_or(node) == safe
    ));
    assert!(
        working.path.is_some() || working.progress > 0 || working.carrying > 0,
        "the accepted assignment has observable work progress"
    );
    let accepted = gym.clone().decision(&accepted_state);
    assert!(
        accepted.mask[Action::TrainSentinel as usize],
        "a safe, working Harvest route releases the emergency decision surface"
    );

    let mut dry_scenario = scenario;
    set_map_tile(&mut dry_scenario, safe, b'.');
    let mut dry_value = serde_json::to_value(dry_scenario.build().unwrap()).unwrap();
    dry_value["tick"] = serde_json::json!(1);
    let mut rejected_state: oxide_sim::State = serde_json::from_value(dry_value).unwrap();
    rejected_state.tick(&assignment);
    let rejected_worker = rejected_state
        .units()
        .iter()
        .find(|unit| unit.id == worker)
        .expect("the replacement survives the rejected command");
    assert!(
        matches!(rejected_worker.order, Order::Idle),
        "the sim rejects the now-dry source instead of landing the assignment"
    );

    let rejected = gym.decision(&rejected_state);
    let legal: Vec<usize> = rejected
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
        "a rejected replacement assignment keeps recovery reconciliation active"
    );

    let ordinary = GymBot::new(PlayerId(0)).decision(&replacement);
    assert!(
        ordinary.mask[Action::TrainSentinel as usize],
        "a fresh one-worker opening never enters emergency recovery"
    );
}

#[test]
fn recovery_keeps_a_pathful_retiring_assignment_inside_reconciliation() {
    let mut scenario = guarded_stranded_scenario(UnitKind::Harvester.stats().cost);
    let guard = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel && unit.x == 10)
        .expect("the recovery fixture has a nearby guard");
    guard.x = 14;
    let safe = chassis::grid::TilePos::new(2, 12);
    set_map_tile(&mut scenario, safe, b's');
    let mut gym = GymBot::new(PlayerId(0));
    let _ = gym.step_plan(&scenario.build().unwrap(), ActionPlan::default());

    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 8,
    });
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    value["tick"] = serde_json::json!(1);
    let clear: oxide_sim::State = serde_json::from_value(value).unwrap();
    let assignment = gym.step_plan(&clear, ActionPlan::default());
    let worker = assignment
        .iter()
        .find_map(|command| match &command.command {
            Command::Harvest { units, node, .. } if *node == safe => units.first().copied(),
            _ => None,
        })
        .expect("the clear route receives an assignment");

    for (dx, dy) in chassis::grid::CARDINALS
        .into_iter()
        .chain(chassis::grid::DIAGONALS)
    {
        set_map_tile(&mut scenario, safe.offset(dx, dy), b'#');
    }
    let mut blocked_value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    blocked_value["tick"] = serde_json::json!(1);
    let mut blocked: oxide_sim::State = serde_json::from_value(blocked_value).unwrap();
    blocked.tick(&assignment);
    let retiring = blocked
        .units()
        .iter()
        .find(|unit| unit.id == worker)
        .expect("the replacement survives the failed route");
    assert!(matches!(
        retiring.order,
        Order::Harvest {
            node,
            anchor,
            retiring: true,
        } if anchor.unwrap_or(node) == safe
    ));
    assert!(
        retiring.path.is_some(),
        "the failed source route becomes a pathful retirement toward home"
    );

    let decision = gym.decision(&blocked);
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
        "a path is not viable work when the Harvest order is retiring"
    );
}

#[test]
fn recovery_does_not_send_a_replacement_across_endpoint_clear_danger() {
    let mut scenario = guarded_stranded_scenario(UnitKind::Harvester.stats().cost);
    scenario.units.retain(|unit| {
        unit.player != 1 || (unit.kind == UnitKind::Sentinel && unit.x == 10 && unit.y == 2)
    });
    let guard = chassis::grid::TilePos::new(13, 12);
    let guard_unit = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1)
        .expect("the fixture keeps one hostile guard");
    guard_unit.x = guard.x;
    guard_unit.y = guard.y;
    let salvage: Vec<_> = scenario
        .map
        .iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.bytes().enumerate().filter_map(move |(x, tile)| {
                matches!(tile, b's' | b'S').then_some(chassis::grid::TilePos::new(
                    i32::try_from(x).unwrap(),
                    i32::try_from(y).unwrap(),
                ))
            })
        })
        .collect();
    for tile in salvage {
        set_map_tile(&mut scenario, tile, b'.');
    }
    for y in 1..23 {
        if y != guard.y {
            set_map_tile(&mut scenario, chassis::grid::TilePos::new(guard.x, y), b'#');
        }
    }
    let safe = chassis::grid::TilePos::new(21, 18);
    assert!(guard.chebyshev(safe) > 7, "the source endpoint is clear");
    set_map_tile(&mut scenario, safe, b's');
    for (x, y) in [(12, 4), (12, 11), (12, 18), (21, 17)] {
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x,
            y,
        });
    }
    let mut gym = GymBot::new(PlayerId(0));
    let initial = gym.step_plan(&scenario.build().unwrap(), ActionPlan::default());
    assert!(initial.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));

    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    value["tick"] = serde_json::json!(1);
    let replacement: oxide_sim::State = serde_json::from_value(value).unwrap();
    let observed = Observation::fog_honest(&replacement, PlayerId(0));
    assert!(
        (1..23).filter(|y| *y != guard.y).all(|y| observed
            .known_rock
            .contains(&chassis::grid::TilePos::new(guard.x, y))),
        "the bot has explored the impassable wall around the guarded opening"
    );
    let worker = replacement
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .expect("the replacement exists")
        .id;

    let mut mechanical = replacement.clone();
    mechanical.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: Command::Harvest {
            units: vec![worker],
            node: safe,
            queue: false,
        },
    }]);
    let path = mechanical
        .units()
        .iter()
        .find(|unit| unit.id == worker)
        .and_then(|unit| unit.path.as_ref())
        .expect("the explicit command has a mechanically valid route");
    assert!(
        path.waypoints
            .iter()
            .any(|waypoint| waypoint.chebyshev(guard) <= 7),
        "the mechanically valid route crosses the bot's remembered danger envelope"
    );

    let commands = gym.step_plan(&replacement, ActionPlan::default());
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command.command, Command::Harvest { .. })),
        "endpoint safety alone must not dispatch the replacement: {commands:?}"
    );
    let decision = gym.decision(&replacement);
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
        "the route-blocked replacement stays inside recovery reconciliation"
    );
}

#[test]
fn recovery_preflight_treats_visible_siege_reach_as_a_hard_barrier() {
    let mut scenario = guarded_stranded_scenario(UnitKind::Harvester.stats().cost);
    scenario.units.retain(|unit| {
        unit.player != 1 || (unit.kind == UnitKind::Sentinel && unit.x == 10 && unit.y == 2)
    });
    let bombard = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1)
        .expect("the fixture keeps one hostile guard");
    bombard.kind = UnitKind::Bombard;
    bombard.x = 7;
    bombard.y = 6;
    let salvage: Vec<_> = scenario
        .map
        .iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.bytes().enumerate().filter_map(move |(x, tile)| {
                matches!(tile, b's' | b'S').then_some(chassis::grid::TilePos::new(
                    i32::try_from(x).unwrap(),
                    i32::try_from(y).unwrap(),
                ))
            })
        })
        .collect();
    for tile in salvage {
        set_map_tile(&mut scenario, tile, b'.');
    }
    let source = chassis::grid::TilePos::new(17, 6);
    set_map_tile(&mut scenario, source, b's');
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 17,
        y: 7,
    });

    let mut gym = GymBot::new(PlayerId(0));
    let initial = gym.step_plan(&scenario.build().unwrap(), ActionPlan::default());
    assert!(initial.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    value["tick"] = serde_json::json!(1);
    let replacement: oxide_sim::State = serde_json::from_value(value).unwrap();
    let worker = replacement
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .unwrap();
    assert_eq!(
        worker.tile().chebyshev(chassis::grid::TilePos::new(7, 6)),
        2
    );
    assert_eq!(source.chebyshev(chassis::grid::TilePos::new(7, 6)), 10);

    let commands = gym.step_plan(&replacement, ActionPlan::default());
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command.command, Command::Harvest { .. })),
        "outward travel remains unsafe inside a visible Bombard's 12.5-tile salvage reach: \
         {commands:?}"
    );
}

#[test]
fn recovery_confirms_a_flipped_seats_world_space_assignment() {
    let mut scenario = Scenario::skirmish();
    scenario.players[1].scrap = UnitKind::Harvester.stats().cost;
    scenario
        .units
        .retain(|unit| unit.player != 1 || unit.kind != UnitKind::Harvester);
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 10,
        y: 7,
    });
    let mut gym = GymBot::new(PlayerId(1));
    let initial = gym.step_plan(&scenario.build().unwrap(), ActionPlan::default());
    assert!(initial.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));

    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Harvester,
        x: 34,
        y: 17,
    });
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    value["tick"] = serde_json::json!(1);
    let replacement: oxide_sim::State = serde_json::from_value(value).unwrap();
    let assignment = gym.step_plan(&replacement, ActionPlan::default());
    let (worker, world_source) = assignment
        .iter()
        .find_map(|command| match &command.command {
            Command::Harvest { units, node, .. } => Some((*units.first()?, *node)),
            _ => None,
        })
        .expect("the flipped seat assigns its replacement");
    assert!(
        world_source.x > replacement.map().width() / 2,
        "the emitted command carries the southeast seat's world-space source"
    );

    let mut accepted = replacement;
    accepted.tick(&assignment);
    let working = accepted
        .units()
        .iter()
        .find(|unit| unit.id == worker)
        .expect("the replacement survives command application");
    assert!(matches!(
        working.order,
        Order::Harvest {
            node,
            anchor,
            retiring: false,
        } if anchor.unwrap_or(node) == world_source
    ));
    assert!(working.path.is_some());
    let mut danger_gym = gym.clone();
    let decision = gym.decision(&accepted);
    assert!(
        decision.mask.iter().enumerate().any(|(index, legal)| {
            *legal
                && ![
                    Action::Idle as usize,
                    Action::NoConstruction as usize,
                    Action::NoOperation as usize,
                ]
                .contains(&index)
        }),
        "world-space order confirmation must release the flipped seat's recovery surface"
    );

    let visible_enemy = Observation::fog_honest(&accepted, PlayerId(1))
        .enemy_units
        .into_iter()
        .find(|unit| {
            unit.kind
                .stats()
                .can_target(oxide_sim::stats::Domain::Ground)
        })
        .expect("the remote spotter keeps one hostile ground threat visible")
        .tile;
    let worker_slot = accepted
        .units()
        .iter()
        .position(|unit| unit.id == worker)
        .unwrap();
    let mut forged = serde_json::to_value(&accepted).unwrap();
    let next = forged["units"][worker_slot]["path"]["next"]
        .as_u64()
        .and_then(|next| usize::try_from(next).ok())
        .unwrap();
    let waypoints = forged["units"][worker_slot]["path"]["waypoints"]
        .as_array_mut()
        .unwrap();
    assert!(next < waypoints.len());
    waypoints[next] = serde_json::to_value(visible_enemy).unwrap();
    let dangerous_path: oxide_sim::State = serde_json::from_value(forged).unwrap();
    let blocked = danger_gym.decision(&dangerous_path);
    let legal: Vec<usize> = blocked
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
        "world-space waypoints must be oriented before auditing the flipped seat's route"
    );
}

#[test]
fn a_recovery_worker_liquidates_a_useful_nonessential_asset() {
    let first = guarded_stranded_scenario(0);
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let _ = gym.step_plan(&state, ActionPlan::default());

    let mut second = first;
    second.players[0].scrap = 10;
    second.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    second.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 3,
        y: 8,
    });
    let state = second.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(1);
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();
    let turret = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let commands = gym.step_plan(&state, ActionPlan::default());
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Salvage { building, .. } if building == turret
        )),
        "an otherwise stranded worker should turn a nonessential turret into most of the screen fund: {commands:?}"
    );
}

#[test]
fn danger_memory_cools_before_recovery_reuses_a_source() {
    let first = guarded_stranded_scenario(UnitKind::Harvester.stats().cost);
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let initial = gym.step_plan(&state, ActionPlan::default());
    assert!(
        !initial.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "fresh danger must keep a naked worker out of the guarded line"
    );

    let mut quiet = first;
    quiet.units.retain(|unit| {
        !(unit.player == 1 && unit.kind == UnitKind::Sentinel && unit.x == 10 && unit.y == 2)
    });
    let at_tick = |tick: u64| {
        let state = quiet.build().unwrap();
        let mut value = serde_json::to_value(state).unwrap();
        value["tick"] = serde_json::json!(tick);
        serde_json::from_value::<oxide_sim::State>(value).unwrap()
    };
    let mut still_hot = gym.clone();
    let commands = still_hot.step_plan(&at_tick(1_799), ActionPlan::default());
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "the last deterministic cooling tick remains guarded"
    );
    let commands = gym.step_plan(&at_tick(1_800), ActionPlan::default());
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "expired danger memory must release the known source: {commands:?}"
    );
}

#[test]
fn a_remembered_armed_building_keeps_its_salvage_line_guarded() {
    let scenario = turret_guarded_stranded_scenario(UnitKind::Harvester.stats().cost);
    let state = scenario.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let initial = gym.step_plan(&state, ActionPlan::default());
    assert!(
        !initial.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "the visible Turret guards the home salvage"
    );

    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(1_800);
    for visible in value["vision"][0]["visible"]["cells"]
        .as_array_mut()
        .unwrap()
    {
        *visible = false.into();
    }
    let hidden: oxide_sim::State = serde_json::from_value(value).unwrap();
    let observation = Observation::fog_honest(&hidden, PlayerId(0));
    assert!(
        observation
            .enemy_buildings
            .iter()
            .any(|building| { building.kind == BuildingKind::Turret && !building.seen })
    );

    let commands = gym.step_plan(&hidden, ActionPlan::default());
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "a ghost Turret remains a static guard after mobile danger would cool: {commands:?}"
    );
}

#[test]
fn recovery_preview_and_direct_lowering_emit_identical_replay_commands() {
    let scenario = guarded_stranded_scenario(FOUNDRY_RECOVERY_RESERVE);
    let mut previewed_state = scenario.build().unwrap();
    let mut direct_state = previewed_state.clone();
    let mut previewed = GymBot::new(PlayerId(0));
    let mut direct = GymBot::new(PlayerId(0));

    for _ in 0..512 {
        let mut previewed_commands = Vec::new();
        let mut direct_commands = Vec::new();
        if previewed_state
            .current_tick()
            .is_multiple_of(previewed.cadence())
            && previewed_state.result().is_none()
        {
            let _ = previewed.decision(&previewed_state);
            previewed_commands.extend(previewed.step_plan(&previewed_state, ActionPlan::default()));
            direct_commands.extend(direct.step_plan(&direct_state, ActionPlan::default()));
        }
        assert_eq!(
            previewed_commands,
            direct_commands,
            "decision preview changed lowering at tick {}",
            previewed_state.current_tick()
        );
        previewed_state.tick(&previewed_commands);
        direct_state.tick(&direct_commands);
        assert_eq!(
            previewed_state.hash(),
            direct_state.hash(),
            "equivalent recorded commands diverged at tick {}",
            previewed_state.current_tick()
        );
    }
}

#[test]
fn a_stalled_recovery_worker_hold_retries_on_a_bounded_cadence() {
    let first = guarded_stranded_scenario(0);
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let _ = gym.step_plan(&state, ActionPlan::default());

    let mut with_worker = first;
    with_worker.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 2,
        y: 10,
    });
    let at_tick = |tick: u64| {
        let state = with_worker.build().unwrap();
        let mut value = serde_json::to_value(state).unwrap();
        value["tick"] = serde_json::json!(tick);
        serde_json::from_value::<oxide_sim::State>(value).unwrap()
    };
    let is_hold =
        |command: &oxide_sim::PlayerCommand| matches!(command.command, Command::Move { .. });

    let first_hold = gym.step_plan(&at_tick(1), ActionPlan::default());
    assert!(first_hold.iter().any(is_hold));
    let suppressed = gym.step_plan(&at_tick(2), ActionPlan::default());
    assert!(
        !suppressed.iter().any(is_hold),
        "the accepted path gets time to make progress"
    );
    let retry = gym.step_plan(&at_tick(121), ActionPlan::default());
    assert!(
        retry.iter().any(is_hold),
        "a rejected or stalled hold cannot suppress retries forever: {retry:?}"
    );
}

#[test]
fn stale_artillery_army_does_not_suppress_a_fresh_recovery_screen() {
    let first = guarded_stranded_scenario(0);
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let _ = gym.step_plan(&state, ActionPlan::default());

    let mut contested = first;
    contested.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    for x in [5, 6] {
        contested.units.push(oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x,
            y: 12,
        });
    }
    let state = contested.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(1);
    let mut state: oxide_sim::State = serde_json::from_value(value).unwrap();

    let form = gym.step_plan(&state, ActionPlan::default());
    let stale_member = form
        .iter()
        .find_map(|command| match &command.command {
            Command::AttackMove { units, .. } => units.first().copied(),
            _ => None,
        })
        .expect("recovery should form one direct screen");
    let fresh_screen = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
        .map(|unit| unit.id)
        .find(|unit| *unit != stale_member)
        .expect("the second direct screen stays outside the first army");
    state.tick(&form);

    let push = gym.step_plan(&state, ActionPlan::default());
    assert!(
        push.iter().any(|command| matches!(
            &command.command,
            Command::AttackMove { units, .. } if units.contains(&stale_member)
        )),
        "the staged recovery screen should begin contesting the guarded source: {push:?}"
    );
    state.tick(&push);

    let mut value = serde_json::to_value(state).unwrap();
    let stale = value["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|unit| unit["id"] == serde_json::json!(stale_member.0))
        .expect("the enlisted screen remains alive");
    stale["kind"] = serde_json::json!("bombard");
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();

    let replacement = gym.step_plan(&state, ActionPlan::default());
    assert!(
        replacement.iter().any(|command| matches!(
            &command.command,
            Command::AttackMove { units, .. } if units.contains(&fresh_screen)
        )),
        "a stale pure-artillery army cannot make the new direct screen look redundant: \
         {replacement:?}"
    );
}

#[test]
fn a_recovery_screen_secures_the_source_after_its_push_lifecycle() {
    let first = guarded_stranded_scenario(0);
    let state = first.build().unwrap();
    let mut gym = GymBot::new(PlayerId(0));
    let _ = gym.step_plan(&state, ActionPlan::default());

    let mut contested = first.clone();
    contested.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 5,
        y: 6,
    });
    contested.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 5,
        y: 12,
    });
    let state = contested.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(1);
    let mut state: oxide_sim::State = serde_json::from_value(value).unwrap();
    let form = gym.step_plan(&state, ActionPlan::default());
    assert!(
        form.iter()
            .any(|command| matches!(command.command, Command::AttackMove { .. }))
    );
    state.tick(&form);

    let push = gym.step_plan(&state, ActionPlan::default());
    let source = push
        .iter()
        .find_map(|command| match command.command {
            Command::AttackMove { goal, .. } if goal.y <= 3 => Some(goal),
            _ => None,
        })
        .expect("the staged screen contests one guarded source");

    let guard = contested
        .units
        .iter_mut()
        .find(|unit| {
            unit.player == 1 && unit.kind == UnitKind::Sentinel && unit.x == 10 && unit.y == 2
        })
        .unwrap();
    guard.kind = UnitKind::Harvester;
    guard.x = 30;
    guard.y = 16;
    let screen = contested
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Sentinel)
        .unwrap();
    screen.x = source.x;
    screen.y = source.y + 2;
    let state = contested.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(3);
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();
    let secured = gym.step_plan(&state, ActionPlan::default());
    assert!(
        secured.iter().any(|command| matches!(
            command.command,
            Command::Harvest { node, .. } if node == source
        )),
        "a Pushing recovery army that reaches a cleared source must release its held worker: {secured:?}"
    );
    assert!(
        !secured
            .iter()
            .any(|command| matches!(command.command, Command::AttackMove { .. })),
        "arrival must not create a second recovery army"
    );
}

#[test]
fn recovery_concedes_only_a_critically_wounded_position_under_visible_pressure() {
    let scenario = guarded_stranded_scenario(0);
    let state = scenario.build().unwrap();
    let (foundry, foundry_max_hp) = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .map(|building| (building.id, building.kind.stats().max_hp))
        .unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["buildings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|building| building["id"] == foundry.0)
        .unwrap()["hp"] = serde_json::json!(foundry_max_hp / 4);
    let state: oxide_sim::State = serde_json::from_value(value).unwrap();

    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step_plan(&state, ActionPlan::default());
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command.command, Command::Surrender))
            .count(),
        1,
        "a doomed, assetless seat may stop feeding replacements into the guard: {commands:?}"
    );
}

fn passive_victim_scenario(hidden_enemy_fighters: usize) -> Scenario {
    let mut scenario = Scenario::skirmish();
    scenario.units.clear();
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 7,
        y: 5,
    });
    for index in 0..8 {
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8 + index % 4,
            y: 7 + index / 4,
        });
    }
    // This spotter makes the hostile Foundry a legal, fog-honest target.
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 31,
        y: 18,
    });
    for index in 0..hidden_enemy_fighters {
        let index = i32::try_from(index).unwrap();
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 18 + index % 4,
            y: 2 + index / 4,
        });
    }
    scenario
}

fn at_passive_victim_tick(scenario: &Scenario) -> oxide_sim::State {
    let state = scenario.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["tick"] = serde_json::json!(27_002);
    serde_json::from_value(value).unwrap()
}

#[test]
fn passive_victim_finish_commitment_forms_and_pushes_a_dominant_army() {
    let mut state = at_passive_victim_tick(&passive_victim_scenario(0));
    let target = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .unwrap()
        .anchor;
    let mut gym = GymBot::new(PlayerId(0));

    let first = gym.step_plan(&state, ActionPlan::default());
    assert!(
        first.iter().any(|command| matches!(
            &command.command,
            Command::AttackMove { units, goal, .. }
                if !units.is_empty() && goal.chebyshev(target) > 3
        )),
        "the no-operation learned plan must be reconciled into army formation: {first:?}"
    );
    state.tick(&first);

    for _ in 0..4 {
        let commands = gym.step_plan(&state, ActionPlan::default());
        let pushed = commands.iter().any(|command| {
            matches!(
                command.command,
                Command::AttackMove { goal, .. } if goal == target
            )
        });
        state.tick(&commands);
        if pushed {
            return;
        }
    }
    panic!("the dominant army never committed to the known Foundry");
}

#[test]
fn undersized_active_finish_remnant_does_not_freeze_reserves() {
    let mut scenario = passive_victim_scenario(0);
    for index in 0..8 {
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8 + index % 4,
            y: 10 + index / 4,
        });
    }
    let state = at_passive_victim_tick(&scenario);
    let target = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .unwrap()
        .anchor;
    let mut gym = GymBot::new(PlayerId(0));

    let formed = gym.step(&state, Action::FormArmy);
    let members = formed
        .iter()
        .find_map(|command| match &command.command {
            Command::AttackMove { units, goal, .. } if *goal != target => Some(units.clone()),
            _ => None,
        })
        .expect("the first army should form at its rally point");
    assert!(members.len() >= 5);
    gym.step(&state, Action::Push);

    let casualties: Vec<_> = members.into_iter().skip(1).collect();
    let mut value = serde_json::to_value(state).unwrap();
    value["units"].as_array_mut().unwrap().retain(|unit| {
        unit["player"] != 0
            || !unit["id"]
                .as_u64()
                .is_some_and(|id| casualties.iter().any(|lost| u64::from(lost.0) == id))
    });
    let thinned: oxide_sim::State = serde_json::from_value(value).unwrap();

    let reserve = gym.step_plan(&thinned, ActionPlan::default());
    assert!(
        reserve.iter().any(|command| matches!(
            &command.command,
            Command::AttackMove { units, goal, .. }
                if !units.is_empty() && *goal != target
        )),
        "an undersized active remnant must release idle fighters into a new body: {reserve:?}"
    );

    for _ in 0..6 {
        let commands = gym.step_plan(&thinned, ActionPlan::default());
        if commands.iter().any(|command| {
            matches!(
                command.command,
                Command::AttackMove { goal, .. } if goal == target
            )
        }) {
            return;
        }
    }
    panic!("the reserve body never committed past the undersized active remnant");
}

#[test]
fn finish_commitment_chains_from_a_cleared_site_to_the_next_known_site() {
    let first_target = chassis::grid::TilePos::new(22, 13);
    let mut scenario = passive_victim_scenario(0);
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Sentinel,
        x: 12,
        y: 8,
    });
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: first_target.x,
        y: first_target.y - 3,
    });
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 1,
        kind: BuildingKind::Turret,
        x: first_target.x,
        y: first_target.y,
    });
    let mut state = at_passive_victim_tick(&scenario);
    let final_target = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .unwrap()
        .anchor;
    let mut gym = GymBot::new(PlayerId(0));
    let mut first_push = false;
    let mut attack_goals = Vec::new();
    for _ in 0..5 {
        let commands = gym.step_plan(&state, ActionPlan::default());
        attack_goals.extend(commands.iter().filter_map(|command| match command.command {
            Command::AttackMove { goal, .. } => Some(goal),
            _ => None,
        }));
        first_push |= commands.iter().any(|command| {
            matches!(
                command.command,
                Command::AttackMove { goal, .. } if goal == first_target
            )
        });
        state.tick(&commands);
        if first_push {
            break;
        }
    }
    assert!(
        first_push,
        "the nearer known Turret must be the first site; goals: {attack_goals:?}"
    );

    scenario
        .buildings
        .retain(|building| building.kind != BuildingKind::Turret);
    let mut moved = 0;
    for unit in scenario
        .units
        .iter_mut()
        .filter(|unit| unit.player == 0 && unit.kind == UnitKind::Sentinel)
    {
        if unit.x == 31 && unit.y == 18 {
            continue; // preserves legal knowledge of the final Foundry
        }
        unit.x = first_target.x - 1 + moved % 4;
        unit.y = first_target.y - 1 + moved / 4;
        moved += 1;
    }
    let cleared = scenario.build().unwrap();
    let mut value = serde_json::to_value(cleared).unwrap();
    value["tick"] = serde_json::json!(state.current_tick() + 1);
    let cleared: oxide_sim::State = serde_json::from_value(value).unwrap();
    let commands = gym.step_plan(&cleared, ActionPlan::default());
    assert!(
        commands.iter().any(|command| matches!(
            command.command,
            Command::AttackMove { goal, .. } if goal == final_target
        )),
        "arrival at a cleared target must stage and chain to the next known legal site: {commands:?}"
    );
}

#[test]
fn finish_commitment_cannot_read_a_hidden_enemy_army() {
    let visible = at_passive_victim_tick(&passive_victim_scenario(0));
    let hidden = at_passive_victim_tick(&passive_victim_scenario(12));
    let commands =
        |state: &oxide_sim::State| GymBot::new(PlayerId(0)).step_plan(state, ActionPlan::default());
    assert_eq!(
        commands(&visible),
        commands(&hidden),
        "units outside vision, radar, and remembered contact cannot affect tactical reconciliation"
    );
}

#[test]
fn finish_commitment_refuses_a_visible_disadvantage() {
    let mut scenario = passive_victim_scenario(0);
    for index in 0..16 {
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 28 + index % 4,
            y: 15 + index / 4,
        });
    }
    let state = at_passive_victim_tick(&scenario);
    let commands = GymBot::new(PlayerId(0)).step_plan(&state, ActionPlan::default());
    assert!(
        !commands.iter().any(|command| matches!(
            command.command,
            Command::AttackMove { .. } | Command::Attack { .. }
        )),
        "known superior defenders must keep a no-operation plan passive: {commands:?}"
    );
}

#[test]
fn shipped_basalt_spine_finishes_the_tick_27002_passive_victim() {
    let scenario = Scenario::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios/basalt-spine.json"),
    )
    .expect("shipped Basalt Spine");
    let mut state = scenario.build().expect("Basalt Spine builds");
    let mut bots = seat_bots(&scenario);
    assert_eq!(bots.len(), 1, "the shipped scenario has one bot commander");
    assert_eq!(bots[0].player(), PlayerId(1));
    let victim_foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .expect("the passive seat starts with a Foundry")
        .anchor;

    let mut saw_reproduction = false;
    let mut finish_commitment = None;
    let mut victory_tick = None;
    while state.current_tick() < 40_000 {
        if state.current_tick() == 27_002 {
            let attacking_fighters: Vec<_> = state
                .units()
                .iter()
                .filter(|unit| {
                    unit.player == PlayerId(1)
                        && unit
                            .kind
                            .stats()
                            .can_target(oxide_sim::stats::Domain::Ground)
                })
                .collect();
            let victim_fighters = state
                .units()
                .iter()
                .filter(|unit| {
                    unit.player == PlayerId(0)
                        && unit
                            .kind
                            .stats()
                            .can_target(oxide_sim::stats::Domain::Ground)
                })
                .count();
            assert!(
                state.result().is_none(),
                "the historical stall is still live"
            );
            assert!(
                attacking_fighters.len() >= 16
                    && attacking_fighters.len() >= victim_fighters.saturating_mul(4),
                "the neural seat must hold an unmistakable finishing advantage"
            );
            let committed = attacking_fighters
                .iter()
                .filter(|unit| {
                    matches!(unit.order, Order::Attack { .. } | Order::AttackMove { .. })
                })
                .count();
            let idle = attacking_fighters
                .iter()
                .filter(|unit| matches!(unit.order, Order::Idle))
                .count();
            assert!(
                committed < 5 && idle >= 16,
                "the fixture must still reproduce a dominant passive body at tick 27,002: {committed} committed, {idle} idle"
            );
            saw_reproduction = true;
        }

        let commands = bots[0].act(&state);
        if saw_reproduction && finish_commitment.is_none() {
            finish_commitment = commands.iter().find_map(|command| match &command.command {
                Command::AttackMove { units, goal, .. }
                    if units.len() >= 16 && goal.chebyshev(victim_foundry) <= 6 =>
                {
                    Some((state.current_tick(), units.len()))
                }
                _ => None,
            });
        }
        state.tick(&commands);
        if state.result().is_some() {
            victory_tick = Some(state.current_tick());
            break;
        }
    }

    assert!(
        saw_reproduction,
        "the run never reached the historical checkpoint"
    );
    assert!(
        finish_commitment.is_some(),
        "the dominant idle army never committed into the passive victim's base"
    );
    assert_eq!(state.result(), Some(GameResult::Victory { team: 1 }));
    assert!(
        victory_tick.is_some_and(|tick| tick < 40_000),
        "the real Basalt run remained stalled at the integration horizon"
    );
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
