//! Stage executors, member scheduling, scout dispatch, and holds.

use super::*;

const STRATEGIC_AIR_QUEUE_DEPTH: usize = 2;

pub(super) fn remembered_recon(
    op: &mut AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let obs = context.ev.obs;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let previous_scout = op.scout;
    op.scout = remembered_recon_scout(op, obs, context.procurement.unavailable);
    if op.scout != previous_scout {
        op.scout_dispatch = None;
        if op.scout.is_some() {
            op.phase_started_at = obs.tick;
        }
    }
    dispatch_scout(
        op,
        plan,
        obs,
        context.ev.intel,
        context.landing_sites,
        connected_public_map(plan, context.ev.public_map),
        out,
    )?;
    schedule(
        context,
        &[(
            scout_kind,
            usize::from(op.scout.is_none())
                .saturating_sub(unowned_queued_scouts(context, scout_kind)),
        )],
    )
    .append_to(out);
    Ok(())
}

fn unowned_queued_scouts(context: &AirPlanningContext<'_>, scout: UnitKind) -> usize {
    let prior = context
        .procurement
        .lanes
        .prior_intents
        .iter()
        .filter(|intent| matches!(intent, Intent::TrainAt { kind, .. } if *kind == scout))
        .count();
    let unavailable = context
        .procurement
        .paid_exclusions
        .iter()
        .filter(|claim| claim.kind == scout)
        .count();
    queued(context.ev.obs, |kind| kind == scout)
        .saturating_add(prior)
        .saturating_sub(unavailable)
}

fn reconcile_preparation_members(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
) -> Result<(), AirRecoveryReason> {
    let obs = context.ev.obs;
    let route_unavailable = if let Some(resources) = context.connected_resources.as_ref() {
        connected_provider_unavailable(
            obs,
            &resources.targets,
            &[],
            context.ev.route(preferred_anchor(op, plan)),
        )
    } else {
        Vec::new()
    };
    let unavailable = merged_unavailable(context.procurement.unavailable, &route_unavailable);
    let previous_scout = op.scout;
    let previous_artillery = op.artillery.clone();
    let previous_strike_aircraft = op.strike_aircraft.clone();
    op.scout = retained_scout(op.scout, obs, &unavailable);
    if op.scout != previous_scout {
        op.scout_dispatch = None;
        if op.scout.is_some() && op.phase() == AirOperationPhase::Recon {
            op.phase_started_at = obs.tick;
        }
    }
    assign_artillery(&mut op.artillery, plan, obs, &unavailable);
    assign_strike_aircraft(&mut op.strike_aircraft, plan, obs, &unavailable);
    if previous_strike_aircraft != op.strike_aircraft {
        op.strike_hold = None;
    }
    if previous_scout
        .is_some_and(|id| unit(obs, id).is_some() && route_unavailable.binary_search(&id).is_ok())
        && op.scout.is_none()
        || previous_strike_aircraft
            .iter()
            .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
            && op.strike_aircraft.len() < plan.desired_strike_aircraft()
    {
        return Err(AirRecoveryReason::UnreachableAirRoute);
    }
    if previous_artillery
        .iter()
        .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
        && op.artillery.len() < plan.desired_artillery()
    {
        return Err(AirRecoveryReason::UnreachableStaging);
    }
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    let desired_screen = plan.desired_screen();
    if let Some(screen) = plan.screen_mut() {
        assign_exact(
            screen,
            desired_screen,
            obs,
            context.procurement.unavailable,
            |kind| kind == screen_kind,
        );
    }
    if connected_package_is_proven_infeasible(op, plan, context) {
        return Err(AirRecoveryReason::PreparationInfeasible);
    }
    Ok(())
}

pub(super) fn recon(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let AirEvidence {
        tuning, obs, intel, ..
    } = context.ev;
    let landing_sites = context.landing_sites;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.ev.public_map);
    reconcile_preparation_members(op, plan, context)?;
    dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)?;
    schedule_missing_members(op, plan, context, scout_kind, out);
    if matches!(plan, AirPlan::Connected(_)) {
        hold_strike_aircraft(op, obs, context.ev.home, out);
    }
    if op.scout_dispatch.is_some()
        && target_seen(op, plan, obs)
        && elapsed(op.phase_started_at, obs.tick) >= tuning.reaction_delay
    {
        enter(op, AirStage::Assemble, obs.tick);
    }
    Ok(())
}

pub(super) fn assemble(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let AirEvidence {
        obs, intel, home, ..
    } = context.ev;
    let landing_sites = context.landing_sites;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.ev.public_map);
    reconcile_preparation_members(op, plan, context)?;
    schedule_missing_members(op, plan, context, scout_kind, out);
    let complete = assembly_complete(op, plan);
    if matches!(plan, AirPlan::Connected(_)) && !complete {
        hold_strike_aircraft(op, obs, home, out);
    }
    if complete {
        if plan.airborne() {
            dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)?;
            enter(op, AirStage::SuppressAa, obs.tick);
            hold_air_strike(op, plan, obs, home, out);
            return Ok(());
        }
        let objective = operation_objective_anchor(op, plan, intel);
        let staging = match artillery_staging(
            op,
            obs,
            home,
            objective,
            context.ev.public_map,
            context.ev.orientation,
        ) {
            None => return Err(AirRecoveryReason::UnreachableStaging),
            Some(ArtilleryStaging::NeedsRecon(goal)) => {
                dispatch_scout_to(op, obs, goal, public_map, out)?;
                hold_strike_aircraft(op, obs, home, out);
                return Ok(());
            }
            Some(ArtilleryStaging::Ready(staging)) => staging,
        };
        dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)?;
        enter(op, AirStage::SuppressAa, obs.tick);
        stage_artillery(op, staging, out);
        hold_strike_aircraft(op, obs, home, out);
    }
    Ok(())
}

pub(super) fn assembly_complete(op: &AirOperation, plan: &AirPlan) -> bool {
    op.scout.is_some()
        && op.artillery.len() == plan.desired_artillery()
        && op.strike_aircraft.len() == plan.desired_strike_aircraft()
        && plan.screen().len() == plan.desired_screen()
}

pub(super) fn suppress(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let tuning = context.ev.tuning;
    let obs = context.ev.obs;
    let intel = context.ev.intel;
    let home = context.ev.home;
    let landing_sites = context.landing_sites;
    let public_map = connected_public_map(plan, context.ev.public_map);
    let cluster_aa = (!plan.airborne()).then(|| cluster_air_defense(op, plan, intel));
    let connected_engagement = if plan.airborne() {
        None
    } else {
        prosecutable_cluster_air_defense_target(
            op,
            plan,
            obs,
            intel,
            public_map,
            context.ev.orientation,
        )
    };
    let air_defense = if plan.airborne() {
        targetable_corridor_flak(intel, home, op.target, landing_sites).map(Target::Building)
    } else {
        connected_engagement
            .as_ref()
            .map(|engagement| engagement.target)
    };
    if let Some(air_defense) = air_defense {
        if elapsed(op.phase_started_at, obs.tick) >= tuning.reaction_delay {
            let units = if plan.airborne() {
                air_strike_members(op, plan, obs)
            } else {
                op.artillery.clone()
            };
            let firing_stands = connected_engagement
                .as_ref()
                .map(|engagement| engagement.firing_stands.clone())
                .unwrap_or_default();
            let positioned = plan.airborne()
                || firing_stands
                    .iter()
                    .all(|(id, stand)| unit(obs, *id).is_some_and(|member| member.tile == *stand));
            if !positioned {
                let dispatch = SuppressionDispatch::Position {
                    target: air_defense,
                    assignments: firing_stands.clone(),
                };
                let changed = plan.suppression_dispatch() != Some(&dispatch);
                for (id, goal) in firing_stands {
                    if unit(obs, id)
                        .is_some_and(|member| member.tile != goal && (changed || member.idle))
                    {
                        out.intents.push(Intent::MoveUnits {
                            units: vec![id],
                            goal,
                        });
                    }
                }
                plan.set_suppression_dispatch(Some(dispatch));
                op.artillery_staging = None;
            } else {
                let dispatch = SuppressionDispatch::Attack {
                    target: air_defense,
                    units: units.clone(),
                };
                let repeat_refused_ground_order = !plan.airborne()
                    && units
                        .iter()
                        .any(|id| unit(obs, *id).is_some_and(|member| member.idle));
                if plan.suppression_dispatch() != Some(&dispatch) || repeat_refused_ground_order {
                    out.intents.push(Intent::AttackUnits {
                        units: units.clone(),
                        target: air_defense,
                    });
                }
                if plan.airborne() {
                    // The suppression attack displaced the prior home hold.
                    // Clearing it lets Verify issue a fresh regroup order
                    // before the strike aircraft commit to the primary objective.
                    op.strike_hold = None;
                    plan.set_strike_dispatch(None);
                } else {
                    op.artillery_staging = None;
                }
                plan.set_suppression_dispatch(Some(dispatch));
            }
        }
        if plan.airborne() {
            dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)
        } else {
            scout_and_hold(op, plan, context, &[], out)
        }
    } else {
        plan.set_suppression_dispatch(None);
        if plan.airborne() {
            return match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
                AirborneCorridorStatus::Defended => Err(AirRecoveryReason::NewAirDefense),
                AirborneCorridorStatus::Clear => {
                    enter(op, AirStage::Verify, obs.tick);
                    scout_and_hold(op, plan, context, landing_sites, out)
                }
                AirborneCorridorStatus::NeedsRecon => {
                    scout_and_hold(op, plan, context, landing_sites, out)
                }
            };
        }
        match cluster_aa
            .expect("connected suppression has a cluster assessment")
            .evidence
        {
            AirDefenseEvidence::CurrentCoverage => Err(AirRecoveryReason::NewAirDefense),
            AirDefenseEvidence::VisibleWithoutKnownCoverage
                if corridor_clear(
                    intel,
                    home,
                    operation_objective_anchor(op, plan, intel),
                    &[],
                ) =>
            {
                enter(op, AirStage::Verify, obs.tick);
                scout_and_hold(op, plan, context, &[], out)
            }
            AirDefenseEvidence::RememberedCoverage
            | AirDefenseEvidence::Unknown
            | AirDefenseEvidence::VisibleWithoutKnownCoverage => {
                scout_and_hold(op, plan, context, &[], out)
            }
        }
    }
}

pub(super) fn verify(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let tuning = context.ev.tuning;
    let obs = context.ev.obs;
    let intel = context.ev.intel;
    let home = context.ev.home;
    let landing_sites = context.landing_sites;
    let (cluster_aa, air_defense) = stage_air_defense(op, plan, context);
    if air_defense.is_some() {
        enter(op, AirStage::SuppressAa, obs.tick);
        return suppress(op, plan, context, out);
    }
    if plan.airborne() {
        return match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
            AirborneCorridorStatus::Defended => Err(AirRecoveryReason::NewAirDefense),
            AirborneCorridorStatus::Clear
                if elapsed(op.phase_started_at, obs.tick)
                    >= tuning
                        .reaction_delay
                        .saturating_add(tuning.commitment_hesitation) =>
            {
                enter(op, AirStage::Strike, obs.tick);
                strike(op, plan, context, out)
            }
            AirborneCorridorStatus::Clear | AirborneCorridorStatus::NeedsRecon => {
                scout_and_hold(op, plan, context, landing_sites, out)
            }
        };
    }
    match cluster_aa
        .expect("connected verification has a cluster assessment")
        .evidence
    {
        AirDefenseEvidence::CurrentCoverage => Err(AirRecoveryReason::NewAirDefense),
        AirDefenseEvidence::VisibleWithoutKnownCoverage
            if corridor_clear(
                intel,
                home,
                operation_objective_anchor(op, plan, intel),
                &[],
            ) && elapsed(op.phase_started_at, obs.tick)
                >= tuning
                    .reaction_delay
                    .saturating_add(tuning.commitment_hesitation) =>
        {
            enter(op, AirStage::Strike, obs.tick);
            strike(op, plan, context, out)
        }
        AirDefenseEvidence::RememberedCoverage
        | AirDefenseEvidence::Unknown
        | AirDefenseEvidence::VisibleWithoutKnownCoverage => {
            scout_and_hold(op, plan, context, &[], out)
        }
    }
}

pub(super) fn strike(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let tuning = context.ev.tuning;
    let obs = context.ev.obs;
    let intel = context.ev.intel;
    let home = context.ev.home;
    let landing_sites = context.landing_sites;
    let public_map = connected_public_map(plan, context.ev.public_map);
    let (cluster_aa, air_defense) = stage_air_defense(op, plan, context);
    let connected_cluster_needs_clearance = cluster_aa.is_some_and(|assessment| {
        assessment.has_targets && assessment.evidence == AirDefenseEvidence::CurrentCoverage
    });
    if air_defense.is_some() || connected_cluster_needs_clearance {
        enter(op, AirStage::SuppressAa, obs.tick);
        return suppress(op, plan, context, out);
    }
    let strike_settled = op
        .strike_issued_at
        .is_some_and(|tick| elapsed(tick, obs.tick) >= tuning.reaction_delay.max(20));
    if matches!(plan, AirPlan::Connected(_))
        && strike_settled
        && operation_objective_cleared(op, plan, obs, intel)
    {
        // Every admitted member was observed gone, so completion needs no
        // renewed sight of their anchors or approach.
        return Err(AirRecoveryReason::Complete);
    }
    let live_target = live_strike_target(op, plan, intel);
    let remembered_target = live_target
        .is_none()
        .then(|| best_remembered_member(plan, intel))
        .flatten();
    let strike_anchor = operation_objective_anchor(op, plan, intel);
    let staging = if plan.airborne() {
        None
    } else {
        match artillery_staging(
            op,
            obs,
            home,
            strike_anchor,
            context.ev.public_map,
            context.ev.orientation,
        ) {
            None => return Err(AirRecoveryReason::UnreachableStaging),
            Some(ArtilleryStaging::NeedsRecon(goal)) => {
                dispatch_scout_to(op, obs, goal, public_map, out)?;
                hold_strike_aircraft(op, obs, home, out);
                return Ok(());
            }
            Some(ArtilleryStaging::Ready(staging)) => Some(staging),
        }
    };
    let corridor_clear = if plan.airborne() {
        airborne_corridor_status(op, plan, obs, intel, home, landing_sites)
            == AirborneCorridorStatus::Clear
    } else if remembered_target.is_some() {
        // Current sight cannot cover the approach to a member that has left
        // it, so only known anti-air along the route blocks reacquisition.
        route_without_known_air_defense(intel, home, strike_anchor)
    } else {
        corridor_clear(intel, home, strike_anchor, landing_sites)
    };
    if !corridor_clear {
        return Err(AirRecoveryReason::NewAirDefense);
    }
    let attackers = air_strike_members(op, plan, obs);
    if let Some(target) = live_target {
        if let Some(id) = target.id {
            let mut air_routes = operation_route_projection(
                plan,
                obs,
                Domain::Air,
                public_map,
                context.ev.orientation,
            );
            if !exact_attack_group_reaches(&mut air_routes, obs, &attackers, target.anchor) {
                return Err(AirRecoveryReason::UnreachableAirRoute);
            }
            dispatch_air_strike(
                plan,
                obs,
                &attackers,
                AirStrikeDispatch::Attack {
                    target: id,
                    anchor: target.anchor,
                },
                out,
            );
        }
        op.strike_issued_at.get_or_insert(obs.tick);
    } else if operation_objective_cleared(op, plan, obs, intel) {
        if strike_settled {
            return Err(AirRecoveryReason::Complete);
        }
        let air_routes =
            operation_route_projection(plan, obs, Domain::Air, public_map, context.ev.orientation);
        let cleared_anchor = last_strike_anchor(plan).unwrap_or(strike_anchor);
        if !air_routes.group_reaches_command_goal(&attackers, cleared_anchor) {
            return Err(AirRecoveryReason::UnreachableAirRoute);
        }
        dispatch_air_strike(
            plan,
            obs,
            &attackers,
            AirStrikeDispatch::AttackMove(cleared_anchor),
            out,
        );
        op.strike_issued_at.get_or_insert(obs.tick);
    } else if let Some(remembered) = remembered_target {
        // Every live member has left current sight. Flying toward the best
        // remembered one reacquires it instead of idling out the phase.
        let air_routes =
            operation_route_projection(plan, obs, Domain::Air, public_map, context.ev.orientation);
        if !air_routes.group_reaches_command_goal(&attackers, remembered.anchor) {
            return Err(AirRecoveryReason::UnreachableAirRoute);
        }
        dispatch_air_strike(
            plan,
            obs,
            &attackers,
            AirStrikeDispatch::AttackMove(remembered.anchor),
            out,
        );
        op.strike_issued_at.get_or_insert(obs.tick);
    }
    if let Some(staging) = staging {
        stage_artillery(op, staging, out);
    }
    Ok(())
}

fn dispatch_air_strike(
    plan: &mut AirPlan,
    obs: &Observation,
    attackers: &[UnitId],
    dispatch: AirStrikeDispatch,
    out: &mut StrategicDecision,
) {
    let units = if plan.strike_dispatch() == Some(dispatch) {
        attackers
            .iter()
            .copied()
            .filter(|id| unit(obs, *id).is_some_and(|member| member.idle))
            .collect()
    } else {
        attackers.to_vec()
    };
    plan.set_strike_dispatch(Some(dispatch));
    if units.is_empty() {
        return;
    }
    out.intents.push(match dispatch {
        AirStrikeDispatch::Attack { target, .. } => Intent::AttackUnits {
            units,
            target: Target::Building(target),
        },
        AirStrikeDispatch::AttackMove(goal) => Intent::AttackMoveUnits { units, goal },
    });
}

/// Schedules in demand order, spreading equal-load work across producers. If
/// the next otherwise-trainable member is unaffordable or all its queues are
/// full, the available bank is claimed so ordinary production cannot skim it.
pub(super) fn schedule(
    context: &AirPlanningContext<'_>,
    demands: &[(UnitKind, usize)],
) -> ProductionPlan {
    let mut out = ProductionPlan::default();
    let procurement = context.procurement;
    if !procurement.allow {
        return out;
    }
    let obs = context.ev.obs;
    let mut bank = obs.scrap.saturating_sub(procurement.reserve.current);
    let mut production = crate::production::ImmediateProduction::new(
        obs,
        procurement.lanes.reservations,
        procurement.lanes.prior_intents,
    );
    'demands: for &(kind, count) in demands {
        if !requirements_met(obs, kind) || !has_producer(obs, kind) {
            continue;
        }
        for _ in 0..count {
            let cost = kind.stats().cost;
            let producer = production
                .available(kind, |building| {
                    if building == BuildingKind::Airworks {
                        STRATEGIC_AIR_QUEUE_DEPTH
                    } else {
                        QUEUE_CAP
                    }
                })
                .min_by_key(|producer| (producer.depth, producer.id));
            if bank < cost || producer.is_none() {
                out.reserved_scrap = out.reserved_scrap.saturating_add(bank.min(cost));
                break 'demands;
            }
            let Some(producer) = producer else {
                break 'demands;
            };
            bank -= cost;
            out.purchases.push(production.append_purchase(producer));
        }
    }
    out
}

fn schedule_missing_members(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    scout_kind: UnitKind,
    out: &mut StrategicDecision,
) {
    if let AirPlan::Island(island) = plan {
        schedule(
            context,
            &missing_island_members(
                AirRoster::from(op),
                island.screen.len(),
                island,
                context,
                scout_kind,
            ),
        )
        .append_to(out);
    }
}

pub(super) fn missing_island_members(
    members: AirRoster<'_>,
    screen_count: usize,
    plan: &IslandPlan,
    context: &AirPlanningContext<'_>,
    scout_kind: UnitKind,
) -> [(UnitKind, usize); 3] {
    let obs = context.ev.obs;
    let bomber_kind = Role::Bomber.unit_for(obs.faction);
    let missing_scout = 1usize.saturating_sub(
        usize::from(members.scout.is_some()) + unowned_queued_scouts(context, scout_kind),
    );
    let missing_strike_aircraft = plan
        .desired_strike_aircraft
        .saturating_sub(members.strike_aircraft.len() + queued(obs, |k| k == bomber_kind));
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    let missing_screen = plan
        .desired_screen
        .saturating_sub(screen_count + queued(obs, |kind| kind == screen_kind));
    [
        (scout_kind, missing_scout),
        (screen_kind, missing_screen),
        (bomber_kind, missing_strike_aircraft),
    ]
}

pub(super) fn connected_package_is_proven_infeasible(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
) -> bool {
    let Some(connected) = plan.connected() else {
        return false;
    };
    let package = &connected.package;
    if !context.procurement.allow || context.ev.obs.tick >= package.preparation_deadline {
        return false;
    }
    let resources = context
        .connected_resources
        .as_ref()
        .expect("connected preparation has one observation-bound resource view");
    let outstanding = missing_package_demands(
        package,
        AirRoster::from(op),
        context.ev.obs,
        &resources.snapshot,
        package.preparation_deadline,
        &resources.access,
    );
    matches!(
        refine_provider_demands(
            ProductionEvidence::with_planning(
                &resources.snapshot,
                &resources.access,
                Some(context.procurement.planning)
            ),
            &outstanding,
            context.ev.obs.tick,
            PreparationConstraints {
                deadline: package.preparation_deadline,
                decision_cadence: context.ev.tuning.cadence,
                protected_forecast_scrap: context.procurement.reserve.forecast,
            },
            connected.commitment.key(),
        ),
        crate::planning::Progress::ProvenInfeasible
    )
}

fn scout_and_hold(
    op: &mut AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    landing_sites: &[TilePos],
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    dispatch_scout(
        op,
        plan,
        context.ev.obs,
        context.ev.intel,
        landing_sites,
        connected_public_map(plan, context.ev.public_map),
        out,
    )?;
    hold_air_strike(op, plan, context.ev.obs, context.ev.home, out);
    Ok(())
}

/// Moves the scout toward its next objective, failing when no air route
/// reaches it.
pub(super) fn dispatch_scout(
    op: &mut AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let goal = reachable_scout_goal(op, plan, obs, intel, landing_sites, public_map)
        .ok_or(AirRecoveryReason::UnreachableAirRoute)?;
    issue_scout_dispatch(op, obs, goal, out);
    Ok(())
}

/// The scout's next objective, when a known air route still reaches it.
pub(super) fn reachable_scout_goal(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    scout_dispatch_goal(op, plan, obs, intel, landing_sites, public_map)
        .filter(|goal| scout_dispatch_is_viable(op, obs, *goal, public_map))
}

fn scout_dispatch_goal(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let target = connected_scout_focus(op, plan, obs, intel);
    scout_goal(op, obs, intel, target, landing_sites, public_map)
}

pub(super) fn connected_scout_focus(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> TilePos {
    let Some(connected) = plan.connected() else {
        return op.target;
    };
    let mut contacts = connected.commitment.live_members(intel);
    contacts.sort_unstable_by_key(|contact| {
        (contact.anchor.y, contact.anchor.x, contact.id, contact.kind)
    });
    for contact in contacts {
        let (width, height) = contact.kind.tier_stats(contact.tier).size;
        for dy in 0..height {
            for dx in 0..width {
                let tile = contact.anchor.offset(dx, dy);
                if !obs.visible(tile) {
                    return tile;
                }
            }
        }
    }
    operation_objective_anchor(op, plan, intel)
}

fn dispatch_scout_to(
    op: &mut AirOperation,
    obs: &Observation,
    goal: TilePos,
    public_map: Option<&PublicMapBriefing>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    if !scout_dispatch_is_viable(op, obs, goal, public_map) {
        return Err(AirRecoveryReason::UnreachableAirRoute);
    }
    issue_scout_dispatch(op, obs, goal, out);
    Ok(())
}

fn issue_scout_dispatch(
    op: &mut AirOperation,
    obs: &Observation,
    goal: TilePos,
    out: &mut StrategicDecision,
) {
    let Some(scout) = op.scout else {
        return;
    };
    let member = unit(obs, scout).expect("a viable scout dispatch retains its live unit");
    if op.scout_dispatch == Some((scout, goal)) {
        return;
    }
    op.scout_dispatch = Some((scout, goal));
    if !member.idle || member.tile.chebyshev(goal) > 1 {
        out.intents.push(Intent::MoveUnits {
            units: vec![scout],
            goal,
        });
    }
}

fn scout_dispatch_is_viable(
    op: &AirOperation,
    obs: &Observation,
    goal: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    let Some(scout) = op.scout else {
        return true;
    };
    let Some(member) = unit(obs, scout) else {
        return false;
    };
    let mut air_routes = route_projection(obs, Domain::Air, public_map);
    air_routes.unit_reaches(member, goal)
        && (op.scout_dispatch != Some((scout, goal))
            || !member.idle
            || member.tile.chebyshev(goal) <= 1)
}

pub(super) fn scout_goal(
    op: &AirOperation,
    obs: &Observation,
    intel: &StrategicIntelligence,
    target: TilePos,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let vision = Role::Scout
        .unit_for(obs.faction)
        .stats()
        .vision
        .saturating_sub(1);
    let current = op
        .scout
        .and_then(|id| unit(obs, id))
        .map_or(target, |scout| scout.tile);
    let focus = flight_objectives(target, landing_sites)
        .into_iter()
        .find(|objective| {
            approach(current, *objective).any(|tile| {
                intel.air_defense_at(tile).evidence()
                    != AirDefenseEvidence::VisibleWithoutKnownCoverage
            })
        })
        .unwrap_or(target);
    let routes = route_projection(obs, Domain::Air, public_map);
    let radius_sq = vision.saturating_mul(vision);
    (focus.y - vision..=focus.y + vision)
        .flat_map(|y| (focus.x - vision..=focus.x + vision).map(move |x| TilePos::new(x, y)))
        .filter(|tile| {
            routing::in_bounds(obs, *tile) && {
                let dx = tile.x - focus.x;
                let dy = tile.y - focus.y;
                dx.saturating_mul(dx) + dy.saturating_mul(dy) <= radius_sq
            }
        })
        .filter(|tile| routes.reaches(current, *tile))
        .min_by_key(|tile| {
            let evidence = match intel.air_defense_at(*tile).evidence() {
                AirDefenseEvidence::VisibleWithoutKnownCoverage => 0,
                AirDefenseEvidence::Unknown => 1,
                AirDefenseEvidence::RememberedCoverage => 2,
                AirDefenseEvidence::CurrentCoverage => 3,
            };
            (
                evidence,
                tile.chebyshev(current),
                tile.chebyshev(focus),
                tile.y,
                tile.x,
            )
        })
}

/// Parks the strike aircraft on the landing pad. A landed aircraft is idle, so
/// the later strike dispatch lifts it off exactly like a person clicking an
/// attack on a parked aircraft.
pub(super) fn hold_strike_aircraft(
    op: &mut AirOperation,
    obs: &Observation,
    home: TilePos,
    out: &mut StrategicDecision,
) {
    let pad = landing_pad(obs, home).unwrap_or(home);
    if !op.strike_aircraft.is_empty() && op.strike_hold != Some(pad) {
        out.intents.push(Intent::MoveUnits {
            units: op.strike_aircraft.clone(),
            goal: pad,
        });
        op.strike_hold = Some(pad);
    }
}

pub(super) fn hold_air_strike(
    op: &mut AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    home: TilePos,
    out: &mut StrategicDecision,
) {
    if !plan.airborne() {
        hold_strike_aircraft(op, obs, home, out);
        return;
    }
    let mut units = op.strike_aircraft.clone();
    units.extend(plan.screen().iter().copied());
    units.sort_unstable();
    units.dedup();
    let pad = landing_pad(obs, home).unwrap_or(home);
    if units.is_empty() || op.strike_hold == Some(pad) {
        return;
    }
    // Turn-limited kinds set down on the pad at the end of their move; the
    // screen holds airborne over home.
    let (landing, circling): (Vec<UnitId>, Vec<UnitId>) = units
        .into_iter()
        .partition(|id| unit(obs, *id).is_some_and(|member| member.kind.stats().turn_rate > 0));
    if !landing.is_empty() {
        out.intents.push(Intent::MoveUnits {
            units: landing,
            goal: pad,
        });
    }
    if !circling.is_empty() {
        out.intents.push(Intent::MoveUnits {
            units: circling,
            goal: home,
        });
    }
    op.strike_hold = Some(pad);
}

pub(super) fn stage_artillery(
    op: &mut AirOperation,
    staging: TilePos,
    out: &mut StrategicDecision,
) {
    if !op.artillery.is_empty() && op.artillery_staging != Some(staging) {
        out.intents.push(Intent::MoveUnits {
            units: op.artillery.clone(),
            goal: staging,
        });
        op.artillery_staging = Some(staging);
    }
}
