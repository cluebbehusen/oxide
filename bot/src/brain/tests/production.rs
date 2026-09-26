use super::*;

#[test]
fn active_bulk_lift_spends_its_funded_bank_without_a_factory_tax() {
    let airworks_capacity = BuildingKind::Airworks
        .base_stats()
        .construction
        .expect("Airworks have a construction price")
        .cost
        .saturating_add(UnitKind::Sentinel.stats().cost);
    let carrier_cost = UnitKind::Skyhook.stats().cost;
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.players[0].scrap = airworks_capacity
        .saturating_add(UnitKind::Sentinel.stats().cost)
        .saturating_add(carrier_cost);
    let mut state = scenario
        .build()
        .expect("open-lane bulk-lift scenario builds");
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 17);
    let mut brain = scripted_brain(&scenario, PlayerId(0), config);
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let home = obs
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Foundry)
        .expect("the observation retains the home Foundry")
        .anchor;
    let lifts = &mut brain.mind_mut().lifts;
    let mut seed_obs = obs.clone();
    seed_obs.scrap = 0;
    let seeded = lifts.think_unrestricted(&seed_obs, home, &[], LiftAirSupport::Independent);
    assert!(lifts.operation().is_some());
    assert!(seeded.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Skyhook,
            ..
        }
    )));

    let result = brain.act_traced(&state);
    let carrier_commands = result
        .commands
        .iter()
        .filter(|command| {
            matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Skyhook,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        carrier_commands, 2,
        "the retained lift may use both planned queue slots without an unfunded factory tax: {:?}",
        result.trace
    );
    assert!(
        result.commands.iter().all(|command| !matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Airworks,
                ..
            }
        )),
        "factory investment must not seize the older operation's funded bank: commands={:?}, trace={:?}",
        result.commands,
        result.trace
    );
    let report = state.tick(&result.commands);
    assert_commands_accepted(&report, PlayerId(0));
}

#[test]
fn active_bulk_lift_retains_its_funding_through_full_queues() {
    let airworks_cost = BuildingKind::Airworks
        .base_stats()
        .construction
        .expect("Airworks have a construction price")
        .cost;
    let fighting_reserve = UnitKind::Sentinel.stats().cost;
    let queued_cost = 2 * UnitKind::Skyhook.stats().cost + 2 * UnitKind::Sentinel.stats().cost;
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.players[0].scrap = queued_cost + airworks_cost + fighting_reserve;
    let mut state = scenario
        .build()
        .expect("bulk-lift capacity scenario builds");
    let airworks = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Airworks)
        .expect("the authored Airworks stands")
        .id;
    let foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .expect("the home Foundry stands")
        .id;
    let queue_orders = [
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: airworks,
                kind: UnitKind::Skyhook,
            },
        },
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: airworks,
                kind: UnitKind::Skyhook,
            },
        },
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: foundry,
                kind: UnitKind::Sentinel,
            },
        },
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: foundry,
                kind: UnitKind::Sentinel,
            },
        },
    ];
    let queued = state.tick(&queue_orders);
    assert!(
        queued.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "the setup queues must be legal: {:?}",
        queued.events
    );

    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 17);
    let mut brain = scripted_brain(&scenario, PlayerId(0), config);
    while !crate::difficulty::strategic_admission_tick(state.current_tick()) {
        state.tick(&[]);
    }
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let home = obs
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Foundry)
        .expect("the observation retains the home Foundry")
        .anchor;
    let queues: Vec<_> = obs
        .my_buildings
        .iter()
        .zip(&obs.my_queues)
        .filter(|(building, _)| {
            matches!(
                building.kind,
                BuildingKind::Foundry | BuildingKind::Airworks
            )
        })
        .map(|(building, queue)| (building.kind, queue.len()))
        .collect();
    assert!(
        queues.iter().all(|(_, depth)| *depth == 2),
        "both ordinary production queues begin at the planning depth: {queues:?}"
    );
    assert_eq!(
        obs.scrap,
        airworks_cost + fighting_reserve,
        "only the exact extra-Airworks fund remains"
    );

    let lifts = &mut brain.mind_mut().lifts;
    let seeded = lifts.think_unrestricted(&obs, home, &[], LiftAirSupport::Independent);
    let operation = lifts
        .operation()
        .expect("the severed enclave starts a lift");
    assert_eq!(operation.phase, LiftPhase::Provision);
    assert!(
        operation.desired_carriers >= 8,
        "the fixture is a bulk lift"
    );
    assert!(
        lifts.remaining_airwork_ticks(&obs, &[]) > 2_400,
        "the active wave needs more than one Airworks' assembly horizon"
    );
    assert!(
        seeded.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                building,
                ..
            } if *building == airworks
        )),
        "the full Airworks queue cannot accept another planned order"
    );

    let result = brain.act_traced(&state);
    let commands = result.commands;
    assert!(
        commands.iter().all(|command| !matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Airworks,
                ..
            }
        )),
        "a full queue must not redirect the retained operation's capital to an unadmitted factory: {commands:?}; trace={:?}",
        result.trace
    );
    assert!(
        commands.iter().all(|command| !matches!(
            command.command,
            Command::Train { building, .. } if building == airworks
        )),
        "the already-full Airworks must not consume the held construction fund: {commands:?}"
    );
    let report = state.tick(&commands);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "strategic reservations and residual utility spending must lower through one legal bank: {:?}",
        report.events
    );
}

#[test]
fn connected_air_and_lift_share_capacity_without_a_residual_factory_tax() {
    let mut scenario = combined_operation_scenario();
    scenario
        .buildings
        .last_mut()
        .expect("the fixture has a reachable enemy structure")
        .kind = BuildingKind::Crucible;
    scenario.buildings.extend([
        building_spec(1, BuildingKind::Foundry, 10, 10),
        building_spec(1, BuildingKind::Airworks, 16, 4),
    ]);
    let mut state = scenario
        .build()
        .expect("combined-operation capacity scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    while !crate::difficulty::strategic_admission_tick(state.current_tick()) {
        state.tick(&[]);
    }
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);
    assert!(orientation.is_identity());
    let oriented = orientation.observe(&raw);
    let mut lift = LiftPlanner::new();
    let _ = lift.think_unrestricted(&oriented, home, &[], LiftAirSupport::Independent);
    let lift_airwork = lift.remaining_airwork_ticks(&oriented, &[]);
    assert!(lift.operation().is_some());
    assert!(lift_airwork > 2_400);

    let mut brain = operation_identity_brain(PlayerId(0), &scenario);
    brain.dials.minimum_core_equivalents = 0;
    let capacity_fund = BuildingKind::Airworks
        .base_stats()
        .construction
        .unwrap()
        .cost;
    enlist_opening_core(&mut brain, &state);
    brain.orientation = Some(orientation);
    brain.mind_mut().lifts = lift;

    let next_think = state.current_tick().saturating_add(brain.dials().cadence);
    while state.current_tick() < next_think {
        state.tick(&[]);
    }
    let spendable_after_capacity = UnitKind::Condor.stats().cost;
    crate::test_support::edit_player(&mut state, PlayerId(0), |item| {
        item.scrap = spendable_after_capacity + capacity_fund
    });

    let result = brain.act_traced(&state);
    let trace = result.trace.expect("the capacity decision is traced");
    let budget = trace.budget.expect("the ledger decision is traced");
    assert_eq!(budget.airworks_capacity, 0);
    assert!(
        trace.channels.connected_air.effects.committed_scrap <= budget.bank,
        "operation spending must still fit observed current capital"
    );
    let report = state.tick(&result.commands);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "{:?}",
        report.events
    );
}

#[test]
fn lost_connected_objective_releases_unpaid_demand_before_purchase() {
    let (mut state, mut brain, due, target) = connected_provider_due_next_cadence();

    let target_id = target
        .target_id
        .expect("the connected target was current at admission");
    let mut document = serde_json::to_value(&state).expect("the due state serializes");
    document["buildings"]
        .as_array_mut()
        .expect("buildings serialize as an array")
        .retain(|building| building["id"].as_u64() != Some(u64::from(target_id.0)));
    state = serde_json::from_value(document)
        .expect("removing the currently visible objective preserves state invariants");
    let target_visible = Observation::fog_honest(&state, PlayerId(0));
    let (target_width, target_height) = target.target_kind.base_stats().size;
    assert!(
        (0..target_height)
            .flat_map(|dy| (0..target_width).map(move |dx| target.target.offset(dx, dy)))
            .all(|tile| target_visible.visible(tile)),
        "current sight must prove that the accepted objective disappeared"
    );

    let recovery = brain.act_traced(&state);
    let recovery_trace = recovery
        .trace
        .as_ref()
        .expect("the due recovery decision is traced");
    assert!(recovery_trace.allocation.error.is_none());
    assert!(recovery_trace.allocation.coordinator_failure.is_none());
    assert_eq!(
        recovery_trace.connected_force.status,
        crate::trace::ConnectedForceStatus::Recovering(crate::AirRecoveryReason::ObjectiveLost,)
    );
    let accepted_due = recovery_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            job.producer == due.producer
                && job.kind == due.kind
                && job.enqueued_at == state.current_tick()
                && matches!(
                    job.owner,
                    crate::trace::ClaimOwnerTrace::Obligation {
                        class: crate::trace::ObligationClass::PersistentPlan,
                        key: crate::trace::ObligationKeyTrace::ConnectedOffense { .. },
                        ..
                    }
                )
        })
        .count();
    assert_eq!(
        accepted_due, 0,
        "an obsolete quotation must not buy a provider for a lost objective"
    );
    let emitted_due = recovery
        .commands
        .iter()
        .filter(|command| {
            matches!(
                command.command,
                Command::Train { building, kind }
                    if building == due.producer && kind == due.kind
            )
        })
        .count();
    assert_eq!(
        emitted_due, 0,
        "recovery must not turn an old quotation into a new purchase: {:?}",
        recovery.commands
    );

    let report = state.tick(&recovery.commands);
    assert_commands_accepted(&report, PlayerId(0));
    while !state.current_tick().is_multiple_of(brain.dials.cadence) {
        state.tick(&[]);
    }
    assert!(state.player(PlayerId(0)).scrap >= due.kind.stats().cost);

    let next = brain.act_traced(&state);
    let next_trace = next
        .trace
        .as_ref()
        .expect("the next-cadence recovery decision is traced");
    assert!(
        next_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .all(|job| {
                !(job.producer == due.producer
                    && job.kind == due.kind
                    && job.request_ordinal == due.request_ordinal)
            })
    );
    assert!(
        next.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train { building, kind }
                if building == due.producer && kind == due.kind
        )),
        "recovery must not retry an unpaid quotation on the next decision: {:?}",
        next.commands
    );
}

#[test]
fn harvester_recovery_cancels_a_due_connected_provider_and_recalls_its_force() {
    let (state, mut brain, due, _) = connected_provider_due_next_cadence();
    assert_ne!(due.kind, UnitKind::Harvester);

    let recovery_bank = UnitKind::Harvester
        .stats()
        .cost
        .saturating_add(due.kind.stats().cost);
    let mut document = serde_json::to_value(&state).expect("the due state serializes");
    document["players"][0]["scrap"] = serde_json::json!(recovery_bank);
    document["units"]
        .as_array_mut()
        .expect("state units serialize as an array")
        .retain(|unit| unit["kind"].as_str() != Some("harvester"));
    for building in document["buildings"]
        .as_array_mut()
        .expect("state buildings serialize as an array")
    {
        building["queue"]
            .as_array_mut()
            .expect("building queues serialize as arrays")
            .retain(|kind| kind.as_str() != Some("harvester"));
    }
    let mut state: State = serde_json::from_value(document)
        .expect("removing every Harvester preserves state invariants");

    let raw = Observation::fog_honest(&state, PlayerId(0));
    assert!(
        raw.my_units
            .iter()
            .all(|unit| unit.kind != UnitKind::Harvester)
            && raw
                .my_queues
                .iter()
                .flatten()
                .all(|kind| *kind != UnitKind::Harvester),
        "the due decision must begin with no live or queued Harvester"
    );
    let (foundry, home) = raw
        .my_buildings
        .iter()
        .enumerate()
        .filter(|(index, building)| {
            building.kind == BuildingKind::Foundry
                && building.built
                && raw.my_queues[*index].len() < oxide_sim::stats::QUEUE_CAP
        })
        .min_by_key(|(_, building)| building.id)
        .map(|(_, building)| (building.id, building.anchor))
        .expect("an affordable completed Foundry has a queue slot");
    assert!(raw.scrap >= UnitKind::Harvester.stats().cost);

    let orientation = brain
        .orientation
        .expect("the connected admission latched an orientation");
    assert!(orientation.is_identity());
    let oriented = orientation.observe(&raw);
    assert!(brain.mind().strategy.air_operation().is_some());
    assert_eq!(due.enqueued_at, state.current_tick());
    let operation = brain.mind().strategy.air_operation().unwrap();
    let reserved = operation
        .scout
        .into_iter()
        .chain(operation.artillery.iter().copied())
        .chain(operation.strike_aircraft.iter().copied())
        .collect::<Vec<_>>();
    assert!(!reserved.is_empty());
    let oriented_home = oriented
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry)
        .min_by_key(|building| building.id)
        .expect("the oriented observation retains the home Foundry")
        .anchor;
    let expected_returning =
        crate::navigation::commands::routable_command_subset_with_public_terrain_and_orientation(
            crate::query_work::QueryPurpose::NavigationTest,
            &oriented,
            brain
                .mind()
                .oriented_public_map
                .as_ref()
                .expect("the admission oriented the public map"),
            &reserved,
            oriented_home,
            orientation,
        );
    assert_eq!(
        expected_returning, reserved,
        "the fixture keeps every reserved operation member routable to home"
    );

    let recovery = brain.act_traced(&state);
    let trace = recovery
        .trace
        .as_ref()
        .expect("the emergency decision is traced");
    assert_eq!(trace.control_flow, DecisionControlFlow::HarvesterRecovery);
    assert_eq!(
        recovery
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train { building, kind: UnitKind::Harvester }
                    if building == foundry
            ))
            .count(),
        1,
        "emergency recovery must emit exactly one affordable Harvester: {:?}",
        recovery.commands
    );
    assert!(
        recovery.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train { building, kind }
                if building == due.producer && kind == due.kind
        )),
        "the due connected provider must yield to the Harvester: {:?}",
        recovery.commands
    );
    let return_orders = recovery
        .commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Move { units, goal, queue }
                if *goal == home && !*queue && units.iter().any(|unit| reserved.contains(unit)) =>
            {
                Some(units.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        return_orders,
        vec![expected_returning],
        "every routable reserved member must receive one shared return-home order"
    );
    let operation = (brain.mind().strategy)
        .air_operation()
        .expect("the failed connected operation remains visible during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(crate::strategy::AirRecoveryReason::PreparationInfeasible)
    );

    let report = state.tick(&recovery.commands);
    assert_commands_accepted(&report, PlayerId(0));
    while !state.current_tick().is_multiple_of(brain.dials.cadence) {
        state.tick(&[]);
    }
    let next = brain.act_traced(&state);
    assert!(
        next.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train { building, kind }
                if building == due.producer && kind == due.kind
        )),
        "the canceled provider must not be retried on the next cadence: {:?}",
        next.commands
    );
}

#[test]
fn fresh_island_admission_accounts_for_an_active_lifts_airworks_prefix() {
    use crate::trace::{ClaimOwnerTrace, ObligationKeyTrace};

    let mut scenario = foundry_saving_lift_competition_scenario(50_000);
    scenario.name = "fresh island admission follows active lift production".into();
    let mut state = scenario
        .build()
        .expect("the active-lift island scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    let airworks = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Airworks)
        .expect("the fixture has one Airworks")
        .id;
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);
    let lift = seeded_bulk_lift(&state, orientation);

    let mut baseline_brain = foundry_competition_brain(&scenario);

    baseline_brain.dials.minimum_core_equivalents = 0;
    baseline_brain.orientation = Some(orientation);

    let baseline = baseline_brain.act_traced(&state);
    assert!(baseline.trace.as_ref().is_some_and(|trace| {
        trace.allocation.error.is_none() && trace.allocation.coordinator_failure.is_none()
    }));
    let baseline_timeout = (baseline_brain.mind().strategy)
        .air_assembly_timeout()
        .expect("the unprefixed control admits the wealthy-island operation");

    let mut brain = foundry_competition_brain(&scenario);

    brain.dials.minimum_core_equivalents = 0;
    brain.orientation = Some(orientation);
    assert!(brain.mind().strategy.air_operation().is_none());
    brain.mind_mut().lifts = lift;

    let act = brain.act_traced(&state);
    let trace = act
        .trace
        .as_ref()
        .expect("the active-lift island admission is traced");
    assert!(
        trace.allocation.error.is_none() && trace.allocation.coordinator_failure.is_none(),
        "the active lift must settle before the fresh island admission: {trace:#?}"
    );
    let accepted_lift_prefix = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            job.enqueued_at == state.current_tick()
                && matches!(
                    job.owner,
                    ClaimOwnerTrace::Obligation {
                        key: ObligationKeyTrace::LiftPurchases,
                        ..
                    }
                )
        })
        .map(|job| (job.producer, job.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        accepted_lift_prefix,
        vec![(airworks, UnitKind::Skyhook)],
        "allocation must own the active lift's exact current producer prefix"
    );

    let airworks_training = act
        .commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Train { building, kind } if building == airworks => Some(kind),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        airworks_training,
        vec![UnitKind::Skyhook, UnitKind::Buzzard],
        "the fresh island plan must see the accepted lift prefix and use only the remaining shallow slot"
    );
    assert!(
        (brain.mind().strategy)
            .air_operation()
            .is_some_and(|operation| {
                operation.assault_admitted() && operation.phase() == AirOperationPhase::Recon
            })
    );
    let prefixed_timeout = (brain.mind().strategy)
        .air_assembly_timeout()
        .expect("the fresh island operation owns an assembly timeout");
    assert_eq!(
        prefixed_timeout,
        baseline_timeout.saturating_add(u64::from(UnitKind::Skyhook.stats().train_ticks)),
        "the island assembly window must cover the allocator-accepted FIFO prefix"
    );

    let report = state.tick(&act.commands);
    assert_commands_accepted(&report, PlayerId(0));
    assert_eq!(
        state
            .building(airworks)
            .expect("the shared Airworks remains live")
            .queue
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![UnitKind::Skyhook, UnitKind::Buzzard]
    );
}

#[test]
fn active_lift_and_island_share_the_last_shallow_airworks_slot_without_starvation() {
    let mut scenario = foundry_saving_lift_competition_scenario(50_000);
    scenario.name = "active lift and island share one Airworks slot".into();
    let mut state = scenario
        .build()
        .expect("the shared-Airworks scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    let airworks = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Airworks)
        .expect("the fixture has one Airworks")
        .id;
    crate::test_support::edit_building(&mut state, airworks, |building| {
        building.queue.push_back(UnitKind::Kestrel);
    });

    let mut brain = foundry_competition_brain(&scenario);

    brain.dials.minimum_core_equivalents = 0;
    let profile = *brain.profile();
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);
    let observed = orientation.observe(&raw);
    let public_map = orientation.briefing(
        &PublicMapBriefing::from_scenario(&scenario)
            .expect("the island fixture has a public briefing"),
    );
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observed);

    let mut strategy = StrategicPlanner::new();
    let island_admission = strategy.think_after_connected_adjudication(StrategicThinkContext::new(
        &profile,
        tuning,
        &observed,
        &intelligence,
        home,
        StrategicCoordination {
            planning: Some(&crate::planning::PlanningWork::default()),
            enlisted: &[],
            lift_support: None,
            allow_new_operation: true,
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
            public_map: Some(&public_map),
            orientation,
        },
    ));
    let island_train = island_admission
        .decision
        .intents
        .iter()
        .find_map(|intent| match intent {
            Intent::TrainAt { building, kind } if *building == airworks => Some(*kind),
            _ => None,
        })
        .expect("the admitted island operation needs the last shallow Airworks slot");
    assert!(strategy.air_operation().is_some_and(|operation| {
        operation.assault_admitted() && operation.phase() == AirOperationPhase::Recon
    }));
    assert!(strategy.connected_deadline().is_none());
    let island_airwork = strategy.remaining_airwork_ticks(&observed, None);
    assert!(island_airwork > 0);

    let mut lift = LiftPlanner::new();
    let lift_admission = lift.think_with_admission_and_producer_lanes(
        &observed,
        home,
        &[],
        LiftAirSupport::Independent,
        LiftAdmission {
            allow_new_commitments: true,
            spendable_scrap: observed.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 0,
        },
        crate::resources::ProducerLaneReservations::empty(),
    );
    assert!(lift_admission.intents.contains(&Intent::TrainAt {
        building: airworks,
        kind: UnitKind::Skyhook,
    }));
    assert_eq!(
        strategy.air_admitted_at(),
        lift.operation().map(|operation| operation.started_at),
        "equal admission ticks exercise the historical island-before-lift tie-break"
    );

    let mind = brain.mind_mut();
    mind.intelligence = intelligence;
    mind.strategy = strategy;
    mind.lifts = lift;
    brain.orientation = Some(orientation);

    {
        use crate::lift::{LiftProducerAssignment, LiftProducerFunding, LiftProducerTiming};

        let mut cancelled = brain.clone();
        let operation = cancelled.mind_mut().lifts.operation().unwrap().clone();
        let starts_at = observed.tick + u64::from(UnitKind::Kestrel.stats().train_ticks);
        cancelled
            .mind_mut()
            .lifts
            .bind_producers(vec![LiftProducerAssignment::new(
                0,
                airworks,
                UnitKind::Skyhook,
                LiftProducerTiming::new(
                    observed.tick + tuning.cadence,
                    starts_at,
                    starts_at + u64::from(UnitKind::Skyhook.stats().train_ticks) - 1,
                    operation.deadline,
                ),
                LiftProducerFunding::new(UnitKind::Skyhook.stats().cost, 0),
            )]);
        assert!(
            cancelled
                .mind_mut()
                .lifts
                .active_production_obligation()
                .unwrap()
                .producer_schedule_is_executable(
                    &ResourceSnapshot::from_observation(&observed),
                    tuning.cadence,
                    observed.tick,
                )
        );
        let mut cancelled_state = state.clone();
        crate::test_support::edit_building(&mut cancelled_state, airworks, |building| {
            building.queue.clear();
        });
        let recovered = cancelled.act_traced(&cancelled_state);
        let trace = recovered.trace.unwrap();
        assert!(
            !trace.budget.as_ref().unwrap().frozen
                && trace.allocation.coordinator_failure.is_none(),
            "cancelling a fixed booking's predecessor must preserve Lift recovery through earlier island preparation: {trace:#?}"
        );
        assert!(
            cancelled
                .mind_mut()
                .lifts
                .active_production_obligation()
                .is_none()
        );
    }

    let first = brain.act_traced(&state);
    let first_trace = first.trace.as_ref().expect("the shared turn is traced");
    assert!(
        first_trace.allocation.error.is_none()
            && first_trace.allocation.coordinator_failure.is_none(),
        "the two active planners must share producer capacity without losing accepted progress: {first_trace:#?}"
    );
    assert_eq!(
        first
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train { building, kind }
                    if building == airworks && kind == island_train
            ))
            .count(),
        1,
        "the equal-tick priority winner owns the one immediately available slot: {:?}",
        first.commands
    );
    assert!(first.commands.iter().all(|command| !matches!(
        command.command,
        Command::Train { building, kind }
            if building == airworks && kind == UnitKind::Skyhook
    )));
    let report = state.tick(&first.commands);
    assert_commands_accepted(&report, PlayerId(0));

    crate::test_support::edit_building(&mut state, airworks, |building| {
        assert_eq!(
            building.queue.iter().copied().collect::<Vec<_>>(),
            vec![UnitKind::Kestrel, island_train]
        );
    });
    let liveness_deadline = state
        .current_tick()
        .saturating_add(u64::from(UnitKind::Kestrel.stats().train_ticks))
        .saturating_add(island_airwork)
        .saturating_add(brain.dials.cadence.saturating_mul(2));
    let mut lift_progressed = false;
    while state.current_tick() <= liveness_deadline {
        let next = brain.act_traced(&state);
        if let Some(trace) = next.trace.as_ref() {
            assert!(
                trace.allocation.error.is_none() && trace.allocation.coordinator_failure.is_none(),
                "shared Airworks pressure must preserve both active planners: {trace:#?}"
            );
        }
        lift_progressed = next.commands.iter().any(|command| {
            matches!(
                command.command,
                Command::Train { building, kind }
                    if building == airworks && kind == UnitKind::Skyhook
            )
        });
        let report = state.tick(&next.commands);
        assert_commands_accepted(&report, PlayerId(0));
        if lift_progressed {
            break;
        }
    }
    assert!(
        lift_progressed,
        "the lower-priority lift must claim Airworks capacity after the older island operation finishes its retained airwork"
    );
}

#[test]
fn admitted_island_air_trains_before_a_fresh_foundry_without_being_thought_twice() {
    use crate::trace::{ClaimOwnerTrace, ObligationKeyTrace, ProducerJobAccessTrace};

    let mut scenario = foundry_saving_air_competition_scenario(50_000);
    scenario.name = "admitted island air precedes a fresh Foundry".into();
    for row in scenario.map.iter_mut().skip(1).take(22) {
        let mut bytes = row.as_bytes().to_vec();
        bytes[38] = b'^';
        *row = String::from_utf8(bytes).expect("the fixture map remains ASCII");
    }
    let scout = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the island fixture has one scout");
    (scout.x, scout.y) = (42, 19);
    let mut state = scenario
        .build()
        .expect("the island allocation scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);

    let mut brain = foundry_competition_brain(&scenario);
    let profile = *brain.profile();
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);
    let observed = orientation.observe(&raw);
    let public_map = orientation.briefing(
        &PublicMapBriefing::from_scenario(&scenario)
            .expect("the island fixture has a public briefing"),
    );
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observed);
    let mut planner = StrategicPlanner::new();
    let admission = planner.think_after_connected_adjudication(StrategicThinkContext::new(
        &profile,
        tuning,
        &observed,
        &intelligence,
        home,
        StrategicCoordination {
            planning: Some(&crate::planning::PlanningWork::default()),
            enlisted: &[],
            lift_support: None,
            allow_new_operation: true,
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
            public_map: Some(&public_map),
            orientation,
        },
    ));
    assert!(planner.air_operation().is_some_and(|operation| {
        operation.assault_admitted() && operation.phase() == AirOperationPhase::Recon
    }));
    assert!(planner.connected_deadline().is_none());
    assert!(admission.decision.intents.iter().any(|intent| matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Buzzard,
            ..
        }
    )));
    let admitted_at = planner
        .air_admitted_at()
        .expect("the island operation has an immutable priority tick");

    let decision_tick = crate::difficulty::strategic_admission_at_or_after(
        admitted_at.saturating_add(tuning.reaction_delay),
    );
    crate::test_support::set_tick(&mut state, decision_tick);
    crate::test_support::edit_player(&mut state, PlayerId(0), |item| {
        item.scrap = UnitKind::Buzzard
            .stats()
            .cost
            .saturating_add(UnitKind::Sentinel.stats().cost)
    });
    let mind = brain.mind_mut();
    mind.intelligence = intelligence;
    mind.strategy = planner;
    brain.orientation = Some(orientation);

    let act = brain.act_traced(&state);
    let training = act
        .commands
        .iter()
        .filter(|command| {
            matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Buzzard,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        training, 1,
        "the due island append lowers exactly once: {:?}",
        act.commands
    );
    assert!(act.commands.iter().all(|command| !matches!(
        command.command,
        Command::Build {
            kind: BuildingKind::Foundry,
            ..
        }
    )));
    let operation = (brain.mind().strategy)
        .air_operation()
        .expect("the island operation remains active");
    assert_eq!(operation.phase(), AirOperationPhase::Assemble);
    assert_eq!(operation.phase_started_at, decision_tick);
    let operation_scout = operation
        .scout
        .expect("the admitted island operation retains its scout");

    let trace = act.trace.expect("the allocation decision is traced");
    assert!(trace.allocation.proposals.entries.iter().any(|proposal| {
        matches!(
            proposal.key,
            crate::trace::ProposalKeyTrace::FoundryExpansion { .. }
        )
    }));
    let strategic_air = trace
        .allocation
        .obligations
        .entries
        .iter()
        .find(|obligation| matches!(obligation.key, ObligationKeyTrace::AirPurchases))
        .expect("the due island decision is an explicit prior obligation");
    assert_eq!(strategic_air.accepted_at, admitted_at);
    let job = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .find(|job| {
            matches!(
                job.owner,
                ClaimOwnerTrace::Obligation {
                    accepted_at,
                    key: ObligationKeyTrace::AirPurchases,
                    ..
                } if accepted_at == admitted_at
            )
        })
        .expect("the island operation retains its exact producer assignment");
    assert_eq!(job.enqueued_at, decision_tick);
    assert!(matches!(
        strategic_air.claims.producer_jobs.entries[0].access,
        ProducerJobAccessTrace::Fixed { enqueued_at, .. } if enqueued_at == decision_tick
    ));

    let report = state.tick(&act.commands);
    assert_commands_accepted(&report, PlayerId(0));

    let mut after_scout_loss = Observation::fog_honest(&state, PlayerId(0));
    after_scout_loss
        .my_units
        .retain(|unit| unit.id != operation_scout);
    after_scout_loss.tick = decision_tick.saturating_add(brain.dials.cadence);
    let after_loss = crate::Brain::act_traced(&mut brain, &after_scout_loss);
    let after_loss_trace = after_loss
        .trace
        .expect("the post-loss island decision is traced");
    assert!(
        after_loss_trace.allocation.error.is_none()
            && after_loss_trace.allocation.coordinator_failure.is_none(),
        "a vanished planner member must not become a permanent unknown-unit obligation: {after_loss_trace:#?}"
    );
    assert!(
        after_loss_trace.budget.is_some_and(|budget| !budget.frozen),
        "the remaining domains must keep making progress after the loss"
    );
    assert!(
        (brain.mind().strategy)
            .air_operation()
            .is_none_or(|operation| operation.scout != Some(operation_scout)),
        "the staged planner must release or replace the lost scout"
    );
}

pub(super) fn connected_provider_due_next_cadence() -> (
    State,
    SeatBot,
    crate::trace::ScheduledProducerJobTrace,
    crate::strategy::AirOperation,
) {
    let mut scenario = foundry_saving_air_competition_scenario(
        UnitKind::Buzzard
            .stats()
            .cost
            .saturating_add(UnitKind::Lancer.stats().cost)
            .saturating_add(
                BuildingKind::Turret
                    .base_stats()
                    .construction
                    .expect("Turrets are constructible")
                    .cost,
            )
            .saturating_add(UnitKind::Sentinel.stats().cost),
    );
    scenario
        .buildings
        .retain(|building| building.kind != BuildingKind::Extractor);
    scenario.map = scenario
        .map
        .iter()
        .map(|row| row.replace('E', "."))
        .collect();
    scenario.name = "connected procurement at the next decision".into();
    scenario
        .buildings
        .push(building_spec(1, BuildingKind::Foundry, 40, 2));
    let scout = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the scenario has one connected-operation scout");
    (scout.x, scout.y) = (42, 19);
    scenario.units.push(unit_spec(0, UnitKind::Bombard, 9, 17));
    let mut state = scenario
        .build()
        .expect("the delayed connected-provider scenario builds");
    let airworks = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Airworks)
        .expect("the scenario has one Airworks")
        .id;
    crate::test_support::edit_building(&mut state, airworks, |airworks_state| {
        airworks_state.queue.extend(core::iter::repeat_n(
            UnitKind::Kestrel,
            oxide_sim::stats::QUEUE_CAP,
        ));
        airworks_state.progress = UnitKind::Kestrel.stats().train_ticks - 1;
    });

    let mut brain = foundry_competition_brain(&scenario);

    let admission = brain.act_traced(&state);
    let admission_trace = admission
        .trace
        .as_ref()
        .expect("the connected admission is traced");
    assert!(admission_trace.allocation.error.is_none());
    assert!(admission_trace.allocation.coordinator_failure.is_none());
    assert!(
        admission_trace
            .allocation
            .proposals
            .entries
            .iter()
            .any(|proposal| {
                proposal.disposition == crate::trace::ProposalDispositionTrace::Accepted
                    && matches!(proposal.key, crate::trace::ProposalKeyTrace::Defense { .. })
            })
    );
    let target = (brain.mind().strategy)
        .air_operation()
        .expect("the connected operation is admitted")
        .clone();
    let due = admission_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                crate::trace::ClaimOwnerTrace::Proposal {
                    key: crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                }
            )
        })
        .min_by_key(|job| job.enqueued_at)
        .cloned()
        .unwrap_or_else(|| {
            panic!("the full queue defers connected procurement: {admission_trace:#?}")
        });
    assert_eq!(
        due.enqueued_at, brain.dials.cadence,
        "the nearly complete front item should expose one slot at the next decision"
    );

    let report = state.tick(&admission.commands);
    assert_commands_accepted(&report, PlayerId(0));
    while state.current_tick() < due.enqueued_at {
        state.tick(&[]);
    }

    (state, brain, due, target)
}
