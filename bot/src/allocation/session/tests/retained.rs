use super::*;

#[test]
fn retained_lift_recovery_respects_conflict_owner_and_foundry_admission() {
    for foundry_accepted_at in [0, 24, 48] {
        let (mut observation, _, _) = active_lift_fixture();
        observation.tick = 24;
        let mut lift = LiftPlanner::new();
        lift.think_with_admission_and_producer_lanes(
            &observation,
            TilePos::new(5, 15),
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: true,
                spendable_scrap: observation.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 5,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        let operation = lift.operation().unwrap().clone();
        assert_eq!(operation.started_at, 24);
        let enqueued_at = 72;
        lift.prepare_producer_binding(
            operation.started_at,
            operation.deadline,
            vec![LiftProducerAssignment::new(
                0,
                BuildingId(2),
                UnitKind::Skyhook,
                LiftProducerTiming::new(
                    enqueued_at,
                    enqueued_at,
                    enqueued_at + Tick::from(UnitKind::Skyhook.stats().train_ticks) - 1,
                    operation.deadline,
                ),
                LiftProducerFunding::new(0, UnitKind::Skyhook.stats().cost),
            )],
        )
        .unwrap()
        .apply(&mut lift);
        let active = lift.active_production_obligation().unwrap();
        let production = active_lift_production_obligation(&active).unwrap();
        let builder = UnitId(200);
        observation.my_units.push(owned_unit(
            builder.0,
            UnitKind::Harvester,
            TilePos::new(9, 22),
        ));
        observation.tick = 60;
        let cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .unwrap()
            .cost;
        observation.scrap = cost;
        let resources = ResourceSnapshot::from_observation(&observation);
        let mut policy = UtilityPolicy::new();
        policy
            .prepare_adjudicated_foundry(
                FreshFoundryProposal::fixture(
                    TilePos::new(10, 22),
                    builder,
                    cost,
                    0,
                    0,
                    operation.deadline,
                    foundry_case(),
                ),
                foundry_accepted_at,
            )
            .unwrap()
            .apply(&mut policy, &mut Vec::new());
        let foundry = policy
            .validated_foundry_obligation(&observation, &resources, true, cost)
            .unwrap();
        let protected = saved_foundry_obligation(foundry).unwrap();
        let mut proof = CrossDomainAllocation::new(&resources, operation.deadline, 12).unwrap();
        proof.import(protected.clone());
        proof.import(production.clone());
        let expected_owner = if foundry_accepted_at > operation.started_at {
            protected.owner()
        } else {
            production.owner()
        };
        let result = proof.resolve(AllocationPersonality::default(), None);
        assert!(
            matches!(&result,
            Err(AllocationError::ObligationConflict { obligation, .. }) if *obligation == expected_owner),
            "{:?}",
            result.err()
        );
        let mut obligations = ObligationPreparation {
            resources,
            obligations: vec![protected.clone(), production.clone()],
            coordinator_failure: None,
            active_connected: None,
            active_lift: Some(active.clone()),
            invalid_active_connected: false,
            invalid_active_lift: false,
            retained_air_claims: None,
            island_preparation: None,
        };
        let mut saved = SavedFoundryPreparation {
            obligation: Some(foundry),
            saving: cost,
            blocked: false,
            preparation_need: None,
        };
        let setup = SessionProfile::new(prime_profile());
        let briefing = connected_briefing(&observation);
        let intelligence = StrategicIntelligence::new();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = lift;
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();

        let mut session = AllocationSession::new(
            setup.context(&observation, TilePos::new(5, 15), &briefing, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(),
            None,
        );
        session
            .retained_work()
            .reconcile_lift_funding_after_saved_capital(&mut saved, &mut obligations);
        let lift = &session.participants.lifts;
        assert_eq!(lift.operation().unwrap().payload, operation.payload);
        assert_eq!(lift.operation().unwrap().deadline, operation.deadline);
        if foundry_accepted_at < operation.started_at {
            assert_eq!(obligations.obligations.len(), 1);
            assert_eq!(saved.obligation, Some(foundry));
            assert_eq!(obligations.obligations[0].owner(), protected.owner());
            assert!(obligations.active_lift.is_none());
            assert_eq!(lift.operation().unwrap().phase, LiftPhase::Recover);
        } else if foundry_accepted_at == operation.started_at {
            assert_eq!(obligations.obligations.len(), 1);
            assert!(saved.obligation.is_none());
            assert_eq!(obligations.obligations[0], production);
            assert_eq!(obligations.active_lift, Some(active));
            assert_eq!(lift.operation().unwrap().phase, operation.phase);
        } else {
            assert_eq!(saved.obligation, Some(foundry));
            assert_eq!(obligations.obligations, vec![protected, production]);
            assert_eq!(obligations.active_lift, Some(active));
            assert_eq!(lift.operation().unwrap().phase, operation.phase);
        }
    }
}

#[test]
fn lost_lift_payload_discards_unpaid_work_before_committing_other_owners() {
    let (mut observation, mut lift, _) = active_lift_fixture();
    let operation = lift.operation().unwrap();
    let accepted_at = operation.started_at;
    let deadline = operation.deadline;
    lift.prepare_producer_binding(
        accepted_at,
        deadline,
        vec![LiftProducerAssignment::new(
            0,
            BuildingId(2),
            UnitKind::Skyhook,
            LiftProducerTiming::new(
                24,
                24,
                24 + Tick::from(UnitKind::Skyhook.stats().train_ticks) - 1,
                deadline,
            ),
            LiftProducerFunding::new(0, UnitKind::Skyhook.stats().cost),
        )],
    )
    .unwrap()
    .apply(&mut lift);
    observation.tick = 12;
    observation
        .my_units
        .retain(|unit| unit.kind.stats().harvest.is_some());
    let (trace, outcome) = allocation_run_for(&observation, StrategicPlanner::new(), lift);
    assert!(outcome.allocation_ok);
    assert!(trace.coordinator_failure.is_none());
    assert!(
        outcome
            .allocated_producer_intents
            .iter()
            .all(|intent| !matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Skyhook,
                    ..
                }
            ))
    );
}

#[test]
fn unfundable_retained_lift_recovers_without_releasing_members() {
    let (mut observation, mut lift, _) = active_lift_fixture();
    observation.tick = 12;
    observation.scrap = 300;
    let operation = lift.operation().unwrap().clone();
    let enqueued_at = 24;
    lift.prepare_producer_binding(
        operation.started_at,
        operation.deadline,
        vec![LiftProducerAssignment::new(
            0,
            BuildingId(2),
            UnitKind::Skyhook,
            LiftProducerTiming::new(
                enqueued_at,
                enqueued_at,
                enqueued_at + Tick::from(UnitKind::Skyhook.stats().train_ticks) - 1,
                operation.deadline,
            ),
            LiftProducerFunding::new(0, UnitKind::Skyhook.stats().cost),
        )],
    )
    .unwrap()
    .apply(&mut lift);
    let members = lift.operation().unwrap().payload.clone();
    let active = lift.active_production_obligation().unwrap();
    let production = active_lift_production_obligation(&active).unwrap();
    let protected = imported_obligation(
        ObligationClass::PersistentPlan,
        0,
        ObligationKey::SavedFoundry {
            anchor: TilePos::new(10, 22),
        },
        ClaimBundle::new(300, vec![], vec![], vec![], vec![], vec![]).unwrap(),
    );
    let resources = ResourceSnapshot::from_observation(&observation);
    let mut proof = CrossDomainAllocation::new(&resources, operation.deadline, 12).unwrap();
    proof.import(protected.clone());
    proof.import(production.clone());
    assert!(
        matches!(proof.resolve(AllocationPersonality::default(), None),
            Err(AllocationError::ObligationConflict { obligation, .. }) if obligation == production.owner())
    );
    let mut obligations = ObligationPreparation {
        resources,
        obligations: vec![protected.clone(), production],
        coordinator_failure: None,
        active_connected: None,
        active_lift: Some(active),
        invalid_active_connected: false,
        invalid_active_lift: false,
        retained_air_claims: None,
        island_preparation: None,
    };
    let setup = SessionProfile::new(prime_profile());
    let briefing = connected_briefing(&observation);
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observation);
    let mut policy = UtilityPolicy::new();
    let mut strategy = StrategicPlanner::new();
    let mut lifts = lift;
    let mut team = TeamReliefPlanner::new();
    let mut raids = RaidPlanner::new();

    let mut session = AllocationSession::new(
        setup.context(&observation, TilePos::new(5, 15), &briefing, &intelligence),
        AllocationParticipants {
            policy: &mut policy,
            strategy: &mut strategy,
            lifts: &mut lifts,
            team: &mut team,
            raids: &mut raids,
        },
        advanced(),
        None,
    );
    session
        .retained_work()
        .reconcile_lift_funding_after_saved_capital(
            &mut SavedFoundryPreparation {
                obligation: None,
                saving: 0,
                blocked: false,
                preparation_need: None,
            },
            &mut obligations,
        );
    assert!(obligations.active_lift.is_none());
    assert_eq!(obligations.obligations.len(), 1);
    assert_eq!(obligations.obligations[0].owner(), protected.owner());
    let lift = &session.participants.lifts;
    assert_eq!(lift.operation().unwrap().phase, LiftPhase::Recover);
    assert_eq!(lift.operation().unwrap().payload, members);
    let mut allocation =
        CrossDomainAllocation::new(&obligations.resources, operation.deadline, 12).unwrap();
    allocation.import(protected);
    assert!(
        allocation
            .resolve(AllocationPersonality::default(), None)
            .is_ok()
    );
}

#[test]
fn active_island_producer_context_honors_an_older_saved_foundry_deadline() {
    let observation = connected_observation(120, 1_000);
    let resources = ResourceSnapshot::from_observation(&observation);
    let cadence = 12;
    let producer = BuildingId(12);
    let kind = UnitKind::Kestrel;
    let mut lane = resources
        .planning_projection(observation.tick.saturating_add(1_000), cadence)
        .expect("the test horizon is bounded")
        .producer(producer)
        .expect("the fixture has one Airworks")
        .clone();
    let timing = lane
        .append(kind, observation.tick)
        .expect("the Airworks can accept the island scout immediately");
    let producer_deadline = timing.ready_at.saturating_add(1);
    let foundry_deadline = producer_deadline.saturating_add(600);
    let saved_foundry = imported_obligation(
        ObligationClass::PersistentPlan,
        observation.tick.saturating_sub(2),
        ObligationKey::SavedFoundry {
            anchor: TilePos::new(14, 14),
        },
        ClaimBundle::new(
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect("the saved Foundry claims are valid")
        .with_deferrable_capital(DeferrableCapitalClaim {
            through: foundry_deadline,
            amount: 100,
        })
        .expect("the saved Foundry has one bounded capital claim"),
    );
    let active_island = imported_obligation(
        ObligationClass::PersistentPlan,
        observation.tick.saturating_sub(1),
        ObligationKey::AirPurchases,
        ClaimBundle::new(
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![ProducerJobClaim::fixed(
                producer,
                kind,
                observation.tick,
                timing.starts_at,
                timing.ready_at,
                producer_deadline,
            )],
        )
        .expect("the active island producer claim is valid"),
    );

    let (_, due) = retained_producer_context(
        &resources,
        &[saved_foundry, active_island],
        cadence,
        observation.tick,
        &crate::planning::PlanningWork::default(),
    )
    .expect("the island lane and older Foundry horizon must settle together");

    assert_eq!(
        due,
        vec![Intent::TrainAt {
            building: producer,
            kind,
        }]
    );
}

#[test]
fn active_revision_can_reuse_its_own_proposed_arrival() {
    let mut obs = connected_observation(1_200, 10_000);
    let (mut strategy, jobs) = current_connected_planner(&obs);
    let arrived = UnitId(900);
    let kind = jobs
        .first()
        .expect("the admitted force needs a provider")
        .kind();
    obs.tick += 12;
    obs.my_units
        .push(owned_unit(arrived.0, kind, TilePos::new(6, 10)));
    obs.my_units.sort_unstable_by_key(|unit| unit.id);
    let before = strategy.clone();
    let proposed = connected_obligation(&mut strategy, &obs);
    assert!(proposed.units().contains(&arrived));
    assert_eq!(strategy, before, "quoting the arrival must not acquire it");
    let outcome = run_connected_session(&obs, &mut UtilityPolicy::new(), &mut strategy);
    assert!(outcome.allocation_ok && outcome.accepted_connected);
    assert!(strategy.owned_units().any(|id| id == arrived));
    assert!(outcome.planner_claims.contains(&arrived));
}

#[test]
fn active_connected_revision_and_saved_foundry_commit_together() {
    let mut observation = connected_observation(1_200, 10_000);
    let builder = UnitId(200);
    observation.my_units.push(owned_unit(
        builder.0,
        UnitKind::Harvester,
        TilePos::new(12, 15),
    ));
    observation.my_units.sort_unstable_by_key(|unit| unit.id);
    observation.my_buildings.push(observed_building(
        14,
        0,
        BuildingKind::Extractor,
        TilePos::new(20, 14),
    ));
    observation.my_queues.push(Vec::new());
    observation.my_queue_progress.push(0);
    let proposal = current_connected_proposal(&observation);
    let fixed_deadline = proposal.deadline();
    let mut planner = StrategicPlanner::new();
    planner
        .prepare_connected_commit(proposal)
        .unwrap()
        .apply(&mut planner);
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let foundry_anchor = TilePos::new(15, 14);
    let mut policy = UtilityPolicy::new();
    policy
        .prepare_adjudicated_foundry(
            FreshFoundryProposal::fixture(
                foundry_anchor,
                builder,
                foundry_cost,
                0,
                0,
                fixed_deadline,
                foundry_case(),
            ),
            observation.tick,
        )
        .expect("the fixture installs one exact saved Foundry")
        .apply(&mut policy, &mut Vec::new());

    observation.tick = observation.tick.saturating_add(12);
    let mut strategy = planner;
    let outcome = run_connected_session(&observation, &mut policy, &mut strategy);

    assert!(outcome.allocation_ok);
    assert!(
        outcome.accepted_connected,
        "the revisable operation must re-enter the typed allocation"
    );
    assert!(outcome.fresh_foundry_intents.contains(&Intent::BuildWith {
        builder,
        kind: BuildingKind::Foundry,
        anchor: foundry_anchor,
    }));
    assert!(
        !outcome.allocated_producer_intents.is_empty(),
        "ample residual capital should still admit fresh standing-force production"
    );
    let retained = connected_obligation(&mut strategy, &observation);
    assert_eq!(retained.deadline(), fixed_deadline);
    assert!(
        retained
            .provider_jobs()
            .iter()
            .all(|job| job.ready_before() == fixed_deadline)
    );
}

#[test]
fn newer_conflict_does_not_discard_an_older_connected_obligation() {
    let observation = connected_observation(120, 10_000);
    let (mut planner, requests) = current_connected_planner(&observation);
    let active = connected_obligation(&mut planner, &observation);
    let first = &requests[0];
    let resources = ResourceSnapshot::from_observation(&observation);
    let newer_accepted_at = active.accepted_at() + 1;
    let newer_key = ObligationKey::AirPurchases;
    let producer = first.eligible_producers()[0];
    let ready = observation.tick + Tick::from(first.kind().stats().train_ticks) - 1;
    let job = ProducerJobClaim::fixed(
        producer,
        first.kind(),
        observation.tick,
        observation.tick,
        ready,
        active.deadline(),
    );
    let newer = imported_obligation(
        ObligationClass::PersistentPlan,
        newer_accepted_at,
        newer_key,
        ClaimBundle::new(0, vec![], vec![], vec![], vec![], vec![job.clone(), job]).unwrap(),
    );
    let active_import = active_connected_obligation(&active)
        .expect("the older connected obligation adapts exactly");
    let active_owner = active_import.owner();
    let mut proof = CrossDomainAllocation::new(&resources, active.deadline(), 12)
        .expect("the fixture horizon is valid");
    proof.import(active_import.clone());
    proof.import(newer.clone());
    assert!(matches!(
        proof.resolve(AllocationPersonality::default(), None),
        Err(AllocationError::ObligationConflict {
            obligation: ClaimOwner::Obligation {
                class: ObligationClass::PersistentPlan,
                accepted_at,
                key,
            },
            conflict: AllocationConflict::ProducerSchedule { .. },
        }) if accepted_at == newer_accepted_at && key == newer_key
    ));

    let mut obligations = ObligationPreparation {
        resources,
        obligations: vec![active_import, newer],
        coordinator_failure: None,
        active_connected: Some(active.clone()),
        active_lift: None,
        invalid_active_connected: false,
        invalid_active_lift: false,
        retained_air_claims: None,
        island_preparation: None,
    };
    let setup = SessionProfile::new(prime_profile());
    let briefing = connected_briefing(&observation);
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observation);
    let mut policy = UtilityPolicy::new();
    let mut strategy = planner;
    let mut lifts = LiftPlanner::new();
    let mut team = TeamReliefPlanner::new();
    let mut raids = RaidPlanner::new();

    let mut session = AllocationSession::new(
        setup.context(&observation, TilePos::new(3, 10), &briefing, &intelligence),
        AllocationParticipants {
            policy: &mut policy,
            strategy: &mut strategy,
            lifts: &mut lifts,
            team: &mut team,
            raids: &mut raids,
        },
        advanced(),
        None,
    );
    session
        .retained_work()
        .downgrade_unfundable_active_connected(
            &mut SavedFoundryPreparation {
                obligation: None,
                saving: 0,
                blocked: false,
                preparation_need: None,
            },
            &AirLiftPreparation {
                lift_membership: None,
                lift_purchases: crate::production::ProductionPlan::default(),
                opening_bootstrap: 0,

                active_lift_precedes_foundry: false,
                active_lift_spendable: 0,
                saved_plan_reserve_already_imported: 0,

                lift_deadline: observation.tick.saturating_add(12),
                fresh_lift_producer_jobs: 0,
                voluntary_scrap_guard: 0,
            },
            &mut obligations,
        );

    assert_eq!(obligations.active_connected, Some(active));
    assert!(
        obligations
            .obligations
            .iter()
            .any(|obligation| obligation.owner() == active_owner),
        "a later planner's conflict cannot demote the older connected commitment"
    );
}

#[test]
fn fresh_standing_force_cannot_defer_a_payable_saved_foundry() {
    payable_saved_foundry_with_planning_allowance(128_000);
}

#[test]
fn exhausted_optional_planning_does_not_delay_a_payable_saved_foundry() {
    payable_saved_foundry_with_planning_allowance(0);
}

fn payable_saved_foundry_with_planning_allowance(allowance: usize) {
    let mut observation = connected_observation(1_200, 0);
    let builder = UnitId(200);
    let foundry_anchor = TilePos::new(15, 14);
    observation.my_units.push(owned_unit(
        builder.0,
        UnitKind::Harvester,
        TilePos::new(12, 15),
    ));
    observation.my_units.sort_unstable_by_key(|unit| unit.id);
    for (id, kind, anchor) in [
        (14, BuildingKind::Extractor, TilePos::new(20, 14)),
        (15, BuildingKind::Reclaimer, TilePos::new(12, 17)),
    ] {
        observation
            .my_buildings
            .push(observed_building(id, 0, kind, anchor));
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress.push(0);
    }
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let forecast_deadline = observation
        .tick
        .saturating_add(Tick::from(foundry_cost).saturating_mul(oxide_sim::stats::RECLAIMER_PERIOD))
        .saturating_add(oxide_sim::stats::RECLAIMER_PERIOD);
    let mut policy = UtilityPolicy::new();
    policy.planning = crate::planning::PlanningWork::with_allowance(allowance);
    let mut initial_intents = Vec::new();
    policy
        .prepare_adjudicated_foundry(
            FreshFoundryProposal::fixture(
                foundry_anchor,
                builder,
                0,
                foundry_cost,
                0,
                forecast_deadline,
                foundry_case(),
            ),
            observation.tick,
        )
        .expect("the forecast-backed fixture installs one exact saved Foundry")
        .apply(&mut policy, &mut initial_intents);
    assert!(
        initial_intents.is_empty(),
        "forecast capital cannot dispatch the Foundry at admission"
    );

    observation.tick = observation.tick.saturating_add(12);
    observation.scrap = foundry_cost;
    let resources = ResourceSnapshot::from_observation(&observation);
    let saved = policy
        .validated_foundry_obligation(&observation, &resources, true, observation.scrap)
        .expect("the accepted Foundry remains valid after its bank accrues");
    assert!(
        saved.ready_to_build(),
        "the full current construction cost makes the retained plan payable"
    );

    let standing_ready_before = observation
        .tick
        .saturating_add(Tick::from(UnitKind::Lancer.stats().train_ticks))
        .saturating_add(1);
    let standing = StandingForceProposal::fixture(StandingForceFixture {
        observed_at: observation.tick,
        ready_before: standing_ready_before,
        kind: UnitKind::Lancer,
        reason: StandingForceReason::ForceProjection,
        specialty: Specialty::Siege,
        personality_emphasis: 100,
        case: ProposalCase {
            urgency: Urgency::Pressing,
            confidence: Confidence::Current,
            value: StrategicValue::Decisive,
            time_to_impact: TimeToImpact::Immediate,
            safety: ExecutionSafety::Secure,
        },
        eligible_producers: vec![BuildingId(11)],
    });
    let setup = SessionProfile::new(prime_profile());
    let briefing = connected_briefing(&observation);
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observation);
    let mut strategy = StrategicPlanner::new();
    let mut lifts = LiftPlanner::new();
    let mut team = TeamReliefPlanner::new();
    let mut raids = RaidPlanner::new();

    let mut prepared = prepared(&observation, None);
    prepared.resources = resources;
    prepared.obligations = vec![
        saved_foundry_obligation(saved)
            .expect("the ready saved Foundry has exact mandatory claims"),
    ];
    prepared.saved_foundry = Some(saved);
    prepared.standing_force = StandingForcePreparation::Unconditional(vec![standing]);
    prepared.allocation_horizon = forecast_deadline.max(standing_ready_before);
    prepared.foundry_saving = foundry_cost;
    let mut session = AllocationSession::new(
        setup.context(&observation, TilePos::new(3, 10), &briefing, &intelligence),
        AllocationParticipants {
            policy: &mut policy,
            strategy: &mut strategy,
            lifts: &mut lifts,
            team: &mut team,
            raids: &mut raids,
        },
        advanced(),
        None,
    );
    let resolved = session.resolve(prepared);
    let outcome = session.finish_allocation(resolved);

    assert!(outcome.allocation_ok);
    assert_eq!(
        outcome.fresh_foundry_intents,
        vec![Intent::BuildWith {
            builder,
            kind: BuildingKind::Foundry,
            anchor: foundry_anchor,
        }],
        "fresh optional production cannot move a payable persistent plan back onto forecast"
    );
    assert!(
        outcome.allocated_producer_intents.is_empty(),
        "the Standing Lancer must wait when the older Foundry consumes the current bank"
    );
}

#[test]
fn rolled_back_connected_procurement_retries_without_extending_deadline() {
    let mut observation = connected_observation(120, 10_000);
    let (planner, _) = current_connected_planner(&observation);
    let first_enqueue = observation.tick;
    let invalid_production = StrategicDecision {
        intents: vec![Intent::TrainAt {
            building: BuildingId(999),
            kind: UnitKind::Sentinel,
        }],
        reservations: Vec::new(),
        reserved_scrap: 0,
    };
    let mut policy = UtilityPolicy::new();
    let mut strategy = planner;
    let failed = run_connected_session_with_producer_work(
        &observation,
        &mut policy,
        &mut strategy,
        invalid_production,
    );
    assert!(
        !failed.allocation_ok,
        "the unrelated invalid producer must roll the whole allocation pass back"
    );
    let retained = connected_obligation(&mut strategy, &observation);
    assert!(!retained.provider_jobs().is_empty());
    observation.tick = first_enqueue + 12;
    let retry = run_connected_session(&observation, &mut policy, &mut strategy);
    assert!(retry.allocation_ok);
    assert!(!retry.allocated_producer_intents.is_empty());
    let after = connected_obligation(&mut strategy, &observation);
    assert_eq!(after.deadline(), retained.deadline());
    assert!(
        strategy
            .air_operation()
            .unwrap()
            .recovery_reason()
            .is_none()
    );
}
