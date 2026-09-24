use super::*;

#[test]
fn accepted_foundry_saving_owns_the_bank_before_later_connected_air_production() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
    let state = scenario
        .build()
        .expect("the Foundry-saving air competition scenario builds");
    let mut brain = foundry_competition_brain(&scenario);

    let first_commands = brain.act(&state);
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let orientation = brain
        .orientation
        .expect("the first think latches the policy frame");
    let oriented = orientation.observe(&raw);
    let saved = brain.policy.validated_foundry_saving(&oriented, true);
    assert!(
        saved > foundry_cost - 1,
        "the first think must accept the underfunded expansion before strategic competition: saved={saved}, commands={first_commands:?}, buildings={:?}, units={}, frames={:?}",
        oriented
            .my_buildings
            .iter()
            .map(|building| (building.kind, building.anchor))
            .collect::<Vec<_>>(),
        oriented.my_units.len(),
        oriented.known_frames,
    );

    let mut continuation = scenario.clone();
    let scout = continuation
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the continuation has one connected-air scout");
    (scout.x, scout.y) = (42, 19);
    let mut state = continuation
        .build()
        .expect("the connected-air continuation builds");
    state.tick(&[]);
    while !state.current_tick().is_multiple_of(brain.dials.cadence)
        || !crate::difficulty::strategic_admission_tick(state.current_tick())
    {
        state.tick(&[]);
    }
    brain.mind_mut().strategy = StrategicPlanner::new();
    let mut wealthy_brain = brain.clone();

    let mut document = serde_json::to_value(&state).expect("the state serializes");
    document["players"][0]["scrap"] = serde_json::json!(saved - 1);
    let starved_state: State =
        serde_json::from_value(document.clone()).expect("the underfunded state remains valid");
    let starved_observation = brain
        .orientation
        .expect("the policy frame remains latched")
        .observe(&Observation::fog_honest(&starved_state, PlayerId(0)));
    assert!(
        starved_observation
            .enemy_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::Foundry),
        "the connected opportunity must be in current sight"
    );
    let mut direct_starved_brain = brain.clone();
    let direct_starved_commands = direct_starved_brain.act(&starved_state);
    let starved = brain.act_traced(&starved_state);
    assert_eq!(starved.commands, direct_starved_commands);
    let mut direct_after = starved_state.clone();
    let mut traced_after = starved_state.clone();
    direct_after.tick(&direct_starved_commands);
    traced_after.tick(&starved.commands);
    assert_eq!(traced_after.hash(), direct_after.hash());
    assert_brain_unchanged(&direct_starved_brain, &brain);
    let starved_trace = starved.trace.expect("the admission think is traced");
    let starved_budget = starved_trace
        .budget
        .expect("the admission think records its scrap ledger");
    assert_eq!(starved_budget.foundry_saving, saved);
    assert_eq!(starved_budget.strategic_spendable, 0);
    assert_eq!(
        starved_trace.channels.connected_air.after,
        ChannelState::Idle,
        "an unfunded coherent package must not become an active operation"
    );
    assert_eq!(
        starved_trace.connected_force.status,
        crate::trace::ConnectedForceStatus::Idle
    );
    assert!(starved_trace.connected_force.package.is_none());
    assert!(
        starved_trace.connected_force.rejected_candidate.is_none(),
        "the connected domain produced a legal proposal for shared adjudication"
    );
    let connected_context = starved_trace
        .allocation
        .connected_context
        .expect("the allocator compared exact connected-presence contexts");
    assert!(
        connected_context.considered >= 2,
        "the absent and minimum connected contexts must both reach shared adjudication"
    );
    assert_eq!(
        connected_context.selected,
        crate::trace::ConnectedPortfolioSelectionTrace::Absent,
        "the older Foundry obligation must make the later connected context lose"
    );
    assert_eq!(
        starved_trace.channels.connected_air.effects.committed_scrap, 0,
        "connected air may see only bank beyond the frozen Foundry total"
    );
    let mut continued_state = starved_state.clone();
    for _ in 0..brain.dials.cadence {
        continued_state.tick(&[]);
    }
    let mut continued_document =
        serde_json::to_value(&continued_state).expect("the continued state serializes");
    continued_document["players"][0]["scrap"] = serde_json::json!(saved - 1);
    let continued_state = serde_json::from_value(continued_document)
        .expect("the normalized continued state remains valid");
    let continued = brain.act_traced(&continued_state);
    let continued_trace = continued.trace.expect("the next strategic think is traced");
    let continued_budget = continued_trace
        .budget
        .expect("the active operation's next think records its scrap ledger");
    assert_eq!(
        continued_budget.prior_operation_spendable, 0,
        "no operation predates the accepted Foundry saving"
    );
    assert_eq!(continued_budget.strategic_spendable, 0);
    assert_eq!(
        continued_trace.channels.connected_air.after,
        ChannelState::Idle
    );
    assert_eq!(
        continued_trace
            .channels
            .connected_air
            .effects
            .committed_scrap,
        0,
        "a post-saving opportunity cannot move ahead of the Foundry hold"
    );
    let starved_raw = Observation::fog_honest(&starved_state, PlayerId(0));
    assert_eq!(
        brain
            .policy
            .validated_foundry_saving(&orientation.observe(&starved_raw), true),
        saved,
        "strategic competition cannot shrink the accepted saving"
    );

    let operation_fund = UnitKind::Bombard
        .stats()
        .cost
        .max(UnitKind::Avalanche.stats().cost)
        .saturating_add(UnitKind::Condor.stats().cost);
    document["players"][0]["scrap"] = serde_json::json!(saved.saturating_add(operation_fund));
    let funded_state: State =
        serde_json::from_value(document).expect("the fully funded state remains valid");
    let funded = wealthy_brain.act_traced(&funded_state);
    let funded_trace = funded.trace.expect("the funded admission think is traced");
    let funded_budget = funded_trace
        .budget
        .expect("the funded think records its scrap ledger");
    let standing_current_scrap = funded_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                crate::trace::ClaimOwnerTrace::Proposal {
                    key: crate::trace::ProposalKeyTrace::StandingForce { .. },
                }
            )
        })
        .map(|job| job.current_scrap)
        .sum::<u32>();
    let capital_current_scrap = funded_trace
        .allocation
        .proposals
        .entries
        .iter()
        .filter(|proposal| {
            proposal.disposition == crate::trace::ProposalDispositionTrace::Accepted
                && matches!(
                    proposal.key,
                    crate::trace::ProposalKeyTrace::Defense { .. }
                        | crate::trace::ProposalKeyTrace::Economy { .. }
                )
        })
        .map(|proposal| proposal.claims.current_scrap)
        .sum::<u32>();
    assert_eq!(funded_budget.foundry_saving, saved);
    assert_eq!(
        funded_budget.voluntary_scrap_guard,
        UnitKind::Sentinel.stats().cost,
        "a selected non-Sentinel standing-force alternative must leave the shallow screen available"
    );
    assert_eq!(
        funded_budget
            .strategic_spendable
            .saturating_add(standing_current_scrap)
            .saturating_add(capital_current_scrap),
        operation_fund.saturating_sub(funded_budget.voluntary_scrap_guard),
        "the accepted Foundry owns only its construction capital; shared allocation may spend the independent excess on connected, standing-force, and defense investment after retaining the shallow screen exactly once: budget={funded_budget:?}, schedule={:?}",
        funded_trace.allocation.producer_schedule,
    );
    assert!(
        funded_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .any(|job| matches!(
                job.owner,
                crate::trace::ClaimOwnerTrace::Obligation {
                    key: crate::trace::ObligationKeyTrace::ConnectedOffense { .. },
                    ..
                } | crate::trace::ClaimOwnerTrace::Proposal {
                    key: crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                }
            )),
        "connected air may claim the independent excess bank"
    );
    assert!(
        funded
            .commands
            .iter()
            .any(|command| matches!(command.command, Command::Train { .. })),
        "bank covering both obligations must admit ordinary paid air-operation work: {:?}",
        funded.commands
    );
}

#[test]
fn older_construction_promise_owns_forecast_until_current_bank_covers_it() {
    let promised_kind = BuildingKind::Foundry;
    let promised_scrap = promised_kind
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let mut scenario = foundry_saving_air_competition_scenario(0);
    let scout = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the scenario has one connected-operation scout");
    (scout.x, scout.y) = (42, 19);
    scenario.buildings.extend([
        building_spec(0, BuildingKind::Reclaimer, 18, 4),
        building_spec(0, BuildingKind::Reclaimer, 21, 4),
    ]);
    let mut state = scenario
        .build()
        .expect("the forecast-ownership scenario builds");
    let founder = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .expect("the scenario has a builder")
        .id;
    let promised_anchor = TilePos::new(20, 9);
    crate::test_support::edit_units(&mut state, |units| {
        let founder = units.iter_mut().find(|unit| unit.id == founder).unwrap();
        founder.order = oxide_sim::state::Order::Found {
            kind: promised_kind,
            anchor: promised_anchor,
        };
        founder.path = None;
    });

    let mut brain = foundry_competition_brain(&scenario);
    let mut funded_brain = brain.clone();
    let mut funded_state = state.clone();

    let starved = brain.act_traced(&state);
    let starved_trace = starved.trace.expect("the admission think is traced");
    let starved_budget = starved_trace
        .budget
        .expect("the admission think records its scrap ledger");
    assert_eq!(starved_budget.bank, 0);
    assert_eq!(starved_budget.deferred_construction, promised_scrap);
    assert_eq!(starved_budget.strategic_spendable, 0);
    assert_eq!(
        starved_trace.channels.connected_air.after,
        ChannelState::Idle,
        "forecast promised to older construction cannot admit a new operation"
    );
    let rejection = starved_trace
        .connected_force
        .rejected_candidate
        .expect("the current opportunity records the protected forecast");
    assert!(
        matches!(
            rejection.reason,
            crate::trace::ConnectedRejectionReasonTrace::ProtectedFunds {
                protected_current_scrap: 0,
                protected_forecast_scrap,
                ..
            } if protected_forecast_scrap == promised_scrap
        ),
        "unexpected rejection: {:?}",
        rejection.reason
    );

    crate::test_support::edit_player(&mut funded_state, PlayerId(0), |item| {
        item.scrap = promised_scrap + UnitKind::Sentinel.stats().cost
    });
    let funded = funded_brain.act_traced(&funded_state);
    let funded_trace = funded.trace.expect("the funded think is traced");
    let funded_budget = funded_trace
        .budget
        .expect("the funded think records its scrap ledger");
    assert_eq!(funded_budget.deferred_construction, promised_scrap);
    assert_eq!(
        funded_budget.strategic_spendable, 0,
        "the current bank remains owned by the older construction promise"
    );
    assert!(
        matches!(
            funded_trace.channels.connected_air.after,
            ChannelState::Active(_)
        ),
        "covering the old promise with current capital frees recurring income for admission"
    );
    let package = funded_trace
        .connected_force
        .package
        .expect("the recurring-income surplus admits a concrete package");
    assert_eq!(
        package.current_scrap,
        UnitKind::Sentinel.stats().cost,
        "only the bank surplus beyond the promise is current capital"
    );
    assert!(package.forecast_scrap > promised_scrap);
}

#[test]
fn compatible_connected_minimum_and_foundry_defer_only_residual_scale() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
    let scout = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the competition scenario has one connected-air scout");
    (scout.x, scout.y) = (42, 19);
    scenario.units.extend([
        unit_spec(0, UnitKind::Bombard, 9, 17),
        unit_spec(0, UnitKind::Condor, 10, 18),
        unit_spec(0, UnitKind::Condor, 11, 18),
    ]);
    let mut state = scenario
        .build()
        .expect("the fully staffed connected-air scenario builds");
    let mut brain = foundry_competition_brain(&scenario);

    let admission_tick = state.current_tick();
    let first = brain.act_traced(&state);
    let first_trace = first.trace.as_ref().expect("the admission think is traced");
    assert_eq!(
        first_trace.channels.connected_air.before,
        ChannelState::Idle
    );
    assert_eq!(
        first_trace.channels.connected_air.after,
        ChannelState::Active(ChannelPhase::AirRecon)
    );
    let accepted_keys = first_trace
        .allocation
        .proposals
        .entries
        .iter()
        .filter(|proposal| proposal.disposition == crate::trace::ProposalDispositionTrace::Accepted)
        .map(|proposal| proposal.key)
        .collect::<Vec<_>>();
    assert_eq!(
        accepted_keys.len(),
        4,
        "the staffed connected minimum, expansion, defense, and standing force are compatible: {accepted_keys:?}"
    );
    assert!(
        accepted_keys
            .iter()
            .any(|key| matches!(key, crate::trace::ProposalKeyTrace::FoundryExpansion { .. }))
    );
    assert!(accepted_keys.iter().any(|key| matches!(
        key,
        crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
    )));
    assert!(
        accepted_keys
            .iter()
            .any(|key| matches!(key, crate::trace::ProposalKeyTrace::Defense { .. }))
    );
    assert!(
        accepted_keys
            .iter()
            .any(|key| matches!(key, crate::trace::ProposalKeyTrace::StandingForce { .. }))
    );
    let package = first_trace
        .connected_force
        .package
        .as_ref()
        .expect("the admitted operation exposes its scaled package");
    assert!(
        package
            .demands
            .recon
            .iter()
            .any(|demand| { demand.kind == UnitKind::Kestrel && demand.count > 0 })
    );
    assert!(
        package
            .demands
            .suppression
            .iter()
            .any(|demand| { demand.kind == UnitKind::Bombard && demand.count > 0 })
    );
    assert!(package.demands.strike.iter().any(|demand| {
        matches!(demand.kind, UnitKind::Buzzard | UnitKind::Condor) && demand.count > 0
    }));
    assert!(first.commands.iter().all(|command| !matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Buzzard,
            ..
        }
    )));
    let residual_buzzard = first_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .find(|job| job.kind == UnitKind::Buzzard)
        .expect("the residual connected scale retains its optional Buzzard")
        .clone();
    assert!(residual_buzzard.enqueued_at > admission_tick);
    assert_eq!(residual_buzzard.current_scrap, 0);
    assert_eq!(
        residual_buzzard.forecast_scrap,
        UnitKind::Buzzard.stats().cost
    );
    let admitted_at = {
        let strategy = &brain.mind().strategy;
        let operation = strategy
            .air_operation()
            .expect("the normal strategic pass admits connected air");
        assert!(operation.scout.is_some());
        assert!(!operation.artillery.is_empty());
        assert!(!operation.strike_aircraft.is_empty());
        strategy
            .air_admitted_at()
            .expect("the admitted operation records its priority tick")
    };
    assert_eq!(admitted_at, admission_tick);
    let orientation = brain
        .orientation
        .expect("the first think latches the policy frame");
    let first_raw = Observation::fog_honest(&state, PlayerId(0));
    let first_oriented = orientation.observe(&first_raw);
    let saved = brain.policy.validated_foundry_saving(&first_oriented, true);
    assert!(
        saved > state.player(PlayerId(0)).scrap,
        "utility accepts the underfunded Foundry after the fully staffed operation"
    );
    assert!(
        brain.policy.operation_precedes_foundry_saving(admitted_at),
        "same-pass strategic admission precedes utility expansion"
    );

    let report = state.tick(&first.commands);
    assert_commands_accepted(&report, PlayerId(0));
    while state.current_tick() < admission_tick.saturating_add(brain.dials.cadence) {
        state.tick(&[]);
    }
    assert_eq!(
        state.current_tick(),
        admission_tick.saturating_add(brain.dials.cadence)
    );

    let mut document = serde_json::to_value(&state).expect("the continuation serializes");
    document["players"][0]["scrap"] = serde_json::json!(saved - 1);
    let later_state: State =
        serde_json::from_value(document).expect("the underfunded continuation remains valid");

    let later = brain.act_traced(&later_state);
    let later_trace = later.trace.expect("the later cadence think is traced");
    let later_budget = later_trace
        .budget
        .expect("the later think records its scrap ledger");
    assert!(later_budget.bank < saved);
    assert_eq!(later_budget.foundry_saving, saved);
    assert_eq!(later_budget.strategic_spendable, 0);
    assert!(later_trace.allocation.coordinator_failure.is_none());
    let retained_buzzard = later_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .find(|job| job.producer == residual_buzzard.producer && job.kind == residual_buzzard.kind)
        .expect("the earlier operation retains its optional Buzzard demand");
    assert_eq!(retained_buzzard.ready_before, residual_buzzard.ready_before);
    assert!(retained_buzzard.enqueued_at >= later_state.current_tick());
    assert_eq!(brain.mind().strategy.air_admitted_at(), Some(admitted_at));
    assert!(
        matches!(
            retained_buzzard.owner,
            crate::trace::ClaimOwnerTrace::Proposal {
                key: crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
            }
        ),
        "optional growth is adjudicated afresh rather than promoted to mandatory debt"
    );
    let matching_commands = later
        .commands
        .iter()
        .filter(|command| {
            matches!(
                command.command,
                Command::Train { building, kind }
                    if building == residual_buzzard.producer
                        && kind == residual_buzzard.kind
            )
        })
        .count();
    assert_eq!(
        matching_commands,
        usize::from(retained_buzzard.enqueued_at == later_state.current_tick()),
        "the retained Buzzard must dispatch exactly on its allocated enqueue tick"
    );
    let later_raw = Observation::fog_honest(&later_state, PlayerId(0));
    assert_eq!(
        brain
            .policy
            .validated_foundry_saving(&orientation.observe(&later_raw), true),
        saved,
        "the earlier operation spends ahead of, but does not erase, the saved Foundry"
    );
}

#[test]
fn one_allocation_dispatches_compatible_foundry_and_connected_offense_commands() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let mut scenario = foundry_saving_air_competition_scenario(
        foundry_cost
            .saturating_add(UnitKind::Buzzard.stats().cost)
            .saturating_add(UnitKind::Sentinel.stats().cost)
            .saturating_add(
                BuildingKind::Turret
                    .base_stats()
                    .construction
                    .expect("Turrets are constructible")
                    .cost,
            )
            .saturating_add(UnitKind::Harvester.stats().cost),
    );
    scenario.name = "simultaneous Foundry and connected offense dispatch".into();
    scenario
        .buildings
        .extend((0..8).map(|index| building_spec(0, BuildingKind::Reclaimer, 16 + index * 3, 2)));
    let scout = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the scenario has one connected-operation scout");
    (scout.x, scout.y) = (42, 19);
    scenario.units.extend([
        unit_spec(0, UnitKind::Bombard, 9, 17),
        unit_spec(0, UnitKind::Condor, 10, 18),
        unit_spec(0, UnitKind::Condor, 11, 18),
    ]);
    let mut state = scenario
        .build()
        .expect("the simultaneous allocation scenario builds");
    let mut brain = foundry_competition_brain(&scenario);

    let act = brain.act_traced(&state);
    let trace = act.trace.expect("the shared admission boundary is traced");
    assert!(
        act.commands.iter().any(|command| matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Foundry,
                ..
            }
        )),
        "the accepted expansion must lower its exact build command: {:?}",
        act.commands
    );
    assert!(trace.allocation.error.is_none());
    assert!(trace.allocation.coordinator_failure.is_none());
    let accepted_keys = trace
        .allocation
        .proposals
        .entries
        .iter()
        .filter(|proposal| proposal.disposition == crate::trace::ProposalDispositionTrace::Accepted)
        .map(|proposal| proposal.key)
        .collect::<Vec<_>>();
    assert_eq!(accepted_keys.len(), 4);
    assert!(
        accepted_keys
            .iter()
            .any(|key| matches!(key, crate::trace::ProposalKeyTrace::FoundryExpansion { .. }))
    );
    assert!(accepted_keys.iter().any(|key| matches!(
        key,
        crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
    )));
    assert!(
        accepted_keys
            .iter()
            .any(|key| matches!(key, crate::trace::ProposalKeyTrace::StandingForce { .. }))
    );
    assert!(
        accepted_keys
            .iter()
            .any(|key| matches!(key, crate::trace::ProposalKeyTrace::Defense { .. }))
    );
    let connected_jobs = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                crate::trace::ClaimOwnerTrace::Proposal {
                    key: crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. },
                }
            )
        })
        .collect::<Vec<_>>();
    assert!(
        !connected_jobs.is_empty()
            && connected_jobs
                .iter()
                .any(|job| job.enqueued_at == state.current_tick())
            && connected_jobs
                .iter()
                .any(|job| job.enqueued_at > state.current_tick()),
        "the accepted connected scale must retain both its current append and exact future producer schedule: {connected_jobs:?}"
    );
    let accepted_due = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| job.enqueued_at == state.current_tick())
        .map(|job| (job.producer, job.kind))
        .collect::<Vec<_>>();
    let lowered = act
        .commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Train { building, kind } => Some((building, kind)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        lowered.starts_with(&accepted_due),
        "allocated current producer work must lower first in exact schedule order: accepted={accepted_due:?}, lowered={lowered:?}"
    );

    let report = state.tick(&act.commands);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "the authoritative State must accept all allocated commands in one transaction: {:?}",
        report.events
    );
}

#[test]
fn recon_promoted_to_assault_keeps_its_original_foundry_priority_in_brain() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
    scenario.buildings.extend((0..16).map(|index| {
        building_spec(
            0,
            BuildingKind::Reclaimer,
            16 + (index % 8) * 4,
            2 + (index / 8) * 3,
        )
    }));
    let mut state = scenario
        .build()
        .expect("the Foundry-saving air competition scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    let raw = Observation::fog_honest(&state, PlayerId(0));
    assert!(raw.enemy_buildings.is_empty());
    let enemy_foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .expect("the remembered enemy Foundry stands");
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);

    let mut brain = foundry_competition_brain(&scenario);
    brain.mind_mut().strategy = StrategicPlanner::new();

    brain.orientation = Some(orientation);
    let prior = prior_foundry_sighting(&raw, enemy_foundry, raw.tick.saturating_sub(100));
    brain
        .mind_mut()
        .intelligence
        .update(&orientation.observe(&prior));

    let admitted = brain.act_traced(&state);
    let (admitted_at, initial_started_at) = {
        let planner = &brain.mind().strategy;
        let operation = planner
            .air_operation()
            .expect("the remembered objective admits reconnaissance");
        assert!(!operation.assault_admitted());
        (
            planner
                .air_admitted_at()
                .expect("the reconnaissance records its immutable admission"),
            operation.started_at,
        )
    };
    assert_eq!(admitted_at, state.current_tick());
    assert_eq!(initial_started_at, admitted_at);
    let oriented = orientation.observe(&raw);
    let saved = brain.policy.validated_foundry_saving(&oriented, true);
    let saved_site = brain
        .policy
        .foundry_builder_lease(&oriented)
        .expect("the saved expansion retains its builder and site")
        .anchor();
    assert!(saved > state.player(PlayerId(0)).scrap);
    assert!(brain.policy.operation_precedes_foundry_saving(admitted_at));
    assert_eq!(
        admitted
            .trace
            .expect("the reconnaissance admission is traced")
            .channels
            .connected_air
            .after,
        ChannelState::Active(ChannelPhase::AirRecon)
    );

    let mut visible_scenario = scenario.clone();
    visible_scenario
        .buildings
        .push(building_spec(0, BuildingKind::Array, 40, 19));
    let scout = visible_scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the continuation retains the reconnaissance aircraft");
    (scout.x, scout.y) = (42, 19);
    let mut visible_state = visible_scenario
        .build()
        .expect("the current-sight continuation builds");
    crate::test_support::set_tick(
        &mut visible_state,
        crate::difficulty::next_strategic_admission_tick(state.current_tick()),
    );
    crate::test_support::edit_player(&mut visible_state, PlayerId(0), |item| {
        item.scrap = saved - 1
    });

    let promoted = brain.act_traced(&visible_state);
    let (preserved_admission, restarted_at) = {
        let planner = &brain.mind().strategy;
        let operation = planner
            .air_operation()
            .expect("current sight promotes the reconnaissance");
        assert!(
            operation.assault_admitted(),
            "current reconnaissance must promote through its retained allocation priority: {:?}",
            promoted.trace
        );
        (
            planner
                .air_admitted_at()
                .expect("the promoted operation retains its admission"),
            operation.started_at,
        )
    };
    assert_eq!(preserved_admission, admitted_at);
    assert_eq!(restarted_at, visible_state.current_tick());
    assert!(restarted_at > admitted_at);
    assert!(brain.policy.operation_precedes_foundry_saving(admitted_at));
    assert!(!brain.policy.operation_precedes_foundry_saving(restarted_at));
    assert!(promoted.trace.is_some());

    let report = visible_state.tick(&promoted.commands);
    assert_commands_accepted(&report, PlayerId(0));
    let next_admission =
        crate::difficulty::next_strategic_admission_tick(visible_state.current_tick());
    while visible_state.current_tick() < next_admission {
        visible_state.tick(&[]);
    }
    let continued = loop {
        crate::test_support::edit_player(&mut visible_state, PlayerId(0), |item| {
            item.scrap = saved - 1
        });
        let decision = brain.act_traced(&visible_state);
        if decision
            .trace
            .as_ref()
            .is_some_and(|trace| trace.budget.as_ref().is_some_and(|budget| !budget.frozen))
        {
            break decision;
        }
        assert!(
            visible_state.current_tick() < next_admission + 120,
            "retained procurement must finish refinement: {:?}",
            decision.trace
        );
        visible_state.tick(&decision.commands);
        while !visible_state
            .current_tick()
            .is_multiple_of(brain.dials.cadence)
        {
            visible_state.tick(&[]);
        }
    };
    let trace = continued
        .trace
        .expect("the post-promotion decision is traced");
    let budget = trace
        .budget
        .expect("the post-promotion decision records its budget");
    assert_eq!(budget.foundry_saving, saved);
    assert_eq!(budget.strategic_spendable, 0);
    assert!(trace.allocation.coordinator_failure.is_none());
    let connected_jobs = trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            matches!(
                job.owner,
                crate::trace::ClaimOwnerTrace::Obligation {
                    accepted_at,
                    key: crate::trace::ObligationKeyTrace::ConnectedOffense { .. },
                    ..
                } if accepted_at == admitted_at
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        !connected_jobs.is_empty(),
        "the promoted operation must retain its procurement priority: {trace:#?}"
    );
    assert!(
        connected_jobs
            .iter()
            .all(|job| job.enqueued_at >= visible_state.current_tick())
    );
    let due_now = connected_jobs
        .iter()
        .filter(|job| job.enqueued_at == visible_state.current_tick())
        .cloned()
        .collect::<Vec<_>>();
    let future = connected_jobs
        .iter()
        .filter(|job| job.enqueued_at > visible_state.current_tick())
        .cloned()
        .collect::<Vec<_>>();
    let mut connected_pairs = connected_jobs
        .iter()
        .map(|job| (job.producer, job.kind))
        .collect::<Vec<_>>();
    connected_pairs.sort_unstable();
    connected_pairs.dedup();
    for (producer, kind) in connected_pairs {
        let expected = due_now
            .iter()
            .filter(|job| job.producer == producer && job.kind == kind)
            .count();
        let actual = continued
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train { building, kind: trained }
                        if building == producer && trained == kind
                )
            })
            .count();
        assert_eq!(
            actual, expected,
            "only connected producer jobs due now may dispatch"
        );
    }

    let report = visible_state.tick(&continued.commands);
    assert_commands_accepted(&report, PlayerId(0));
    let Some(due_at) = future.iter().map(|job| job.enqueued_at).min() else {
        assert!(
            !due_now.is_empty(),
            "all retained demand was purchased this decision"
        );
        return;
    };
    let future_due = future
        .iter()
        .filter(|job| job.enqueued_at == due_at)
        .cloned()
        .collect::<Vec<_>>();
    while visible_state.current_tick() < due_at {
        visible_state.tick(&[]);
    }
    let due = brain.act_traced(&visible_state);
    let due_trace = due.trace.as_ref().expect("the future dispatch is traced");
    for expected in &future_due {
        assert!(
            due_trace
                .allocation
                .producer_schedule
                .entries
                .iter()
                .any(|job| {
                    job.owner == expected.owner
                        && job.producer == expected.producer
                        && job.kind == expected.kind
                        && job.ready_before == expected.ready_before
                }),
            "the retained force remains funded before its original deadline: expected {expected:#?}; {due_trace:#?}"
        );
    }
    let mut due_pairs = future_due
        .iter()
        .map(|job| (job.producer, job.kind))
        .collect::<Vec<_>>();
    due_pairs.sort_unstable();
    due_pairs.dedup();
    for (producer, kind) in due_pairs {
        let expected = due_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                job.enqueued_at == visible_state.current_tick()
                    && job.producer == producer
                    && job.kind == kind
            })
            .count();
        let actual = due
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train { building, kind: trained }
                        if building == producer && trained == kind
                )
            })
            .count();
        assert_eq!(
            actual, expected,
            "each current allocation row must emit once; earlier forecasts may change"
        );
    }
    let report = visible_state.tick(&due.commands);
    assert_commands_accepted(&report, PlayerId(0));
    let next_think = due_at.saturating_add(brain.dials.cadence);
    while visible_state.current_tick() < next_think {
        visible_state.tick(&[]);
    }
    let after_due = brain.act_traced(&visible_state);
    let after_due_trace = after_due
        .trace
        .expect("the post-dispatch continuation is traced");
    for issued in future_due {
        assert!(
            after_due_trace
                .allocation
                .producer_schedule
                .entries
                .iter()
                .all(|job| {
                    job.owner != issued.owner
                        || job.request_ordinal != issued.request_ordinal
                        || job.producer != issued.producer
                        || job.kind != issued.kind
                        || job.enqueued_at != issued.enqueued_at
                        || job.starts_at != issued.starts_at
                        || job.ready_at != issued.ready_at
                        || job.ready_before != issued.ready_before
                }),
            "an issued connected assignment must not re-enter allocation"
        );
    }
    let continued_raw = orientation.observe(&Observation::fog_honest(&visible_state, PlayerId(0)));
    let retained = brain.policy.validated_foundry_saving(&continued_raw, true);
    if retained == 0 {
        assert!(
            continued_raw
                .my_buildings
                .iter()
                .any(|building| building.kind == BuildingKind::Foundry
                    && building.anchor == saved_site),
            "the original expansion may release its reserve only after paying for its exact foundation"
        );
    } else {
        assert_eq!(retained, saved);
    }
}

#[test]
fn brain_funds_bulk_lifts_according_to_foundry_admission_order() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let shallow_guard = UnitKind::Sentinel.stats().cost;

    let mut earlier_scenario = foundry_saving_lift_competition_scenario(
        foundry_cost.saturating_add(shallow_guard).saturating_sub(1),
    );
    let last_carrier = earlier_scenario
        .units
        .iter()
        .rposition(|unit| unit.player == 0 && unit.kind == UnitKind::Skyhook)
        .expect("the lift fixture begins with multiple carriers");
    earlier_scenario.units.remove(last_carrier);
    earlier_scenario
        .buildings
        .extend((0..4).map(|index| building_spec(0, BuildingKind::Reclaimer, 2 + index * 3, 6)));
    let mut earlier_state = earlier_scenario
        .build()
        .expect("the earlier-lift competition scenario builds");
    let raw = Observation::fog_honest(&earlier_state, PlayerId(0));
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);
    assert!(orientation.is_identity());
    let lift = seeded_bulk_lift(&earlier_state, orientation);
    let lift_admitted_at = lift
        .operation()
        .expect("the earlier lift is active")
        .started_at;
    let mut earlier_brain = foundry_competition_brain(&earlier_scenario);
    earlier_brain.orientation = Some(orientation);

    earlier_brain.mind_mut().lifts = lift;

    let mut direct_earlier_brain = earlier_brain.clone();
    let direct_continued = direct_earlier_brain.act(&earlier_state);
    let continued = earlier_brain.act_traced(&earlier_state);
    assert_eq!(continued.commands, direct_continued);
    assert_brain_unchanged(&direct_earlier_brain, &earlier_brain);
    let accepted_raw = Observation::fog_honest(&earlier_state, PlayerId(0));
    let accepted_oriented = orientation.observe(&accepted_raw);
    let saved = earlier_brain
        .policy
        .validated_foundry_saving(&accepted_oriented, true);
    assert!(
        saved
            > earlier_state
                .player(PlayerId(0))
                .scrap
                .saturating_sub(shallow_guard),
        "the earlier lift fixture must leave the accepted Foundry underfunded after preserving the shallow screen: saved={saved}, scrap={}, guard={shallow_guard}, trace={:?}",
        earlier_state.player(PlayerId(0)).scrap,
        continued.trace,
    );
    assert!(
        earlier_brain
            .policy
            .operation_precedes_foundry_saving(lift_admitted_at)
    );
    let continued_trace = continued
        .trace
        .expect("the older lift continuation is traced");
    assert!(
        continued_trace.allocation.error.is_none()
            && continued_trace.allocation.coordinator_failure.is_none()
    );
    assert!(
        continued_trace
            .allocation
            .proposals
            .entries
            .iter()
            .any(|proposal| matches!(
                (proposal.key, &proposal.disposition,),
                (
                    crate::trace::ProposalKeyTrace::FoundryExpansion { .. },
                    crate::trace::ProposalDispositionTrace::Accepted,
                )
            ))
    );
    let lift_job = continued_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .find(|job| {
            job.kind == UnitKind::Skyhook
                && matches!(
                    job.owner,
                    crate::trace::ClaimOwnerTrace::Obligation {
                        accepted_at,
                    key: crate::trace::ObligationKeyTrace::LiftProduction,
                        ..
                    } if accepted_at == lift_admitted_at
                )
        })
        .expect("the older lift retains one exact future carrier")
        .clone();
    let lift_request_ordinal = usize::try_from(lift_job.request_ordinal)
        .expect("the traced Lift ordinal fits the planner's index space");
    let retained = (earlier_brain.mind().lifts)
        .operation()
        .expect("the Lift remains active")
        .producer_assignments
        .iter()
        .find(|assignment| assignment.request_ordinal() == lift_request_ordinal)
        .expect("the exact selected carrier schedule is retained by the Lift");
    assert_eq!(retained.producer(), lift_job.producer);
    assert_eq!(retained.kind(), lift_job.kind);
    assert_eq!(retained.timing().enqueued_at(), lift_job.enqueued_at);
    assert_eq!(retained.timing().starts_at(), lift_job.starts_at);
    assert_eq!(retained.timing().ready_at(), lift_job.ready_at);
    assert_eq!(retained.timing().ready_before(), lift_job.ready_before);
    assert!(lift_job.enqueued_at > earlier_state.current_tick());
    assert_eq!(
        lift_job
            .current_scrap
            .saturating_add(lift_job.forecast_scrap),
        UnitKind::Skyhook.stats().cost
    );
    let due_now = continued_trace
        .allocation
        .producer_schedule
        .entries
        .iter()
        .filter(|job| {
            job.producer == lift_job.producer
                && job.kind == UnitKind::Skyhook
                && job.enqueued_at == earlier_state.current_tick()
        })
        .count();
    let issued_now = continued
        .commands
        .iter()
        .filter(|command| {
            matches!(
                command.command,
                Command::Train {
                    building,
                    kind: UnitKind::Skyhook,
                } if building == lift_job.producer
            )
        })
        .count();
    assert_eq!(due_now, 1, "the older lift retains one due carrier");
    assert_eq!(
        issued_now, due_now,
        "only producer work due on this observation may lower; the future carrier remains bound"
    );
    let report = earlier_state.tick(&continued.commands);
    assert_commands_accepted(&report, PlayerId(0));
    while earlier_state.current_tick() < lift_job.enqueued_at {
        earlier_state.tick(&[]);
    }
    let mut direct_due_brain = earlier_brain.clone();
    let direct_due = direct_due_brain.act(&earlier_state);
    let due = earlier_brain.act_traced(&earlier_state);
    assert_eq!(due.commands, direct_due);
    assert_brain_unchanged(&direct_due_brain, &earlier_brain);
    let due_trace = due.trace.as_ref().expect("the due carrier is traced");
    assert!(
        due_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .any(|job| {
                job.owner == lift_job.owner
                    && job.producer == lift_job.producer
                    && job.kind == lift_job.kind
                    && job.request_ordinal == lift_job.request_ordinal
                    && job.enqueued_at == lift_job.enqueued_at
                    && job.starts_at == lift_job.starts_at
                    && job.ready_at == lift_job.ready_at
                    && job.ready_before == lift_job.ready_before
            })
    );
    assert_eq!(
        due.commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Train {
                    building,
                    kind: UnitKind::Skyhook,
                } if building == lift_job.producer
            ))
            .count(),
        1,
        "the older lift's exact carrier must dispatch once on its allocated tick"
    );
    assert_eq!(
        (earlier_brain.mind().lifts)
            .operation()
            .expect("the Lift remains active while its carrier trains")
            .issued_producers,
        vec![lift_request_ordinal],
    );
    let report = earlier_state.tick(&due.commands);
    assert_commands_accepted(&report, PlayerId(0));
    let after_due_tick = lift_job
        .enqueued_at
        .saturating_add(earlier_brain.dials.cadence);
    while earlier_state.current_tick() < after_due_tick {
        earlier_state.tick(&[]);
    }
    let after_due = earlier_brain.act_traced(&earlier_state);
    assert!(after_due.commands.iter().all(|command| !matches!(
        command.command,
        Command::Train {
            building,
            kind: UnitKind::Skyhook,
        } if building == lift_job.producer
    )));

    let later_scenario = foundry_saving_lift_competition_scenario(foundry_cost - 1);
    let mut later_state = later_scenario
        .build()
        .expect("the later-lift competition scenario builds");
    let mut later_brain = foundry_competition_brain(&later_scenario);

    let first_commands = later_brain.act(&later_state);
    let later_orientation = later_brain
        .orientation
        .expect("the saving-first think latches its orientation");
    let first_raw = Observation::fog_honest(&later_state, PlayerId(0));
    let first_oriented = later_orientation.observe(&first_raw);
    let later_saved = later_brain
        .policy
        .validated_foundry_saving(&first_oriented, true);
    assert!(later_saved > later_state.player(PlayerId(0)).scrap);
    let report = later_state.tick(&first_commands);
    assert_commands_accepted(&report, PlayerId(0));
    while !later_state
        .current_tick()
        .is_multiple_of(later_brain.dials.cadence)
        || !crate::difficulty::strategic_admission_tick(later_state.current_tick())
    {
        later_state.tick(&[]);
    }
    crate::test_support::edit_player(&mut later_state, PlayerId(0), |item| {
        item.scrap = later_saved - 1
    });
    let later_lift = seeded_bulk_lift(&later_state, later_orientation);
    let later_lift_admitted_at = later_lift
        .operation()
        .expect("the later lift is active")
        .started_at;
    assert!(
        !later_brain
            .policy
            .operation_precedes_foundry_saving(later_lift_admitted_at)
    );
    later_brain.mind_mut().lifts = later_lift;

    let blocked = later_brain.act_traced(&later_state);
    let blocked_trace = blocked
        .trace
        .expect("the later lift continuation is traced");
    let blocked_budget = blocked_trace
        .budget
        .expect("the later lift continuation records its budget");
    assert_eq!(blocked_budget.foundry_saving, later_saved);
    assert_eq!(
        blocked_budget.prior_operation_spendable, 0,
        "a lift accepted after the Foundry saving receives no predecessor allowance"
    );
    assert_eq!(blocked_budget.strategic_spendable, 0);
    assert_eq!(blocked_trace.channels.lift.effects.committed_scrap, 0);
    assert!(blocked.commands.iter().all(|command| !matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Skyhook,
            ..
        }
    )));
    let blocked_raw = Observation::fog_honest(&later_state, PlayerId(0));
    assert_eq!(
        later_brain
            .policy
            .validated_foundry_saving(&later_orientation.observe(&blocked_raw), true),
        later_saved
    );
}

#[test]
fn mirrored_brain_dispatches_and_releases_the_exact_foundry_lease() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let scenario = mirrored_foundry_saving_scenario(foundry_cost - 1);
    let mut state = scenario
        .build()
        .expect("the mirrored Foundry-saving scenario builds");
    let mut brain = foundry_competition_brain(&scenario);

    let first_commands = brain.act(&state);
    let orientation = brain
        .orientation
        .expect("the southeast home latches an orientation");
    assert!(!orientation.is_identity());
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let oriented = orientation.observe(&raw);
    let saved = brain.policy.validated_foundry_saving(&oriented, true);
    assert!(saved > state.player(PlayerId(0)).scrap);
    let lease = brain
        .policy
        .foundry_builder_lease(&oriented)
        .expect("the accepted expansion owns one exact canonical lease");
    let world_anchor = orientation.anchor(lease.anchor(), lease.kind().base_stats().size);
    assert_ne!(world_anchor, lease.anchor());

    let first_report = state.tick(&first_commands);
    assert!(first_report.events.iter().all(|event| !matches!(
        event,
        oxide_sim::event::Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
    while !state.current_tick().is_multiple_of(brain.dials.cadence)
        || !crate::difficulty::strategic_admission_tick(state.current_tick())
    {
        state.tick(&[]);
    }
    crate::test_support::edit_player(&mut state, PlayerId(0), |item| item.scrap = saved);

    let result = brain.act_traced(&state);
    let commands = result.commands;
    assert!(
        commands.iter().any(|command| matches!(
            &command.command,
            Command::Build {
                units,
                kind: BuildingKind::Foundry,
                anchor,
                ..
            } if units == &[lease.builder()] && *anchor == world_anchor
        )),
        "the canonical lease must lower to its exact mirrored world command: {commands:?}; trace={:?}",
        result.trace
    );
    let funded_raw = Observation::fog_honest(&state, PlayerId(0));
    let funded_oriented = orientation.observe(&funded_raw);
    assert_eq!(
        brain
            .policy
            .validated_foundry_saving(&funded_oriented, true),
        0,
        "dispatching the exact mirrored command clears the persistent saving"
    );
    assert!(
        brain
            .policy
            .foundry_builder_lease(&funded_oriented)
            .is_none()
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
        "the mirrored lease must be legal in authoritative world space: {:?}",
        report.events
    );
}

pub(super) fn mirrored_foundry_saving_scenario(scrap: u32) -> Scenario {
    let mut scenario = foundry_saving_air_competition_scenario(scrap);
    scenario.name = "mirrored Foundry saving dispatch".into();
    let width = i32::try_from(scenario.map[0].len()).expect("fixture width fits i32");
    let height = i32::try_from(scenario.map.len()).expect("fixture height fits i32");
    for row in &mut scenario.map {
        *row = row
            .chars()
            .map(|tile| {
                if matches!(tile, '1' | '2' | 'E') {
                    '.'
                } else {
                    tile
                }
            })
            .collect();
    }
    for (anchor, marker) in [
        (TilePos::new(44, 20), b'1'),
        (TilePos::new(1, 1), b'2'),
        (TilePos::new(16, 6), b'E'),
    ] {
        let row = scenario
            .map
            .get_mut(anchor.y as usize)
            .expect("the mirrored marker row exists");
        let mut bytes = row.as_bytes().to_vec();
        bytes[anchor.x as usize] = marker;
        *row = String::from_utf8(bytes).expect("the mirrored fixture map remains ASCII");
    }
    for unit in &mut scenario.units {
        unit.x = width - 1 - unit.x;
        unit.y = height - 1 - unit.y;
    }
    for building in &mut scenario.buildings {
        let size = building.kind.base_stats().size;
        building.x = width - size.0 - building.x;
        building.y = height - size.1 - building.y;
    }
    scenario
}
