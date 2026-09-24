use super::*;

#[test]
fn standard_and_veteran_reach_their_opening_core_before_the_first_fabricator() {
    const OPENING_END: u64 = 5_000;

    let scenario = calibration_open_ferrous();
    let mut state = scenario.build().expect("the calibration opening builds");
    let personality_seed = 1_616_201;
    let public_map = public_map(&scenario);
    let mut brains = [
        SeatBot::scripted(
            PlayerId(0),
            BotConfig::scripted(
                BotDifficulty::Standard,
                BotStance::Balanced,
                personality_seed,
            ),
            Arc::clone(&public_map),
        ),
        SeatBot::scripted(
            PlayerId(1),
            BotConfig::scripted(
                BotDifficulty::Veteran,
                BotStance::Balanced,
                personality_seed,
            ),
            public_map,
        ),
    ];
    let floors = [5_u64, 6_u64];
    let mut first_fabricator = [None; 2];
    let mut core_at_fabricator = [None; 2];

    while state.current_tick() < OPENING_END && first_fabricator.iter().any(Option::is_none) {
        let tick = state.current_tick();
        let statuses = [0, 1].map(|seat| {
            combat_core_status(
                &Observation::fog_honest(&state, PlayerId(seat as u8)),
                &[],
                &[],
                floors[seat],
            )
        });
        let commands: Vec<_> = brains
            .iter_mut()
            .flat_map(|brain| brain.act(&state))
            .collect();
        for command in &commands {
            let seat = usize::from(command.player.0);
            if matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Fabricator,
                    ..
                }
            ) && first_fabricator[seat].is_none()
            {
                first_fabricator[seat] = Some(tick);
                core_at_fabricator[seat] = Some(statuses[seat].projected_strength);
                assert!(
                    statuses[seat].ready,
                    "seat {seat} started its first Fabricator with core status {:?}",
                    statuses[seat]
                );
                assert!(
                    statuses[seat].projected_strength >= statuses[seat].target_strength,
                    "post-floor reinforcement may extend the line while capital accumulates"
                );
            }
        }

        let report = state.tick(&commands);
        for event in report.events {
            if let oxide_sim::Event::CommandRejected { player, reason } = event {
                let placements: Vec<_> = commands
                    .iter()
                    .filter_map(|command| match &command.command {
                        Command::Build {
                            kind,
                            anchor,
                            units,
                            ..
                        } => Some((
                            *kind,
                            *anchor,
                            state.place_intent_refusal_replacing(
                                command.player,
                                *kind,
                                *anchor,
                                units,
                            ),
                        )),
                        _ => None,
                    })
                    .collect();
                panic!(
                    "seat {player} issued a rejected opening command: {reason:?}; tick={}; commands={commands:?}; placements={placements:?}",
                    state.current_tick()
                );
            }
        }
    }

    assert!(
        first_fabricator.iter().all(Option::is_some),
        "every rung should start a Fabricator within the opening window: first={first_fabricator:?}, scrap={:?}, buildings={:?}, queues={:?}",
        [0, 1].map(|seat| state.player(PlayerId(seat)).scrap),
        state
            .buildings()
            .iter()
            .map(|building| (building.player, building.kind, building.built))
            .collect::<Vec<_>>(),
        state
            .buildings()
            .iter()
            .map(|building| (building.id, building.queue.clone()))
            .collect::<Vec<_>>(),
    );
    assert!(core_at_fabricator.iter().all(Option::is_some));
}

#[test]
fn landed_lift_assault_cannot_reopen_prime_capital_spending() {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.players[0].scrap = 50_000;
    let mut kept_sentinels = 0;
    scenario.units.retain(|unit| {
        if unit.kind != UnitKind::Sentinel {
            return true;
        }
        kept_sentinels += 1;
        kept_sentinels <= 16
    });
    scenario
        .units
        .extend((0..8).map(|index| unit_spec(0, UnitKind::Skyhook, 3 + index, 20)));
    let state = scenario
        .build()
        .expect("the landed-assault opening fixture builds");
    let mut planning = Observation::fog_honest(&state, PlayerId(0));
    let home = planning
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Foundry)
        .expect("the authored home Foundry stands")
        .anchor;
    let original_riders = planning.my_units.clone();
    let mut lifts = LiftPlanner::new();
    let _ = lifts.think_with_admission_and_producer_lanes(
        &planning,
        home,
        &[],
        LiftAirSupport::Independent,
        LiftAdmission {
            allow_new_commitments: true,
            spendable_scrap: planning.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 8,
        },
        crate::resources::ProducerLaneReservations::empty(),
    );
    let manifests = lifts
        .operation()
        .expect("the disconnected objective admits a lift")
        .manifests
        .clone();
    let riders: Vec<_> = manifests
        .iter()
        .flat_map(|manifest| manifest.riders.iter().copied())
        .collect();
    assert_eq!(riders.len(), 8, "the lift leaves Prime's home eight intact");

    planning.my_units.retain(|unit| !riders.contains(&unit.id));
    for manifest in &manifests {
        let carrier = planning
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .expect("every exact carrier remains observable");
        carrier.tile = manifest.pickup;
        carrier.cargo = manifest
            .riders
            .iter()
            .filter_map(|id| original_riders.iter().find(|unit| unit.id == *id))
            .map(|unit| unit.kind.stats().transport_size)
            .sum();
    }
    planning.tick += 1;
    let _ = lifts.think_with_admission_and_producer_lanes(
        &planning,
        home,
        &[],
        LiftAirSupport::Independent,
        LiftAdmission {
            allow_new_commitments: false,
            spendable_scrap: planning.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 8,
        },
        crate::resources::ProducerLaneReservations::empty(),
    );
    assert_eq!(
        lifts.operation().map(|operation| operation.phase),
        Some(LiftPhase::Landing)
    );

    let first_manifest = manifests
        .first()
        .expect("the multi-carrier fixture assigns a first manifest");
    {
        let carrier = planning
            .my_units
            .iter_mut()
            .find(|unit| unit.id == first_manifest.carrier)
            .expect("the first carrier reaches its exact drop");
        carrier.tile = first_manifest.drop;
        carrier.cargo = 0;
        planning
            .my_units
            .extend(first_manifest.riders.iter().map(|id| {
                let mut rider = original_riders
                    .iter()
                    .find(|unit| unit.id == *id)
                    .expect("the manifest names an exact original rider")
                    .clone();
                rider.tile = first_manifest.drop;
                rider
            }));
    }
    planning.my_units.sort_unstable_by_key(|unit| unit.id);
    planning.tick += 1;
    let _ = lifts.think_with_admission_and_producer_lanes(
        &planning,
        home,
        &[],
        LiftAirSupport::Independent,
        LiftAdmission {
            allow_new_commitments: false,
            spendable_scrap: planning.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 8,
        },
        crate::resources::ProducerLaneReservations::empty(),
    );
    let mixed_operation = lifts
        .operation()
        .expect("the remaining loaded carrier keeps the lift landing");
    assert_eq!(mixed_operation.phase, LiftPhase::Landing);
    assert!(mixed_operation.manifests[0].attack_issued);
    assert!(
        mixed_operation
            .manifests
            .iter()
            .skip(1)
            .any(|manifest| !manifest.attack_issued)
    );

    planning.tick += 1;
    let mixed_decision = lifts.think_with_admission_and_producer_lanes(
        &planning,
        home,
        &[],
        LiftAirSupport::Independent,
        LiftAdmission {
            allow_new_commitments: false,
            spendable_scrap: planning.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 8,
        },
        crate::resources::ProducerLaneReservations::empty(),
    );
    assert!(
        first_manifest
            .riders
            .iter()
            .all(|id| mixed_decision.reservations.contains(id)),
        "landed riders remain reserved while another manifest is still landing: {mixed_decision:?}"
    );

    for manifest in manifests.iter().skip(1) {
        let carrier = planning
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .expect("every remaining carrier reaches its exact drop");
        carrier.tile = manifest.drop;
        carrier.cargo = 0;
        planning.my_units.extend(manifest.riders.iter().map(|id| {
            let mut rider = original_riders
                .iter()
                .find(|unit| unit.id == *id)
                .expect("the manifest names an exact original rider")
                .clone();
            rider.tile = manifest.drop;
            rider
        }));
    }
    planning.my_units.sort_unstable_by_key(|unit| unit.id);
    planning.tick += 1;
    let _ = lifts.think_with_admission_and_producer_lanes(
        &planning,
        home,
        &[],
        LiftAirSupport::Independent,
        LiftAdmission {
            allow_new_commitments: false,
            spendable_scrap: planning.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 8,
        },
        crate::resources::ProducerLaneReservations::empty(),
    );
    let operation = lifts
        .operation()
        .expect("the planner keeps directing its landed assault");
    assert_eq!(operation.phase, LiftPhase::Recover);
    assert!(
        operation
            .manifests
            .iter()
            .all(|manifest| manifest.attack_issued)
    );

    let one_home_sentinel = state
        .units()
        .iter()
        .find(|unit| {
            unit.player == PlayerId(0)
                && unit.kind == UnitKind::Sentinel
                && !riders.contains(&unit.id)
        })
        .expect("the original core has one removable home member")
        .id;
    let assault_positions: Vec<_> = operation
        .manifests
        .iter()
        .flat_map(|manifest| {
            manifest
                .riders
                .iter()
                .copied()
                .map(move |id| (id, manifest.drop))
        })
        .collect();
    let mut observed = Observation::fog_honest(&state, PlayerId(0));
    observed
        .my_units
        .retain(|unit| unit.id != one_home_sentinel);
    for unit in &mut observed.my_units {
        if let Some((_, drop)) = assault_positions
            .iter()
            .find(|(rider, _)| *rider == unit.id)
        {
            unit.tile = *drop;
        }
    }
    assert!(combat_core_status(&observed, &[], &[], 8).ready);
    let exact_lift_claims = prior_planner_claims(&[], [], &[], &[], Some(operation));
    let protected = combat_core_status(&observed, &exact_lift_claims, &[], 8);
    assert!(
        !protected.ready,
        "the seven home hulls, not the eight island riders, define the opening core: {protected:?}"
    );

    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_024);
    let mut control = scripted_brain(&scenario, PlayerId(0), config);

    let control_commands = crate::Brain::act(&mut control, &observed);
    assert!(
        control_commands.iter().any(|command| matches!(
            command.command,
            Command::Build { .. } | Command::UpgradeBuilding { .. }
        )),
        "the rich control must prove voluntary capital is otherwise available: {control_commands:?}"
    );

    let mut protected_brain = scripted_brain(&scenario, PlayerId(0), config);

    protected_brain.mind_mut().lifts = lifts;

    let commands = crate::Brain::act(&mut protected_brain, &observed);
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Sentinel,
            ..
        }
    )));
    assert!(
        commands.iter().all(|command| match command.command {
            Command::Build { .. } | Command::UpgradeBuilding { .. } => false,
            Command::Train { kind, .. } => kind == UnitKind::Sentinel,
            _ => true,
        }),
        "operation-owned island riders cannot reopen voluntary spending: {commands:?}"
    );
}

#[test]
fn every_difficulty_preserves_first_carrier_capital_during_island_recon() {
    let mut scenario = prospective_lift_reservation_scenario();
    scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
    let mut state = scenario
        .build()
        .expect("prospective-lift reservation scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);

    let raw = Observation::fog_honest(&state, PlayerId(0));
    assert_eq!(
        raw.scrap,
        UnitKind::Skyhook.stats().cost,
        "the only spendable capital must be the first carrier's exact cost"
    );
    assert!(
        raw.enemy_buildings.is_empty(),
        "the objective must be out of current sight"
    );
    assert!(
        (0..raw.map_height).all(|y| raw.known_rock_at(TilePos::new(20, y))),
        "the bot must honestly know the complete ground barrier"
    );
    let enemy_foundry = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .expect("the enemy Foundry stands beyond the barrier");

    let mut spending_by_difficulty = Vec::new();
    for difficulty in BotDifficulty::ALL {
        let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        assert!(
            state.current_tick().is_multiple_of(brain.dials().cadence),
            "the shared snapshot must be a think tick for {difficulty:?}"
        );

        let home = home_foundry(&raw);
        let orientation = Orientation::for_home(&raw, home);
        brain.orientation = Some(orientation);
        let prior = prior_foundry_sighting(&raw, enemy_foundry, raw.tick.saturating_sub(100));
        let prior = orientation.observe(&prior);
        brain.mind_mut().intelligence.update(&prior);

        let oriented = orientation.observe(&raw);
        let oriented_home = orientation.anchor(home, BuildingKind::Foundry.base_stats().size);
        let mut expected_intelligence = brain.mind().intelligence.clone();
        expected_intelligence.update(&oriented);
        let target = expected_intelligence
            .buildings()
            .first()
            .expect("the synthetic prior sighting creates one contact");
        assert_eq!(
            brain.mind().lifts.prospective_first_carrier_commitment(
                &oriented,
                oriented_home,
                &[],
                &[],
                0,
                target,
            ),
            UnitKind::Skyhook.stats().cost,
            "the fog-honest snapshot warrants exactly one prospective carrier for {difficulty:?}"
        );

        let resources = crate::resources::ResourceSnapshot::from_observation(&oriented);
        let public_map = orientation.briefing(&brain.mind().public_map);
        let builders = oriented.my_units.iter().collect::<Vec<_>>();
        let unguarded_defense = brain.policy.fresh_defense_proposals(
            &brain.mind().profile,
            &oriented,
            &resources,
            &public_map,
            orientation,
            oriented_home,
            expected_intelligence.units(),
            expected_intelligence.buildings(),
            &builders,
            &[],
            0,
            0,
            0,
            crate::utility::DecisionEvidence {
                battlefield: brain.mind().battlefield.assessment(),
                experience: &brain.mind().experience,
            },
        );
        assert!(
            !unguarded_defense.is_empty(),
            "the fixture offers defense without the carrier hold for {difficulty:?}"
        );

        let act = brain.act_traced(&state);
        let first_trace = act
            .trace
            .as_ref()
            .expect("the prospective hold is traced on its admission think");
        let first_budget = first_trace
            .budget
            .as_ref()
            .expect("the prospective hold is traced on its admission think");
        assert_eq!(
            first_budget.prospective_carrier,
            UnitKind::Skyhook.stats().cost,
            "the first-carrier hold is owned exactly once for {difficulty:?}"
        );
        assert_eq!(
            first_budget.voluntary_scrap_guard,
            UnitKind::Sentinel.stats().cost,
            "the reachable ground objective keeps the overlapping shallow guard visible for {difficulty:?}"
        );
        assert_eq!(
            first_budget.utility_spendable, 0,
            "the exact carrier bank is protected once rather than splitting into additive carrier and shallow holds for {difficulty:?}"
        );
        assert!(
            first_trace
                .allocation
                .proposals
                .entries
                .iter()
                .all(|proposal| !matches!(
                    proposal.key,
                    crate::trace::ProposalKeyTrace::Defense { .. }
                )),
            "unaffordable defense is rejected before expensive quotation for {difficulty:?}"
        );
        assert!(
            first_trace
                .allocation
                .proposals
                .entries
                .iter()
                .all(|proposal| proposal.claims.minimum_residual_scrap
                    >= UnitKind::Skyhook.stats().cost),
            "every fresh voluntary proposal must preserve the prospective carrier floor for {difficulty:?}"
        );
        assert!(
            first_trace
                .allocation
                .proposals
                .entries
                .iter()
                .filter(|proposal| matches!(
                    proposal.key,
                    crate::trace::ProposalKeyTrace::Defense { .. }
                ))
                .all(|proposal| proposal.disposition
                    != crate::trace::ProposalDispositionTrace::Accepted),
            "no voluntary defense may spend the prospective carrier floor for {difficulty:?}"
        );
        let commands = act.commands;
        let operation = (brain.mind().strategy)
            .air_operation()
            .expect("remembered disconnected Foundry starts reconnaissance");
        assert_eq!(
            operation.phase(),
            AirOperationPhase::Recon,
            "{difficulty:?}"
        );
        assert!(!operation.assault_admitted(), "{difficulty:?}");
        assert!(
            (brain.mind().lifts).operation().is_none(),
            "prospective capital must not start or freeze a lift for {difficulty:?}"
        );
        let spending: Vec<_> = commands
            .iter()
            .filter_map(|command| match &command.command {
                command @ (Command::Build { .. }
                | Command::Train { .. }
                | Command::UpgradeBuilding { .. }) => Some(command.clone()),
                _ => None,
            })
            .collect();
        assert!(
            spending.is_empty(),
            "{difficulty:?} spent the first-carrier fund before current sight: {commands:?}"
        );

        let mut continued_state = state.clone();
        continued_state.tick(&commands);
        for _ in 1..brain.dials.cadence {
            continued_state.tick(&[]);
        }
        let continued = brain.act_traced(&continued_state);
        let continued_budget = continued
            .trace
            .as_ref()
            .and_then(|trace| trace.budget.as_ref())
            .expect("the active remembered Recon keeps a traced carrier hold");
        assert_eq!(
            continued_budget.prospective_carrier,
            UnitKind::Skyhook.stats().cost,
            "active remembered Recon keeps the exact first-carrier hold for {difficulty:?}"
        );
        assert!(
            continued.commands.iter().all(|command| !matches!(
                &command.command,
                Command::Build { .. } | Command::Train { .. } | Command::UpgradeBuilding { .. }
            )),
            "{difficulty:?} spent the active Recon carrier hold: {:?}",
            continued.commands
        );
        spending_by_difficulty.push(spending);
    }

    assert!(
        spending_by_difficulty
            .windows(2)
            .all(|pair| pair[0] == pair[1]),
        "higher difficulties must preserve the lower rung's mandatory transport prefix"
    );
}

#[test]
fn fallen_prime_core_keeps_paid_work_and_an_active_operation_progressing() {
    let scenario = combined_operation_scenario();
    let mut state = scenario
        .build()
        .expect("the combined-operation continuation builds");
    for _ in 0..6_000 {
        state.tick(&[]);
    }

    let mut brain = operation_identity_brain(PlayerId(0), &scenario);

    let mut prepaid_operation_queue = None;
    for _ in 0..1_000 {
        let commands = brain.act(&state);
        let active = (brain.mind().strategy).air_operation();
        if let Some((building, kind)) = commands.iter().find_map(|command| match command.command {
            Command::Train {
                building,
                kind: UnitKind::Condor,
            } if active.is_some() => Some((building, UnitKind::Condor)),
            _ => None,
        }) {
            prepaid_operation_queue = Some((building, kind));
        }
        let report = state.tick(&commands);
        assert_commands_accepted(&report, PlayerId(0));
        if prepaid_operation_queue.is_some() && (brain.mind().strategy).air_operation().is_some() {
            break;
        }
    }
    let (producer, queued_kind) = prepaid_operation_queue
        .expect("the active air operation prepays a Condor in its Airworks queue");
    assert!(
        state
            .building(producer)
            .is_some_and(|building| building.queue.contains(&queued_kind)),
        "the operation-owned production order is paid and remains queued"
    );

    let builder = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .expect("the safe home economy retains a voluntary-capital builder")
        .id;
    let capital_anchor = TilePos::new(14, 7);
    let report = state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Build {
            units: vec![builder],
            kind: BuildingKind::Array,
            anchor: capital_anchor,
            queue: false,
            defer: false,
        },
    }]);
    assert_commands_accepted(&report, PlayerId(0));
    let capital_site = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Array
                && building.anchor == capital_anchor
        })
        .expect("the ordinary command pays for a safe voluntary Array site")
        .id;
    assert!(
        !state
            .building(capital_site)
            .expect("the paid site remains")
            .built,
        "the capital work must still be unfinished before the later loss"
    );

    let lost_core: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| {
            unit.player == PlayerId(0)
                && matches!(
                    unit.kind.role(),
                    Role::Sentinel | Role::Warden | Role::Breaker
                )
        })
        .skip(1)
        .map(|unit| unit.id.0)
        .collect();
    assert!(
        lost_core.len() >= 7,
        "the fixture begins with at least Prime's full core before its later loss"
    );
    let mut document = serde_json::to_value(&state).expect("the live continuation serializes");
    document["units"]
        .as_array_mut()
        .expect("units serialize as an array")
        .retain(|unit| {
            !lost_core.contains(&(unit["id"].as_u64().expect("unit ids are numeric") as u32))
        });
    for building in document["buildings"]
        .as_array_mut()
        .expect("buildings serialize as an array")
    {
        building["queue"]
            .as_array_mut()
            .expect("building queues serialize as arrays")
            .retain(|kind| !matches!(kind.as_str(), Some("sentinel" | "warden" | "breaker")));
    }
    state = serde_json::from_value(document).expect("the post-loss state remains valid");
    assert_eq!(
        state
            .units()
            .iter()
            .filter(|unit| {
                unit.player == PlayerId(0)
                    && matches!(
                        unit.kind.role(),
                        Role::Sentinel | Role::Warden | Role::Breaker
                    )
            })
            .count(),
        1,
        "the later casualty leaves Prime below its eight-equivalent floor"
    );
    assert!(
        state
            .building(producer)
            .is_some_and(|building| building.queue.contains(&queued_kind)),
        "the operation-owned queue survives the core's later losses"
    );
    let post_loss_observation = Observation::fog_honest(&state, PlayerId(0));
    let post_loss_orientation = *brain
        .orientation
        .as_ref()
        .expect("the real SeatBot latched its player-facing orientation before the loss");
    let post_loss_exclusions = prior_planner_claims(
        &[],
        (brain.mind().strategy).owned_units(),
        &[],
        (brain.mind().raids).reservations(),
        (brain.mind().lifts).operation(),
    );
    let post_loss_oriented = post_loss_orientation.observe(&post_loss_observation);
    let post_loss_core = combat_core_status(
        &post_loss_oriented,
        &post_loss_exclusions,
        &[],
        u64::from(brain.dials.minimum_core_equivalents),
    );
    assert!(
        !post_loss_core.ready,
        "the actual post-loss SeatBot input must remain below Prime's protected core: \
         {post_loss_core:?}, queues={:?}",
        post_loss_oriented.my_queues
    );
    assert!(
        (brain.mind().strategy).air_operation().is_some(),
        "the paid operation is active before core loss"
    );
    let site_hp_before_loss = state
        .building(capital_site)
        .expect("the paid capital site survived the loss")
        .hp;
    let queue_progress_before_loss = state
        .building(producer)
        .expect("the prepaid operation queue survived the loss")
        .progress;

    let retained_lift_jobs = brain
        .mind()
        .lifts
        .active_production_obligation()
        .map(|obligation| obligation.producer_jobs().to_vec())
        .unwrap_or_default();
    let mut recovery_started = false;
    let mut queued_condor_finished = false;
    let mut operation_continuation_observed = false;
    for _ in 0..1_500 {
        let recovery_observation = Observation::fog_honest(&state, PlayerId(0));
        let recovery_exclusions = prior_planner_claims(
            &[],
            (brain.mind().strategy).owned_units(),
            &[],
            (brain.mind().raids).reservations(),
            (brain.mind().lifts).operation(),
        );
        let core_deficient = !combat_core_status(
            &post_loss_orientation.observe(&recovery_observation),
            &recovery_exclusions,
            &[],
            u64::from(brain.dials.minimum_core_equivalents),
        )
        .ready;
        let operation_was_active = (brain.mind().strategy).air_operation().is_some();
        let commands = brain.act(&state);
        if core_deficient && operation_was_active {
            let strategy = &brain.mind().strategy;
            assert!(
                strategy.air_operation().is_some() || strategy.terminal_outcome().is_some(),
                "core loss must not silently discard an active operation"
            );
            operation_continuation_observed = true;
        }
        recovery_started |= core_deficient
            && commands.iter().any(|command| {
                matches!(
                    command.command,
                    Command::Train {
                        kind: UnitKind::Sentinel,
                        ..
                    }
                )
            });
        assert!(commands.iter().all(|command| !matches!(
            command.command,
            Command::Cancel { .. } | Command::CancelTrain { .. }
        )));
        if core_deficient {
            assert!(
                commands.iter().all(|command| match command.command {
                    Command::Build { kind, anchor, .. } => {
                        state.buildings().iter().any(|site| {
                            site.player == PlayerId(0) && site.kind == kind && site.anchor == anchor
                        })
                    }
                    Command::Train {
                        kind: UnitKind::Sentinel,
                        ..
                    } => true,
                    Command::Train { building, kind } => retained_lift_jobs.iter().any(|job| job
                        .producer()
                        == building
                        && job.kind() == kind
                        && job.timing().enqueued_at() == state.current_tick()),
                    Command::UpgradeBuilding { .. } => false,
                    _ => true,
                }),
                "a deficient core may execute an existing exact producer commitment but must not admit new specialty capital: {commands:?}"
            );
        }

        let report = state.tick(&commands);
        assert_commands_accepted(&report, PlayerId(0));
        queued_condor_finished |= report.events.iter().any(|event| {
            matches!(
                event,
                oxide_sim::event::Event::UnitTrained {
                    kind,
                    player: PlayerId(0),
                    ..
                } if *kind == queued_kind
            )
        });
        if recovery_started
            && queued_condor_finished
            && state
                .building(capital_site)
                .is_some_and(|site| site.hp > site_hp_before_loss)
            && operation_continuation_observed
        {
            break;
        }
    }

    assert!(recovery_started, "Prime resumes its ordinary Sentinel line");
    assert!(
        state
            .building(capital_site)
            .is_some_and(|site| site.hp > site_hp_before_loss),
        "the paid voluntary site remains and advances through core recovery"
    );
    assert!(
        state.building(producer).is_some_and(|building| {
            building.progress > queue_progress_before_loss || queued_condor_finished
        }),
        "the prepaid operation queue keeps making ordinary production progress"
    );
    assert!(
        queued_condor_finished,
        "the prepaid operation unit completes without a repurchase"
    );
    assert!(
        operation_continuation_observed,
        "the existing operation survives core loss until its ordinary terminal transition"
    );
}

#[test]
fn opening_core_gate_blocks_fresh_strategic_work_at_the_brain_boundary() {
    let scenario = independent_bomber_operation_scenario();
    let mut state = scenario
        .build()
        .expect("independent bomber scenario builds");
    for _ in 0..6_000 {
        state.tick(&[]);
    }

    let mut gated = operation_identity_brain(PlayerId(0), &scenario);
    let commands = gated.act(&state);
    assert!((gated.mind().strategy).air_operation().is_none());
    assert!((gated.mind().lifts).operation().is_none());
    assert!((gated.mind().raids).operation().is_none());
    assert!(commands.iter().all(|command| !matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Skyhook
                | UnitKind::Condor
                | UnitKind::Moth
                | UnitKind::Buzzard
                | UnitKind::Darter,
            ..
        }
    )));

    let mut admitted = operation_identity_brain(PlayerId(0), &scenario);
    admitted.dials.minimum_core_equivalents = 0;
    let mut control = state.clone();
    for _ in 0..4_000 {
        let commands = admitted.act(&control);
        if (admitted.mind().strategy).air_operation().is_some() {
            break;
        }
        control.tick(&commands);
    }
    assert!(
        (admitted.mind().strategy).air_operation().is_some(),
        "the same fog-honest snapshot must otherwise qualify for a fresh air operation"
    );
}

#[test]
fn opening_emergency_defense_and_core_recovery_commit_once_through_state() {
    let mut scenario = Scenario::skirmish();
    scenario.map = scenario
        .map
        .iter()
        .map(|row| row.replace('E', "."))
        .collect();
    scenario.name = "opening emergency defense transaction".into();
    scenario.players[0].scrap = BuildingKind::Turret
        .base_stats()
        .construction
        .expect("Turrets are constructible")
        .cost
        .saturating_add(UnitKind::Sentinel.stats().cost);
    scenario
        .units
        .retain(|unit| unit.player != 0 || unit.kind != UnitKind::Sentinel);
    let pressure = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
        .expect("Skirmish has one hostile Sentinel");
    (pressure.x, pressure.y) = (4, 12);

    let mut state = scenario
        .build()
        .expect("the opening emergency scenario builds");
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 4_104);
    let mut brain = scripted_brain(&scenario, PlayerId(0), config);
    brain.dials.minimum_core_equivalents = 1;
    brain.dials.harvester_target = 3;

    let first = brain.act_traced(&state);
    let trace = first.trace.expect("the emergency allocation is traced");
    let emergency = trace
        .allocation
        .obligations
        .entries
        .iter()
        .find_map(|obligation| match obligation.key {
            crate::trace::ObligationKeyTrace::EmergencyDefense {
                building: BuildingKind::Turret,
                anchor,
            } => Some((anchor, obligation.claims.builders.entries.as_slice())),
            _ => None,
        })
        .expect("the visible ground threat admits one exact emergency Turret");
    let [expected_builder] = emergency.1 else {
        panic!("the frozen emergency payload owns one exact builder: {trace:?}");
    };
    let build_index = first
        .commands
        .iter()
        .position(|command| {
            matches!(
                &command.command,
                Command::Build {
                    units,
                    kind: BuildingKind::Turret,
                    anchor,
                    queue: false,
                    defer: false,
                } if units.as_slice() == [*expected_builder] && *anchor == emergency.0
            )
        })
        .expect("the selected emergency payload lowers without reranking");
    let recovery_index = first
        .commands
        .iter()
        .position(|command| {
            matches!(
                command.command,
                Command::Train {
                    kind: UnitKind::Sentinel,
                    ..
                }
            )
        })
        .expect("capital left after the defense funds core recovery");
    assert!(
        build_index < recovery_index,
        "survival construction must precede ordinary core recovery: {:?}",
        first.commands
    );
    assert_eq!(
        first
            .commands
            .iter()
            .filter(|command| matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Turret,
                    ..
                }
            ))
            .count(),
        1
    );

    let report = state.tick(&first.commands);
    assert!(
        report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )),
        "State must accept both exactly funded commands: {:?}",
        report.events
    );
    assert_eq!(state.player(PlayerId(0)).scrap, 0);
    assert!(state.buildings().iter().any(|building| {
        building.player == PlayerId(0)
            && building.kind == BuildingKind::Turret
            && building.anchor == emergency.0
    }));
    assert!(state.buildings().iter().any(|building| {
        building.player == PlayerId(0)
            && building.kind == BuildingKind::Foundry
            && building.queue.front() == Some(&UnitKind::Sentinel)
    }));

    while state.current_tick() < brain.dials.cadence {
        state.tick(&[]);
    }
    let next = brain.act(&state);
    assert!(
        next.iter().all(|command| !matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Turret,
                ..
            }
        )),
        "the paid in-progress defense must not be admitted twice: {next:?}"
    );
}

#[test]
fn strategic_air_spending_cannot_consume_the_opening_bootstrap_reserve() {
    let mut scenario = independent_bomber_operation_scenario();
    scenario.name = "brain opening bootstrap reserve".into();
    scenario.map[11].replace_range(20..=20, ".");
    scenario.players[0].scrap = 0;
    scenario
        .units
        .extend((0..4).map(|index| unit_spec(0, UnitKind::Harvester, 4 + index, 8)));
    scenario
        .units
        .extend((0..8).map(|index| unit_spec(0, UnitKind::Sentinel, 4 + index, 10)));
    let mut scouting_state = scenario
        .build()
        .expect("the connected strategic-capital fixture builds");
    for _ in 0..6_000 {
        scouting_state.tick(&[]);
    }

    let mut brain = operation_identity_brain(PlayerId(0), &scenario);

    for _ in 0..1_000 {
        let commands = brain.act(&scouting_state);
        let report = scouting_state.tick(&commands);
        assert_commands_accepted(&report, PlayerId(0));
        if (brain.mind().strategy).air_operation().is_some() {
            break;
        }
    }
    assert!(
        (brain.mind().strategy).air_operation().is_some(),
        "current scout sight should establish the strategic operation"
    );
    let mut bootstrap_brain = brain.clone();
    brain.exec = Executive::default();

    let mut bootstrap_scenario = scenario;
    bootstrap_scenario.name = "brain strategic opening bootstrap reserve".into();
    bootstrap_scenario.players[0].scrap = UnitKind::Harvester.stats().cost
        + BuildingKind::Extractor
            .base_stats()
            .construction
            .expect("Extractors have a construction price")
            .cost;
    bootstrap_scenario
        .buildings
        .retain(|building| building.kind != BuildingKind::Reclaimer);
    let mut kept_harvesters = 0usize;
    bootstrap_scenario.units.retain(|unit| {
        if unit.player != 0 || unit.kind != UnitKind::Harvester {
            return unit.kind != UnitKind::Kestrel;
        }
        kept_harvesters += 1;
        kept_harvesters <= 3
    });
    bootstrap_scenario
        .units
        .push(unit_spec(1, UnitKind::Harvester, 12, 11));
    let home_frame = TilePos::new(8, 11);
    let row = bootstrap_scenario
        .map
        .get_mut(home_frame.y as usize)
        .expect("the fixture contains the home-frame row");
    let mut bytes = row.as_bytes().to_vec();
    bytes[home_frame.x as usize] = b'E';
    *row = String::from_utf8(bytes).expect("the fixture map remains ASCII");
    let mut bootstrap_state = bootstrap_scenario
        .build()
        .expect("the opening-bootstrap continuation builds");
    while bootstrap_state.current_tick() <= scouting_state.current_tick()
        || !crate::difficulty::strategic_admission_tick(bootstrap_state.current_tick())
    {
        bootstrap_state.tick(&[]);
    }
    let bootstrap_scrap = UnitKind::Harvester.stats().cost
        + BuildingKind::Extractor
            .base_stats()
            .construction
            .expect("Extractors have a construction price")
            .cost;
    let mut document =
        serde_json::to_value(&bootstrap_state).expect("the bootstrap state serializes");
    document["players"][0]["scrap"] = serde_json::json!(bootstrap_scrap);
    bootstrap_state = serde_json::from_value(document).expect("the exact bootstrap bank is valid");
    bootstrap_brain.exec = Executive::default();

    let commands = bootstrap_brain.act(&bootstrap_state);
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));
    assert!(commands.iter().any(|command| matches!(
        command.command,
        Command::Build {
            kind: BuildingKind::Extractor,
            anchor,
            ..
        } if anchor == home_frame
    )));
    assert!(commands.iter().all(|command| !matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Kestrel | UnitKind::Buzzard | UnitKind::Condor | UnitKind::Skyhook,
            ..
        }
    )));
    let report = bootstrap_state.tick(&commands);
    assert_commands_accepted(&report, PlayerId(0));
    assert_eq!(
        bootstrap_state.player(PlayerId(0)).scrap,
        0,
        "the fourth worker and supported home Extractor own the full exact bootstrap bank"
    );
}

#[test]
fn exact_prime_core_cannot_be_frozen_into_a_new_team_relief() {
    let scenario = opening_core_team_relief_scenario();
    let state = scenario
        .build()
        .expect("opening-core team-relief scenario builds");

    let mut control = operation_identity_brain(PlayerId(0), &scenario);

    control.mind_mut().profile.traits.support = 70;
    control.dials.minimum_core_equivalents = 0;
    control.act(&state);
    assert!(
        serde_json::to_value(&control.mind().team).unwrap()["watch"].is_object(),
        "a feasible relief opportunity observes pressure without claiming its candidate"
    );

    let mut gated = operation_identity_brain(PlayerId(0), &scenario);

    gated.mind_mut().profile.traits.support = 70;
    gated.act(&state);

    let relief = &gated.mind().team;
    assert!(relief.operation().is_none());
    assert!(
        relief.reservations().is_empty(),
        "a new relief watch cannot reserve any of Prime's exact eight-unit core"
    );
}

#[test]
fn exact_prime_core_cannot_be_frozen_into_a_new_lift_payload() {
    let scenario = opening_core_lift_scenario();
    let state = scenario.build().expect("opening-core lift scenario builds");

    let mut control = operation_identity_brain(PlayerId(0), &scenario);

    control.dials.minimum_core_equivalents = 0;
    control.act(&state);
    assert!(
        (control.mind().lifts).operation().is_some(),
        "the visible disconnected objective otherwise admits a lift"
    );

    let mut gated = operation_identity_brain(PlayerId(0), &scenario);

    let commands = gated.act(&state);

    assert!(
        (gated.mind().lifts).operation().is_none(),
        "a new lift cannot freeze Prime's exact eight-unit core as payload"
    );
    assert!(commands.iter().all(|command| !matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Skyhook,
            ..
        }
    )));
}

pub(super) fn calibration_open_ferrous() -> Scenario {
    let map = [
        "################################################",
        "#..............................................#",
        "#..............................................#",
        "#.......ss.....................................#",
        "#..............................................#",
        "#....1....E....##..............................#",
        "#..............................................#",
        "#..............................................#",
        "#...................#..........................#",
        "#...........s.......#..........................#",
        "#.............s................................#",
        "#................E.............................#",
        "#..............................................#",
        "#..................S...........................#",
        "#..............................................#",
        "#..............................................#",
        "#...........................S..................#",
        "#............................E.................#",
        "#..............................................#",
        "#................................s.............#",
        "#..........................#.......s...........#",
        "#..........................#...................#",
        "#..............................................#",
        "#...................................E....2.....#",
        "#..............................##..............#",
        "#..............................................#",
        "#.....................................ss.......#",
        "#..............................................#",
        "#..............................................#",
        "################################################",
    ];
    Scenario {
        mode: Default::default(),
        name: "Calibration Open - Ferrous".into(),
        seed: 1_616_101,
        map: map.into_iter().map(str::to_owned).collect(),
        players: vec![
            player_spec("West Ferrous", Faction::Ferrous, 150, None),
            player_spec("East Ferrous", Faction::Ferrous, 150, None),
        ],
        units: vec![
            unit_spec(0, UnitKind::Harvester, 6, 8),
            unit_spec(0, UnitKind::Harvester, 7, 8),
            unit_spec(0, UnitKind::Harvester, 8, 7),
            unit_spec(0, UnitKind::Sentinel, 10, 8),
            unit_spec(1, UnitKind::Harvester, 41, 21),
            unit_spec(1, UnitKind::Harvester, 40, 21),
            unit_spec(1, UnitKind::Harvester, 39, 22),
            unit_spec(1, UnitKind::Sentinel, 37, 21),
        ],
        buildings: Vec::new(),
        meta: None,
    }
}

pub(super) fn opening_core_lift_scenario() -> Scenario {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.name = "brain opening-core lift admission".into();
    scenario.players[0].scrap = 50_000;
    let mut kept_sentinels = 0usize;
    scenario.units.retain(|unit| {
        if unit.kind != UnitKind::Sentinel {
            return true;
        }
        kept_sentinels += 1;
        kept_sentinels <= 8
    });
    scenario
}
