use super::*;

#[test]
fn retained_lift_recovery_respects_conflict_owner_and_foundry_admission() {
    for foundry_accepted_at in [0, 24, 48] {
        let (mut observation, _, _) = active_lift_fixture();
        observation.tick = 24;
        let mut lift = LiftPlanner::new();
        lift.think_with_admission(
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
        );
        let operation = lift.operation().unwrap().clone();
        assert_eq!(operation.started_at, 24);
        let enqueued_at = 72;
        lift.bind_producer_assignments(
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
        .unwrap();
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
            .commit_adjudicated_foundry(
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
                &mut Vec::new(),
            )
            .unwrap();
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
            legacy_air_claims: None,
            staged_strategy: None,
        };
        let mut saved = SavedFoundryPreparation {
            obligation: Some(foundry),
            saving: cost,
            blocked: false,
            preparation_need: None,
        };
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let briefing = connected_briefing(&observation);
        let intelligence = StrategicIntelligence::new();
        let mut strategy = None;
        let mut lifts = Some(lift);
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut session = AllocationSession::new(
            AllocationSessionContext {
                evidence: Default::default(),
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(5, 15),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(5, 15)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            None,
        );
        session
            .retained_work()
            .reconcile_lift_funding_after_saved_capital(&mut saved, &mut obligations);
        let lift = session.participants.lifts.as_ref().unwrap();
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
fn unfundable_retained_lift_recovers_without_releasing_members() {
    let (mut observation, mut lift, _) = active_lift_fixture();
    observation.tick = 12;
    observation.scrap = 300;
    let operation = lift.operation().unwrap().clone();
    let enqueued_at = 24;
    lift.bind_producer_assignments(
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
    .unwrap();
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
        legacy_air_claims: None,
        staged_strategy: None,
    };
    let profile = prime_profile();
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let dials = Dials::scripted(&profile, tuning);
    let briefing = connected_briefing(&observation);
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observation);
    let mut policy = UtilityPolicy::new();
    let mut strategy = None;
    let mut lifts = Some(lift);
    let mut team = None;
    let mut raids = None;
    let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
    let mut session = AllocationSession::new(
        AllocationSessionContext {
            evidence: Default::default(),
            dials: &dials,
            profile: &profile,
            tuning,
            observation: &observation,
            home: TilePos::new(5, 15),
            public_map: &briefing,
            orientation: Orientation::for_home(&observation, TilePos::new(5, 15)),
            intelligence: &intelligence,
            enlisted: &[],
            lift_support: None,
        },
        AllocationParticipants {
            policy: &mut policy,
            strategy: &mut strategy,
            lifts: &mut lifts,
            team: &mut team,
            raids: &mut raids,
        },
        advanced(snapshots),
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
    let lift = session.participants.lifts.as_ref().unwrap();
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
        ObligationKey::Legacy {
            channel: LegacyChannel::StrategicAir,
            sequence: 1,
        },
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
        &crate::bot::planning::PlanningWork::default(),
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
    let mut proposal = current_connected_proposal(&observation);
    let identity = proposal.identity();
    let assignments = connected_assignments(&proposal, false)
        .into_iter()
        .map(|assignment| {
            let timing = assignment.timing();
            let shift = 24;
            ConnectedProducerAssignment::new(
                identity,
                assignment.request_ordinal(),
                assignment.producer(),
                assignment.kind(),
                ConnectedProducerTiming::new(
                    timing.enqueued_at().saturating_add(shift),
                    timing.starts_at().saturating_add(shift),
                    timing.ready_at().saturating_add(shift),
                    timing.ready_before(),
                ),
                ConnectedProducerFunding::new(assignment.kind().stats().cost, 0),
            )
        })
        .collect::<Vec<_>>();
    proposal
        .bind_producer_assignments(assignments.clone())
        .expect("the future exact minimum schedule binds");
    let mut planner = StrategicPlanner::new();
    planner
        .commit_connected_proposal(proposal)
        .expect("the bound connected package commits");
    let fixed_deadline = assignments[0].timing().ready_before();
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let foundry_anchor = TilePos::new(15, 14);
    let mut policy = UtilityPolicy::new();
    policy
        .commit_adjudicated_foundry(
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
            &mut Vec::new(),
        )
        .expect("the fixture installs one exact saved Foundry");

    observation.tick = observation.tick.saturating_add(12);
    let mut strategy = Some(planner);
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
    let retained = strategy
        .as_ref()
        .and_then(|planner| planner.active_connected_obligation(&observation))
        .expect("the revised connected operation remains active");
    assert_eq!(retained.deadline(), fixed_deadline);
    assert_eq!(
        retained
            .provider_jobs()
            .iter()
            .take(assignments.len())
            .map(|assignment| {
                (
                    assignment.request_ordinal(),
                    assignment.producer(),
                    assignment.kind(),
                    assignment.timing(),
                )
            })
            .collect::<Vec<_>>(),
        assignments
            .iter()
            .map(|assignment| {
                (
                    assignment.request_ordinal(),
                    assignment.producer(),
                    assignment.kind(),
                    assignment.timing(),
                )
            })
            .collect::<Vec<_>>(),
        "a compatible Foundry cannot shift accepted connected jobs while the revision adds new marginal work"
    );
}

#[test]
fn newer_conflict_does_not_discard_an_older_connected_obligation() {
    let observation = connected_observation(120, 10_000);
    let (planner, assignments) = current_connected_planner(&observation, false);
    let active = planner
        .active_connected_obligation(&observation)
        .expect("the committed operation exposes its exact obligation");
    let first = assignments[0];
    let resources = ResourceSnapshot::from_observation(&observation);
    let newer_accepted_at = active.accepted_at().saturating_add(1);
    let newer_key = ObligationKey::Legacy {
        channel: LegacyChannel::TeamRelief,
        sequence: 99,
    };
    let decision = StrategicDecision {
        intents: vec![Intent::TrainAt {
            building: first.producer(),
            kind: first.kind(),
        }],
        reservations: Vec::new(),
        committed_scrap: first.kind().stats().cost,
    };
    let newer = legacy_decision_obligation(
        &resources,
        LegacyDecisionRequest {
            cadence: 12,
            accepted_at: newer_accepted_at,
            decision_tick: first.timing().enqueued_at(),
            channel: LegacyChannel::TeamRelief,
            sequence: 99,
            decision: &decision,
            prior_producer_intents: &[],
            production_deadline: active.deadline(),
        },
    )
    .expect("the newer producer claim is independently legal");
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
                class: ObligationClass::Legacy,
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
        legacy_air_claims: None,
        staged_strategy: None,
    };
    let profile = prime_profile();
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let dials = Dials::scripted(&profile, tuning);
    let briefing = connected_briefing(&observation);
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observation);
    let mut policy = UtilityPolicy::new();
    let mut strategy = Some(planner);
    let mut lifts = None;
    let mut team = None;
    let mut raids = None;
    let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
    let mut session = AllocationSession::new(
        AllocationSessionContext {
            evidence: Default::default(),
            dials: &dials,
            profile: &profile,
            tuning,
            observation: &observation,
            home: TilePos::new(3, 10),
            public_map: &briefing,
            orientation: Orientation::for_home(&observation, TilePos::new(3, 10)),
            intelligence: &intelligence,
            enlisted: &[],
            lift_support: None,
        },
        AllocationParticipants {
            policy: &mut policy,
            strategy: &mut strategy,
            lifts: &mut lifts,
            team: &mut team,
            raids: &mut raids,
        },
        advanced(snapshots),
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
                lift_decision: StrategicDecision::default(),
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
        .saturating_add(Tick::from(foundry_cost).saturating_mul(crate::stats::RECLAIMER_PERIOD))
        .saturating_add(crate::stats::RECLAIMER_PERIOD);
    let mut policy = UtilityPolicy::new();
    policy.planning = crate::bot::planning::PlanningWork::with_allowance(allowance);
    let mut initial_intents = Vec::new();
    policy
        .commit_adjudicated_foundry(
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
            &mut initial_intents,
        )
        .expect("the forecast-backed fixture installs one exact saved Foundry");
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
    let profile = prime_profile();
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let dials = Dials::scripted(&profile, tuning);
    let briefing = connected_briefing(&observation);
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observation);
    let original_policy = policy.clone();
    let mut strategy = None;
    let mut lifts = None;
    let mut team = None;
    let mut raids = None;
    let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
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
        AllocationSessionContext {
            evidence: Default::default(),
            dials: &dials,
            profile: &profile,
            tuning,
            observation: &observation,
            home: TilePos::new(3, 10),
            public_map: &briefing,
            orientation: Orientation::for_home(&observation, TilePos::new(3, 10)),
            intelligence: &intelligence,
            enlisted: &[],
            lift_support: None,
        },
        AllocationParticipants {
            policy: &mut policy,
            strategy: &mut strategy,
            lifts: &mut lifts,
            team: &mut team,
            raids: &mut raids,
        },
        advanced(snapshots),
        None,
    );
    let resolved = session.resolve(
        prepared,
        CommitSnapshots {
            policy: original_policy.speculative_checkpoint(),
        },
    );
    let outcome = session.commit_or_restore(resolved);

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
fn connected_enqueue_missed_after_rollback_enters_bounded_recovery() {
    let mut observation = connected_observation(120, 10_000);
    let mut proposal = current_connected_proposal(&observation);
    let identity = proposal.identity();
    let assignments = connected_assignments(&proposal, false)
        .into_iter()
        .map(|assignment| {
            let timing = assignment.timing();
            let shift = 24;
            ConnectedProducerAssignment::new(
                identity,
                assignment.request_ordinal(),
                assignment.producer(),
                assignment.kind(),
                ConnectedProducerTiming::new(
                    timing.enqueued_at().saturating_add(shift),
                    timing.starts_at().saturating_add(shift),
                    timing.ready_at().saturating_add(shift),
                    timing.ready_before(),
                ),
                ConnectedProducerFunding::new(assignment.kind().stats().cost, 0),
            )
        })
        .collect::<Vec<_>>();
    proposal
        .bind_producer_assignments(assignments.clone())
        .expect("the future exact minimum schedule binds");
    let mut planner = StrategicPlanner::new();
    planner
        .commit_connected_proposal(proposal)
        .expect("the bound connected package commits");
    let first_enqueue = assignments
        .iter()
        .map(|assignment| assignment.timing().enqueued_at())
        .min()
        .expect("the connected minimum retains provider work");
    observation.tick = first_enqueue;
    let invalid_team_decision = StrategicDecision {
        intents: vec![Intent::TrainAt {
            building: BuildingId(999),
            kind: UnitKind::Sentinel,
        }],
        reservations: Vec::new(),
        committed_scrap: UnitKind::Sentinel.stats().cost,
    };
    let mut policy = UtilityPolicy::new();
    let mut strategy = Some(planner);
    let failed = run_connected_session_with_team_decision(
        &observation,
        &mut policy,
        &mut strategy,
        invalid_team_decision,
    );
    assert!(
        !failed.allocation_ok,
        "the unrelated invalid producer must roll the whole allocation pass back"
    );
    let retained = strategy
        .as_ref()
        .and_then(|planner| planner.active_connected_obligation(&observation))
        .expect("rollback must preserve the previously admitted operation");
    assert!(
        retained
            .provider_jobs()
            .iter()
            .any(|assignment| assignment.timing().enqueued_at() == first_enqueue),
        "rollback must leave the due append unpaid until a later pass diagnoses it"
    );

    observation.tick = first_enqueue.saturating_add(12);
    assert_connected_enters_bounded_recovery(
        &observation,
        &mut policy,
        &mut strategy,
        "missed accepted enqueue after rollback",
    );
}
