use super::*;

#[test]
fn interrupted_unsafe_turret_is_resolved_through_an_ordinary_state_command() {
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 1_000;
    scenario
        .buildings
        .push(building_spec(1, BuildingKind::Turret, 12, 5));
    let mut state = scenario
        .build()
        .expect("the interrupted-site fixture builds");
    let requested_builder = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .expect("the home economy has a builder")
        .id;
    let anchor = TilePos::new(7, 5);
    let placed = state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Build {
            units: vec![requested_builder],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: false,
        },
    }]);
    assert!(
        placed
            .events
            .iter()
            .all(|event| !matches!(event, oxide_sim::event::Event::CommandRejected { .. })),
        "the paid Turret site must enter through the ordinary command boundary: {placed:?}"
    );
    let site = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Turret
                && building.anchor == anchor
        })
        .expect("the accepted command placed the intended Turret")
        .id;

    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 9_001);
    let mut brain = scripted_brain(&scenario, PlayerId(0), config);
    while state.current_tick() < 100 && state.building(site).unwrap().progress == 0 {
        state.tick(&[]);
    }
    assert!(state.building(site).unwrap().progress > 0);
    while !state.current_tick().is_multiple_of(brain.dials.cadence) {
        state.tick(&[]);
    }
    let active_builder = state
        .units()
        .iter()
        .find(|unit| matches!(unit.order, oxide_sim::state::Order::Build { site: target } if target == site))
        .map(|unit| unit.id)
        .expect("the builder is still working when danger interrupts the site");
    let evacuation = brain.act(&state);
    assert!(
        evacuation.iter().any(|command| matches!(
            &command.command,
            Command::Move { units, .. } if units.contains(&active_builder)
        )),
        "the visible gun must evacuate the active builder: {evacuation:?}"
    );
    assert!(
        evacuation.iter().all(|command| !matches!(
            command.command,
            Command::Cancel { building } if building == site
        )),
        "the staffed site is interrupted before it becomes an orphan"
    );
    state.tick(&evacuation);

    while !state.current_tick().is_multiple_of(brain.dials.cadence) {
        state.tick(&[]);
    }
    assert!(matches!(
        state
            .unit(active_builder)
            .expect("the active builder survived")
            .order,
        oxide_sim::state::Order::Idle | oxide_sim::state::Order::Move { .. }
    ));
    assert!(
        state.building(site).is_some_and(|building| !building.built),
        "the interruption must leave a paid unfinished site"
    );

    let deadline = state.current_tick() + brain.dials.cadence * 4;
    let mut resolution = brain.act(&state);
    while resolution.is_empty() && state.current_tick() < deadline {
        state.tick(&[]);
        resolution = brain.act(&state);
    }
    assert!(
        resolution.iter().any(|command| matches!(
            command.command,
            Command::Cancel { building } if building == site
        )),
        "an observably unsafe orphan Turret must be resolved instead of decaying forever: {resolution:?}"
    );
    let resolved = state.tick(&resolution);
    assert!(
        resolved
            .events
            .iter()
            .all(|event| !matches!(event, oxide_sim::event::Event::CommandRejected { .. })),
        "the ordinary cancellation must be accepted by State: {resolved:?}"
    );
    assert!(state.building(site).is_none());
    assert!(
        resolved.events.iter().any(|event| matches!(
            event,
            oxide_sim::event::Event::BuildCancelled {
                building,
                player: PlayerId(0),
                refund,
            } if *building == site && *refund > 0
        )),
        "the ordinary partial-refund event must account for the abandoned investment: {resolved:?}"
    );
}

#[test]
fn primary_island_operations_claim_their_force_without_fragmenting_a_raid() {
    let scenario = combined_operation_scenario();
    let mut state = scenario
        .build()
        .expect("combined-operation scenario builds");
    for _ in 0..6_000 {
        state.tick(&[]);
    }

    let mut brain = operation_identity_brain(PlayerId(0), &scenario);
    enlist_opening_core(&mut brain, &state);

    let result = brain.act_traced(&state);
    let commands = result.commands;

    let air = (brain.mind().strategy)
        .air_operation()
        .expect("the wealthy disconnected match starts the bomber operation");
    let lift = (brain.mind().lifts)
        .operation()
        .expect("the same match starts its coordinated bulk lift");
    assert!(lift.desired_carriers >= 8);
    assert!(lift.payload.len() >= 32);
    assert_eq!(
        (lift.target_player, lift.target),
        (air.target_player, air.target),
        "the second-starting lift must inherit the air operation's exact objective"
    );
    assert_eq!(lift.planned_drops.len(), lift.desired_carriers);
    assert!(
        (brain.mind().raids).operation().is_none(),
        "simultaneous air and lift work must consume Prime's optional-operation attention too"
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
        "all combined-operation commands must remain ordinary legal commands: {:?}; commands={commands:?}",
        report.events
    );
}

#[test]
fn difficulty_attention_limits_competing_operations_without_dropping_active_raids() {
    let scenario = combined_operation_scenario();
    let mut prepared = scenario
        .build()
        .expect("combined-operation scenario builds");
    for _ in 0..6_000 {
        prepared.tick(&[]);
    }

    for difficulty in BotDifficulty::ALL {
        let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        enlist_opening_core(&mut brain, &prepared);
        let result = brain.act_traced(&prepared);
        let trace = result
            .trace
            .expect("the competing-operation decision is traced");
        assert!(brain.mind().strategy.air_operation().is_some());
        assert!(brain.mind().lifts.operation().is_some());
        assert!(brain.mind().raids.operation().is_none());
        let attention = trace
            .gates
            .raid_attention
            .expect("admission evaluates optional attention");
        assert_eq!(attention.strategic_load, 2);
        assert!(
            !attention.admitted,
            "{difficulty:?} cannot add a third operation"
        );
    }

    for difficulty in BotDifficulty::ALL {
        let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        enlist_opening_core(&mut brain, &prepared);

        let profile = *brain.profile();
        let tuning = DifficultyTuning::for_level(difficulty);
        let obs = Observation::fog_honest(&prepared, PlayerId(0));
        let home = obs
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the home Foundry stands")
            .anchor;
        let mut prior_raid = RaidPlanner::new();
        let raid_start = prior_raid.think_unrestricted(&profile, tuning, &obs, home, &[], &[]);
        assert!(matches!(
            raid_start.intents.as_slice(),
            [Intent::AttackMoveUnits { .. }]
        ));
        let prior_members = prior_raid
            .operation()
            .expect("zero load admits the raid on every rung")
            .members
            .clone();
        brain.mind_mut().raids = prior_raid;

        let _ = brain.act(&prepared);

        assert!(
            (brain.mind().strategy).air_operation().is_some(),
            "{difficulty:?} must start the air operation in the continuation fixture"
        );
        assert!(
            (brain.mind().lifts).operation().is_some(),
            "{difficulty:?} must start the lift operation in the continuation fixture"
        );
        assert_eq!(
            (brain.mind().raids)
                .operation()
                .expect("attention limits cannot suspend a claimed raid")
                .members,
            prior_members,
            "{difficulty:?}"
        );
    }
}

#[test]
fn stale_active_recon_releases_its_carrier_floor_before_defense_allocation() {
    let mut scenario = prospective_lift_reservation_scenario();
    scenario.name = "stale Recon releases prospective carrier".into();
    scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
    scenario
        .buildings
        .retain(|building| building.kind != BuildingKind::Foundry);
    scenario
        .units
        .push(unit_spec(1, UnitKind::Sentinel, 18, 19));
    let mut state = scenario
        .build()
        .expect("stale prospective-lift scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    let last_seen = state.current_tick().saturating_sub(100);
    let mut brain = brain_with_remembered_lift_target(&scenario, &state, last_seen);

    let admitted = brain.act_traced(&state);
    assert_eq!(
        admitted
            .trace
            .as_ref()
            .and_then(|trace| trace.budget.as_ref())
            .map(|budget| budget.prospective_carrier),
        Some(UnitKind::Skyhook.stats().cost)
    );
    assert_eq!(
        (brain.mind().strategy)
            .air_operation()
            .map(|operation| operation.phase()),
        Some(AirOperationPhase::Recon)
    );

    crate::test_support::set_tick(&mut state, last_seen.saturating_add(541));
    while !state.current_tick().is_multiple_of(24) {
        let next_tick = state.current_tick().saturating_add(1);
        crate::test_support::set_tick(&mut state, next_tick);
    }
    let released = brain.act_traced(&state);
    let trace = released
        .trace
        .as_ref()
        .expect("the stale recovery decision is traced");
    assert_eq!(
        trace
            .budget
            .as_ref()
            .expect("the stale decision retains budget evidence")
            .prospective_carrier,
        0,
        "stale Recon must not impose a phantom Skyhook floor"
    );
    assert!(
        trace.allocation.proposals.entries.iter().any(|proposal| {
            matches!(proposal.key, crate::trace::ProposalKeyTrace::Defense { .. })
                && proposal.disposition == crate::trace::ProposalDispositionTrace::Accepted
        }),
        "the released carrier bank should remain eligible for the best defensive quote: {trace:#?}"
    );
    let operation = (brain.mind().strategy)
        .air_operation()
        .expect("the stale operation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::StaleIntelligence)
    );
}

#[test]
fn unreachable_remembered_recon_never_imposes_a_carrier_floor_on_defense() {
    let mut scenario = prospective_lift_reservation_scenario();
    scenario.name = "unreachable Recon releases prospective carrier".into();
    scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
    scenario
        .buildings
        .retain(|building| building.kind != BuildingKind::Foundry);
    scenario
        .units
        .push(unit_spec(1, UnitKind::Sentinel, 18, 19));
    let sealed_scout = TilePos::new(18, 2);
    scenario.map = scenario
        .map
        .iter()
        .enumerate()
        .map(|(y, row)| {
            row.chars()
                .enumerate()
                .map(|(x, terrain)| {
                    let tile = TilePos::new(
                        i32::try_from(x).expect("the focused map width fits i32"),
                        i32::try_from(y).expect("the focused map height fits i32"),
                    );
                    if sealed_scout.chebyshev(tile) == 2 {
                        '^'
                    } else {
                        terrain
                    }
                })
                .collect()
        })
        .collect();
    let mut state = scenario
        .build()
        .expect("peak-sealed prospective-lift scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    let mut brain = brain_with_remembered_lift_target(
        &scenario,
        &state,
        state.current_tick().saturating_sub(100),
    );

    let released = brain.act_traced(&state);
    let trace = released
        .trace
        .as_ref()
        .expect("the unreachable recovery decision is traced");
    assert_eq!(
        trace
            .budget
            .as_ref()
            .expect("the unreachable decision retains budget evidence")
            .prospective_carrier,
        0,
        "a peak-sealed scout route must not impose a phantom Skyhook floor"
    );
    assert!(
        trace.allocation.proposals.entries.iter().any(|proposal| {
            matches!(proposal.key, crate::trace::ProposalKeyTrace::Defense { .. })
                && proposal.disposition == crate::trace::ProposalDispositionTrace::Accepted
        }),
        "the released carrier bank should remain eligible for the best defensive quote: {:?}",
        trace.allocation.proposals
    );
    let strategy = &brain.mind().strategy;
    assert!(
        strategy.air_operation().is_some_and(|operation| {
            operation.phase() == AirOperationPhase::Recover
                && operation.recovery_reason() == Some(AirRecoveryReason::UnreachableAirRoute)
                && !operation.assault_admitted()
        }) || matches!(
            strategy.terminal_outcome(),
            Some(AirOperationOutcome::Aborted { .. })
        ),
        "the route refusal must enter or complete bounded recovery"
    );
}

#[test]
fn fresh_raid_claiming_the_only_lift_payload_releases_the_carrier_hold() {
    let mut scenario = prospective_lift_reservation_scenario();
    scenario.name = "fresh raid releases prospective carrier".into();
    scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
    let mut retained_sentinels = 0usize;
    scenario.units.retain(|unit| {
        if unit.player != 0 || unit.kind != UnitKind::Sentinel {
            return true;
        }
        retained_sentinels += 1;
        retained_sentinels <= 10
    });
    scenario
        .units
        .extend([9, 10].map(|y| unit_spec(0, UnitKind::Scuttler, 16, y)));
    let mut state = scenario
        .build()
        .expect("prospective raid/lift scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);

    let raw = Observation::fog_honest(&state, PlayerId(0));
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);
    let mut brain = scripted_brain(
        &scenario,
        PlayerId(0),
        BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_024),
    );
    brain.orientation = Some(orientation);
    let enemy_foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .expect("the enemy Foundry stands beyond the barrier");
    let prior = prior_foundry_sighting(&raw, enemy_foundry, raw.tick.saturating_sub(100));
    brain
        .mind_mut()
        .intelligence
        .update(&orientation.observe(&prior));

    let scuttlers = raw
        .my_units
        .iter()
        .filter(|unit| unit.kind == UnitKind::Scuttler)
        .map(|unit| unit.id)
        .collect::<Vec<_>>();
    assert_eq!(scuttlers.len(), 2);
    let act = brain.act_traced(&state);
    let trace = act.trace.expect("the coordinated decision is traced");
    assert!(
        trace
            .allocation
            .proposals
            .entries
            .iter()
            .all(
                |proposal| proposal.claims.minimum_residual_scrap >= UnitKind::Skyhook.stats().cost
            ),
        "allocation must preview the carrier before residual Raid admission: {:?}",
        trace.allocation.proposals
    );
    assert_eq!(
        (brain.mind().raids)
            .operation()
            .expect("Prime admits the fresh raid beside Recon")
            .members,
        scuttlers
    );
    assert_eq!(
        (brain.mind().strategy)
            .air_operation()
            .map(|operation| operation.phase()),
        Some(AirOperationPhase::Recon)
    );
    assert_eq!(
        trace
            .budget
            .expect("the residual carrier decision is traced")
            .prospective_carrier,
        0,
        "the newly reserved raiders leave no payload for a prospective lift"
    );
}

#[test]
fn coordinated_air_and_bulk_lift_complete_one_shared_objective_cycle() {
    let scenario = combined_lifecycle_scenario();
    let mut state = scenario
        .build()
        .expect("combined lifecycle scenario builds");
    for _ in 0..6_000 {
        state.tick(&[]);
    }

    let mut brain = operation_identity_brain(PlayerId(0), &scenario);
    brain.dials.minimum_core_equivalents = 0;

    let mut shared_target = None;
    let mut bomber_release = false;
    let mut bomber_release_tick = None;
    let mut lift_held_before_air_release = false;
    let mut first_target_unload_tick = None;
    let mut carrier_loads = Vec::new();
    let mut target_unloads = Vec::new();
    let mut loaded_riders = Vec::new();
    let mut landed_assault = Vec::new();
    let mut allocated_while_cargo_was_in_transit = false;

    for _ in 0..8_000 {
        let cargo_is_transitively_owned = loaded_riders
            .iter()
            .any(|rider| state.unit(*rider).is_none());
        let act = brain.act_traced(&state);
        if cargo_is_transitively_owned && let Some(trace) = act.trace.as_ref() {
            assert!(
                trace.allocation.error.is_none() && trace.allocation.coordinator_failure.is_none(),
                "loaded riders are owned through their carrier rather than imported as missing independent units: {:?}",
                trace.allocation
            );
            allocated_while_cargo_was_in_transit = true;
        }
        let commands = act.commands;
        if shared_target.is_none()
            && let (Some(air), Some(lift)) = (
                (brain.mind().strategy).air_operation(),
                (brain.mind().lifts).operation(),
            )
        {
            assert_eq!(
                (air.target_player, air.target),
                (lift.target_player, lift.target)
            );
            assert!(
                lift.desired_carriers >= 3,
                "the fixture must form a bulk lift"
            );
            shared_target = Some((air.target_id, air.target));
        }

        if let Some((target_id, target)) = shared_target {
            lift_held_before_air_release |=
                (brain.mind().lifts).operation().is_some_and(|operation| {
                    matches!(
                        operation.phase,
                        LiftPhase::Boarding | LiftPhase::AwaitSupport
                    ) && !operation.manifests.is_empty()
                }) && !bomber_release;
            for command in &commands {
                match &command.command {
                    Command::Attack {
                        units,
                        target: command_target,
                        ..
                    } if *command_target
                        == Target::Building(
                            target_id.expect("the shared Foundry is currently identified"),
                        )
                        .into() =>
                    {
                        let strike_aircraft = units
                            .iter()
                            .filter(|id| {
                                state.unit(**id).is_some_and(|unit| {
                                    unit.player == PlayerId(0) && unit.kind == UnitKind::Moth
                                })
                            })
                            .count();
                        let screen = units
                            .iter()
                            .filter(|id| {
                                state.unit(**id).is_some_and(|unit| {
                                    unit.player == PlayerId(0) && unit.kind == UnitKind::Darter
                                })
                            })
                            .count();
                        if strike_aircraft >= 4 && screen >= 2 {
                            bomber_release = true;
                            bomber_release_tick.get_or_insert(state.current_tick());
                        }
                    }
                    Command::Load {
                        units, transport, ..
                    } if state.unit(*transport).is_some_and(|unit| {
                        unit.player == PlayerId(0) && unit.kind == UnitKind::Skyhook
                    }) =>
                    {
                        carrier_loads.push(*transport);
                        loaded_riders.extend(units.iter().copied());
                    }
                    Command::Unload { transport, at, .. } if at.chebyshev(target) <= 6 => {
                        first_target_unload_tick.get_or_insert(state.current_tick());
                        target_unloads.push(*transport);
                    }
                    Command::AttackMove { units, goal, .. } if goal.chebyshev(target) <= 6 => {
                        landed_assault.extend(
                            units
                                .iter()
                                .copied()
                                .filter(|unit| loaded_riders.contains(unit)),
                        );
                    }
                    _ => {}
                }
            }
        }

        let report = state.tick(&commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "the coordinated lifecycle emitted an illegal command: {:?}",
            report.events
        );

        carrier_loads.sort_unstable();
        carrier_loads.dedup();
        target_unloads.sort_unstable();
        target_unloads.dedup();
        loaded_riders.sort_unstable();
        loaded_riders.dedup();
        landed_assault.sort_unstable();
        landed_assault.dedup();
        if bomber_release
            && target_unloads.len() >= 3
            && !loaded_riders.is_empty()
            && landed_assault == loaded_riders
        {
            break;
        }
    }

    assert!(
        shared_target.is_some(),
        "both operations choose one objective"
    );
    assert!(
        lift_held_before_air_release,
        "the bulk wave boards without launching before the air corridor releases"
    );
    assert!(
        bomber_release,
        "the mixed bomber wing releases on that objective"
    );
    assert!(
        first_target_unload_tick >= bomber_release_tick,
        "the shared lift cannot launch before the bomber operation releases it: first_target_unload_tick={first_target_unload_tick:?}, bomber_release_tick={bomber_release_tick:?}"
    );
    assert!(
        carrier_loads.len() >= 3,
        "at least three carriers receive manifests"
    );
    assert!(
        !loaded_riders.is_empty(),
        "the manifests contain a real ground wave"
    );
    assert!(
        allocated_while_cargo_was_in_transit,
        "at least one allocation boundary observes riders transitively owned as carrier cargo"
    );
    assert_eq!(
        target_unloads, carrier_loads,
        "the bulk wave launches together"
    );
    assert_eq!(
        landed_assault, loaded_riders,
        "every manifested rider is handed to the shared-objective assault"
    );
}

#[test]
fn a_wealthy_island_brain_launches_grouped_bombers_without_a_lift_payload() {
    let scenario = independent_bomber_operation_scenario();
    let mut state = scenario
        .build()
        .expect("independent bomber scenario builds");
    for _ in 0..6_000 {
        state.tick(&[]);
    }

    assert!(
        state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(0))
            .all(|unit| unit.kind.stats().transport_size == 0)
    );
    let mut brain = operation_identity_brain(PlayerId(0), &scenario);
    // This fixture isolates the independent bomber lifecycle. SeatBot-level
    // opening-core admission is covered by dedicated mixed-roster tests.
    brain.dials.minimum_core_equivalents = 0;

    let mut launched = None;
    for _ in 0..4_000 {
        let commands = brain.act(&state);
        for command in &commands {
            let units = match &command.command {
                Command::Attack { units, .. } | Command::AttackMove { units, .. } => units,
                _ => continue,
            };
            let bomber_count = units
                .iter()
                .filter(|id| {
                    state.units().iter().any(|unit| {
                        unit.id == **id
                            && unit.player == PlayerId(0)
                            && unit.kind == UnitKind::Condor
                    })
                })
                .count();
            let screen_count = units
                .iter()
                .filter(|id| {
                    state.units().iter().any(|unit| {
                        unit.id == **id
                            && unit.player == PlayerId(0)
                            && unit.kind == UnitKind::Buzzard
                    })
                })
                .count();
            if bomber_count >= 4 {
                launched = Some((
                    state.current_tick(),
                    bomber_count,
                    screen_count,
                    units.len(),
                ));
                break;
            }
        }

        assert!(
            (brain.mind().lifts).operation().is_none(),
            "an air-only roster cannot make the bomber operation depend on a lift"
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
            "the independent bomber operation emitted an illegal command: {:?}",
            report.events
        );
        if launched.is_some() {
            break;
        }
    }

    let (tick, strike_aircraft, screen, wing) =
        launched.expect("the independent bomber wing launches");
    assert!(tick < 10_000);
    assert_eq!(strike_aircraft, 6);
    assert_eq!(screen, 3);
    assert_eq!(
        wing,
        strike_aircraft + screen,
        "the frozen roster launches together"
    );
}

#[test]
fn an_active_raid_keeps_its_members_when_a_new_bulk_lift_forms() {
    let scenario = combined_operation_scenario();
    let mut state = scenario
        .build()
        .expect("combined-operation scenario builds");
    for _ in 0..6_000 {
        state.tick(&[]);
    }

    let mut brain = operation_identity_brain(PlayerId(0), &scenario);
    enlist_opening_core(&mut brain, &state);
    let profile = *brain.profile();
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let home = obs
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Foundry)
        .expect("the home Foundry stands")
        .anchor;
    let mut prior_raid = RaidPlanner::new();
    prior_raid.think_unrestricted(
        &profile,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &obs,
        home,
        &[],
        &[],
    );
    let prior_members = prior_raid
        .operation()
        .expect("the reachable Extractor starts a raid")
        .members
        .clone();
    assert_eq!(prior_members.len(), 2);
    brain.mind_mut().raids = prior_raid;

    brain.act(&state);

    let lift = (brain.mind().lifts)
        .operation()
        .expect("the independent bulk lift also forms");
    assert!(lift.desired_carriers >= 8);
    assert!(
        prior_members
            .iter()
            .all(|member| !lift.payload.contains(member)),
        "a new lift cannot steal members from a raid already under way"
    );
}

#[test]
fn contested_operation_members_retain_recovery_without_dispatch() {
    let mut scenario = opening_core_team_relief_scenario();
    scenario.name = "conflicting retained operation ownership".into();
    for row in scenario.map.iter_mut().skip(1).take(22) {
        let mut bytes = row.as_bytes().to_vec();
        bytes[20] = b'~';
        *row = String::from_utf8(bytes).expect("the fixture map remains ASCII");
    }
    let mut marker_row = scenario.map[10].as_bytes().to_vec();
    marker_row[24] = b'.';
    marker_row[15] = b'2';
    scenario.map[10] = String::from_utf8(marker_row).expect("the fixture map remains ASCII");
    let pressure = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 2 && unit.kind == UnitKind::Sentinel)
        .expect("the team fixture has one pressure unit");
    (pressure.x, pressure.y) = (17, 10);
    scenario
        .buildings
        .push(building_spec(0, BuildingKind::Airworks, 6, 3));
    scenario
        .units
        .extend((0..4).map(|index| unit_spec(0, UnitKind::Skyhook, 4 + index, 18)));
    scenario.units.push(unit_spec(0, UnitKind::Kestrel, 32, 10));
    let mut state = scenario
        .build()
        .expect("the conflicting-operation scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);

    let mut brain = operation_identity_brain(PlayerId(0), &scenario);
    brain.dials.minimum_core_equivalents = 0;

    let mut profile = *brain.profile();
    profile.traits.support = 70;
    profile.traits.fortification = 65;
    brain.mind_mut().profile = profile;
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let home = obs
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Foundry)
        .expect("the home Foundry stands")
        .anchor;

    let mut team = TeamReliefPlanner::new();
    let watched = team.think_unrestricted(&profile, tuning, &obs, home, &[], &[]);
    assert!(
        !watched.reservations.is_empty(),
        "the first current-pressure observation must freeze a credible relief watch: allies={:?}, enemies={:?}",
        obs.ally_buildings,
        obs.enemy_units
    );
    let mut admitted_obs = obs.clone();
    admitted_obs.tick = crate::difficulty::strategic_admission_at_or_after(
        admitted_obs
            .tick
            .saturating_add(u64::from(oxide_sim::TICKS_PER_SECOND))
            .saturating_add(tuning.reaction_delay),
    );
    crate::test_support::set_tick(&mut state, admitted_obs.tick);
    let _ = team.think_unrestricted(&profile, tuning, &admitted_obs, home, &[], &[]);
    let team_members = team
        .operation()
        .expect("sustained allied pressure admits a relief")
        .members
        .clone();
    let mut lift = LiftPlanner::new();
    let _ = lift.think_with_admission_and_producer_lanes(
        &admitted_obs,
        home,
        &[],
        LiftAirSupport::Independent,
        LiftAdmission {
            allow_new_commitments: true,
            spendable_scrap: admitted_obs.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 0,
        },
        crate::resources::ProducerLaneReservations::empty(),
    );
    let lift_payload = lift
        .operation()
        .expect("the disconnected enemy Foundry admits a lift")
        .payload
        .to_vec();
    assert!(
        team_members
            .iter()
            .any(|member| lift_payload.binary_search(member).is_ok()),
        "independently seeded operations must intentionally claim at least one common unit"
    );

    let ally_foundry = admitted_obs
        .ally_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Foundry)
        .expect("the relief target stands before the conflicting turn")
        .id;
    let mut failed_turn_obs = Observation::fog_honest(&state, PlayerId(0));
    failed_turn_obs
        .ally_buildings
        .retain(|building| building.id != ally_foundry);
    let mut independently_advanced_team = team.clone();
    let _ = independently_advanced_team.think_with_admission(
        &profile,
        tuning,
        &failed_turn_obs,
        home,
        &[],
        TeamReliefAdmission {
            additionally_reserved: &[],
            allow_new_operation: false,
            core_reservations: &[],
            minimum_core_equivalents: 0,
        },
    );
    assert_ne!(
        independently_advanced_team, team,
        "the failed turn must exercise a real accepted lifecycle transition"
    );

    let team_before = team.clone();
    let lift_before = lift.clone();
    brain.mind_mut().team = team;
    brain.mind_mut().lifts = lift;

    let result = crate::Brain::act_traced(&mut brain, &failed_turn_obs);
    let trace = result
        .trace
        .expect("the conflicting allocation boundary is traced");
    assert!(matches!(
        trace.allocation.error,
        Some(crate::trace::AllocationErrorTrace::ObligationConflict { .. })
    ));
    assert!(
        trace
            .budget
            .is_some_and(|budget| { budget.frozen && budget.utility_spendable == 0 }),
        "a malformed shared session must not reopen the current bank to residual utility"
    );
    let maintained_team = &brain.mind().team;
    assert_eq!(
        maintained_team.outcomes,
        independently_advanced_team.outcomes
    );
    let relief = maintained_team.operation().unwrap();
    assert_eq!(relief.phase, crate::team::TeamReliefPhase::Withdrawing);
    assert_eq!(
        relief.exit_reason,
        Some(crate::team::TeamReliefExitReason::FoundryLost)
    );
    assert_eq!(
        relief.dispatch,
        team_before.operation().unwrap().dispatch,
        "contested members cannot record an unissued return order"
    );
    assert!(result.commands.iter().all(|command| !matches!(
        &command.command, Command::Move { units, .. } if units.iter().any(|id| team_members.contains(id))
    )));
    let mut restored_lift = brain.mind().lifts.clone();
    assert!(restored_lift.outcomes.pending.is_empty());
    restored_lift.outcomes = lift_before.outcomes.clone();
    assert_eq!(restored_lift, lift_before);
    assert!(result.commands.iter().all(|command| !matches!(
        command.command,
        Command::Build { .. } | Command::Train { .. }
    )));
}

#[test]
fn partial_guile_muster_is_reserved_before_the_generic_army_draft() {
    let mut obs = test_island_observation();
    obs.known_rock.clear();
    obs.enemy_units.push(UnitObs {
        player: PlayerId(1),
        ..test_unit(600, UnitKind::Harvester, TilePos::new(20, 15))
    });
    obs.my_units
        .push(test_unit(1, UnitKind::Scuttler, TEST_HOME));
    obs.my_units.extend(
        (10..=13).map(|id| test_unit(id, UnitKind::Sentinel, TEST_HOME.offset(id as i32 - 9, 0))),
    );
    obs.my_units.sort_unstable_by_key(|unit| unit.id);

    let briefing_scenario = Scenario::skirmish();
    let mut brain = operation_identity_brain(PlayerId(0), &briefing_scenario);
    let profile = *brain.profile();
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let raids = &mut brain.mind_mut().raids;
    let partial = raids.think_unrestricted(&profile, tuning, &obs, TEST_HOME, &[], &[]);
    assert_eq!(partial.reservations, [UnitId(1)]);
    assert!(partial.intents.is_empty());

    let prior_claims = prior_planner_claims(&[], [], &[], raids.reservations(), None);
    brain.exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy {
            staging: TEST_HOME,
            size: 5,
        }],
        &prior_claims,
    );
    assert!(
        brain
            .exec
            .armies()
            .iter()
            .flat_map(|army| &army.members)
            .all(|member| *member != UnitId(1)),
        "the partial exact muster must survive a generic draft"
    );

    obs.tick = crate::difficulty::next_strategic_admission_tick(obs.tick);
    obs.my_units
        .push(test_unit(2, UnitKind::Scuttler, TEST_HOME.offset(1, 0)));
    obs.my_units.sort_unstable_by_key(|unit| unit.id);
    let enlisted: Vec<_> = brain.exec.enlisted().collect();
    let complete = brain.mind_mut().raids.think_unrestricted(
        &profile,
        tuning,
        &obs,
        TEST_HOME,
        &enlisted,
        &[],
    );
    assert_eq!(complete.reservations, [UnitId(1), UnitId(2)]);
    assert!(matches!(
        complete.intents.as_slice(),
        [Intent::AttackMoveUnits { units, .. }]
            if units == &[UnitId(1), UnitId(2)]
    ));
}

#[test]
fn provisioning_lift_payload_does_not_ground_unreserved_defenders() {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.units.push(unit_spec(1, UnitKind::Sentinel, 5, 6));
    let mut state = scenario.build().expect("double-booking scenario builds");
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 17);
    let mut brain = scripted_brain(&scenario, PlayerId(0), config);
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let staging = TilePos::new(8, 15);

    let muster = brain.exec.apply_with_reservations(
        PlayerId(0),
        &obs,
        &[Intent::FormArmy { staging, size: 40 }],
        &[],
    );
    assert!(muster.iter().any(|command| matches!(
        &command.command,
        Command::AttackMove { units, goal, queue: false }
            if units.len() == 40 && *goal == staging
    )));
    let army = brain.exec.armies()[0].clone();
    assert_eq!(army.state, ArmyState::Staging);
    assert_eq!(army.target, None);
    let enlisted: Vec<_> = brain.exec.enlisted().collect();
    let mut policy_probe = brain.policy.clone();
    let unreserved = policy_probe.think_residual(
        brain.dials(),
        &obs,
        std::slice::from_ref(&army),
        &enlisted,
        &[],
        &brain.mind().public_map,
    );
    assert!(
        unreserved.iter().any(|intent| matches!(
            intent,
            Intent::PushArmy {
                army: candidate,
                target: TilePos { x: 5, y: 6 },
            } if *candidate == army.id
        )),
        "the fixture must offer the exact ground push that lift reservations suppress: {unreserved:?}"
    );

    for think in 0..2 {
        while !state.current_tick().is_multiple_of(brain.dials().cadence) {
            state.tick(&[]);
        }
        let result = brain.act_traced(&state);
        let commands = result.commands;
        let operation = (brain
            .mind()
            .lifts).operation()
            .unwrap_or_else(|| {
                panic!(
                    "the severed enemy Foundry freezes a lift payload on think {think}: commands={commands:?}; trace={:?}",
                    result.trace
                )
            });
        assert_eq!(operation.phase, LiftPhase::Provision);
        assert!(!operation.payload.is_empty());
        assert!(
            operation
                .payload
                .iter()
                .all(|unit| army.members.contains(unit))
        );
        assert!(
            commands.iter().all(|command| !matches!(
                &command.command,
                Command::AttackMove { units, .. }
                    if units.iter().any(|unit| operation.payload.contains(unit))
            )),
            "think {think} double-booked the frozen lift payload: {commands:?}"
        );
        let mut strategic_claims = operation.payload.to_vec();
        if let Some(air) = (brain.mind().strategy).air_operation() {
            strategic_claims.extend(air.scout);
            strategic_claims.extend(air.artillery.iter().copied());
            strategic_claims.extend(air.strike_aircraft.iter().copied());
        }
        if let Some(relief) = (brain.mind().team).operation() {
            strategic_claims.extend(relief.members.iter().copied());
        }
        if let Some(raid) = (brain.mind().raids).operation() {
            strategic_claims.extend(raid.members.iter().copied());
        }
        strategic_claims.sort_unstable();
        strategic_claims.dedup();
        let available: Vec<_> = army
            .members
            .iter()
            .copied()
            .filter(|unit| strategic_claims.binary_search(unit).is_err())
            .collect();
        let staged = brain
            .exec
            .armies()
            .iter()
            .find(|candidate| candidate.id == army.id)
            .expect("the defending remainder remains tracked");
        let mission = staged
            .mission
            .as_ref()
            .expect("the unreserved body receives a defense responsibility");
        assert!(matches!(
            mission.purpose,
            crate::executive::ArmyPurpose::Defend(_)
        ));
        assert!(
            mission.goal.chebyshev(TilePos::new(5, 6)) <= 8,
            "the defensive service point must answer the visible incursion"
        );
        if think == 0 {
            assert!(
                commands.iter().any(|command| matches!(
                    &command.command,
                    Command::AttackMove {
                        units,
                        goal,
                        queue: false,
                    } if units == &available && *goal == mission.goal
                )),
                "unreserved members must remain available for the visible emergency: {commands:?}"
            );
        }
        assert_eq!(staged.members, available, "think {think}");
        assert_eq!(staged.state, ArmyState::Pushing);
        assert_eq!(staged.target, Some(mission.goal));

        state.tick(&[]);
    }
}

pub(super) fn brain_with_remembered_lift_target(
    scenario: &Scenario,
    state: &State,
    last_seen: u64,
) -> SeatBot {
    let raw = Observation::fog_honest(state, PlayerId(0));
    let home = home_foundry(&raw);
    let orientation = Orientation::for_home(&raw, home);
    let mut brain = scripted_brain(
        scenario,
        PlayerId(0),
        BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_024),
    );
    brain.orientation = Some(orientation);
    let enemy_foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .expect("the enemy Foundry stands beyond the barrier");
    let prior = prior_foundry_sighting(&raw, enemy_foundry, last_seen);
    brain
        .mind_mut()
        .intelligence
        .update(&orientation.observe(&prior));
    brain
}

pub(super) fn combined_lifecycle_scenario() -> Scenario {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.name = "brain combined air and lift lifecycle".into();
    scenario.players[0].faction = Faction::Cupric;
    scenario.players[0].scrap = 50_000;
    scenario.units.clear();
    scenario.units.extend(
        (0..16).map(|index| unit_spec(0, UnitKind::Scuttler, 3 + index % 8, 15 + index / 8)),
    );
    scenario
        .units
        .extend((0..4).map(|index| unit_spec(0, UnitKind::Skyhook, 3 + index, 18)));
    scenario
        .units
        .extend((0..6).map(|index| unit_spec(0, UnitKind::Moth, 8 + index, 18)));
    scenario
        .units
        .extend((0..3).map(|index| unit_spec(0, UnitKind::Darter, 14 + index, 18)));
    scenario.units.push(unit_spec(0, UnitKind::Gnat, 31, 11));
    scenario
}
