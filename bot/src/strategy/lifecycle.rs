//! The planner's per-decision lifecycle and recovery rules.

use super::*;

pub(super) struct AirPlanningContext<'a> {
    pub(super) ev: AirEvidence<'a>,
    /// Procurement whose `unavailable` units are every unit the operation
    /// may not enlist this decision.
    pub(super) procurement: AirProcurement<'a>,
    pub(super) landing_sites: &'a [TilePos],
    pub(super) connected_resources: Option<ConnectedProductionResources>,
}

impl StrategicPlanner {
    /// Releases unpaid connected demand during emergency economy recovery
    /// and returns every still-routable member immediately. The operation
    /// stays in recovery even with no survivors: this path returns before Lift
    /// runs, so the next full decision settles it and hands the abort to Lift.
    pub(crate) fn recover_unpaid_connected_for_economy_emergency(
        &mut self,
        context: EconomyEmergencyRecovery<'_>,
    ) -> Option<StrategicDecision> {
        let EconomyEmergencyRecovery {
            profile,
            tuning,
            obs,
            home,
            public_map,
            orientation,
            recon_paid_exclusions,
        } = context;
        // This path returns before the normal observation would prune.
        self.prune_paid_production(obs);
        let has_unpaid_provider = self.air.as_ref().is_some_and(|active| {
            if active.op.phase() > AirOperationPhase::Assemble {
                return false;
            }
            let Some(package) = active.plan.package() else {
                return false;
            };
            let snapshot = ResourceSnapshot::from_observation(obs);
            let access = emergency_paid_queue_access(obs, recon_paid_exclusions);
            // Demand a paid queue can still deliver costs no further scrap,
            // whichever program bought it. The obligation path credits the same
            // occurrences, so recalling the operation over them would abort an
            // attack that needs no money.
            connected_provider_shortfall(active, obs)
                .into_iter()
                .any(|(kind, count)| {
                    count
                        > count_paid_queued_ready_with_access(
                            &snapshot,
                            kind,
                            package.preparation_deadline,
                            &access,
                        )
                })
        });
        if !has_unpaid_provider {
            return None;
        }

        let ActiveAirOperation { op, plan } = self
            .air
            .as_mut()
            .expect("unpaid connected demand belongs to one active operation");
        recover(op, AirRecoveryReason::PreparationInfeasible, obs.tick);
        self.cooldown_until = obs.tick.saturating_add(cooldown(profile, tuning));
        let mut out = StrategicDecision::default();
        reconcile_recovery_return(
            op,
            plan,
            RecoveryReturnContext {
                obs,
                home,
                public_map,
                orientation,
                issue_order: true,
            },
            &mut out,
        );
        out.reservations = reservations(op, plan, obs);
        Some(out)
    }

    pub(super) fn think(
        &mut self,
        ev: AirEvidence<'_>,
        inputs: ThinkInputs<'_>,
    ) -> StrategicDecision {
        let AirEvidence {
            profile,
            obs,
            intel,
            home,
            public_map,
            ..
        } = ev;
        let mut active = match self.begin_or_resume(ev, &inputs) {
            Ok(active) => active,
            Err(idle) => return idle,
        };
        let objective_gone = self.watch_outcomes(obs, intel, &active);
        let ActiveAirOperation { op, plan } = &mut active;
        if let Some(screen) = plan.screen_mut() {
            screen.retain(|id| {
                unit(obs, *id)
                    .is_some_and(|member| member.kind == Role::AirGround.unit_for(obs.faction))
            });
        }
        let owned = reservations(op, plan, obs);
        if owned.iter().any(|id| inputs.claimed_elsewhere.contains(id)) {
            self.air = Some(active);
            return StrategicDecision {
                reservations: owned,
                ..Default::default()
            };
        }
        let began_in_recovery = op.phase() == AirOperationPhase::Recover;
        refresh_target(op, plan, intel);
        if !op.assault_admitted()
            && strategic_admission_tick(obs.tick)
            && let Some(current_target) = current_target_contact(op, intel)
        {
            let admitted_at = plan.admitted_at();
            if wealthy_island_target(profile, obs, home, current_target, public_map) {
                let mut admitted = IslandPlan::new(profile, obs, inputs.procurement.lanes);
                op.admit_assault(obs.tick);
                admitted.admitted_at = admitted_at;
                *plan = AirPlan::Island(admitted);
            }
        }
        if op.assault_admitted()
            && op.phase() <= AirOperationPhase::Assemble
            && let AirPlan::Connected(connected) = plan
        {
            connected.refocus(intel);
        }
        if !began_in_recovery && op.phase() != AirOperationPhase::Recover {
            abort_if_needed(op, plan, profile, obs, intel);
        }

        let mut out = StrategicDecision::default();
        let landing_sites = landing_sites(inputs.lift_support, op);
        let owned = reservations(op, plan, obs);
        let mut enlisted = excluding_owned(inputs.procurement.unavailable, &owned);
        if inputs.owned_only {
            enlisted.extend(
                obs.my_units
                    .iter()
                    .filter_map(|unit| (!owned.contains(&unit.id)).then_some(unit.id)),
            );
            enlisted.sort_unstable();
            enlisted.dedup();
        }
        let context = planning_context(
            ev,
            AirProcurement {
                unavailable: &enlisted,
                ..inputs.procurement
            },
            op,
            plan,
            &landing_sites,
        );
        let staged = match op.stage {
            AirStage::Watching => remembered_recon(op, plan, &context, &mut out),
            AirStage::Recon => recon(op, plan, &context, &mut out),
            AirStage::Assemble => assemble(op, plan, &context, &mut out),
            AirStage::SuppressAa => suppress(op, plan, &context, &mut out),
            AirStage::Verify => verify(op, plan, &context, &mut out),
            AirStage::Strike => strike(op, plan, &context, &mut out),
            AirStage::Recover { .. } => Ok(()),
        };
        let end = self.finish_stage(
            ev,
            &inputs,
            began_in_recovery,
            staged,
            &mut active,
            &mut out,
        );
        if let Some(reason) = active.op.recovery_reason() {
            let (outcome, reason, confidence, doctrine) =
                recovery_outcome(reason, objective_gone, &active.op, &self.outcomes, obs);
            self.outcomes
                .finish(obs, outcome, reason, confidence, doctrine);
        }
        match end {
            OperationEnd::Settled
                if !out.reservations.is_empty()
                    && reusable_survivors(active.op.recovery_reason()) =>
            {
                self.standby = AirStandby::from_operation(&active.op, obs);
                self.terminal_outcome = Some(air_operation_outcome(&active.op));
            }
            OperationEnd::Settled | OperationEnd::Released => {
                self.terminal_outcome = Some(air_operation_outcome(&active.op));
            }
            OperationEnd::Continue => self.air = Some(active),
        }
        out
    }

    /// The operation this observation continues or begins, or the idle
    /// decision when there is none.
    fn begin_or_resume(
        &mut self,
        ev: AirEvidence<'_>,
        inputs: &ThinkInputs<'_>,
    ) -> Result<ActiveAirOperation, StrategicDecision> {
        self.terminal_outcome = None;
        self.standby.prune(ev.obs);
        let holding = |standby: &AirStandby| StrategicDecision {
            reservations: standby.reservations(),
            ..StrategicDecision::default()
        };
        if ev.intel.observed_at() != Some(ev.obs.tick) {
            return Err(holding(&self.standby));
        }
        if let Some(active) = self.air.take() {
            return Ok(active);
        }
        match self.admission(ev, inputs.procurement.allow, inputs.lift_support) {
            AirAdmission::Hold => Err(holding(&self.standby)),
            AirAdmission::NoTarget => {
                self.standby = AirStandby::default();
                Err(StrategicDecision::default())
            }
            AirAdmission::Begin(selected) => {
                let standby = core::mem::take(&mut self.standby);
                Ok(fresh_air_operation(
                    ev.profile,
                    ev.obs,
                    inputs.procurement.lanes,
                    selected,
                    standby,
                ))
            }
        }
    }

    /// Whether a fresh operation may begin on this observation. Reads only
    /// planner state, so the carrier preview and the lifecycle share it.
    pub(super) fn admission<'i>(
        &self,
        ev: AirEvidence<'i>,
        allow_new_operation: bool,
        lift_support: Option<&LiftSupportRequest>,
    ) -> AirAdmission<'i> {
        let AirEvidence {
            profile,
            tuning,
            obs,
            intel,
            home,
            public_map,
            ..
        } = ev;
        if !allow_new_operation || obs.tick < self.cooldown_until {
            return AirAdmission::Hold;
        }
        let Some(selected) =
            select_fresh_air_target(profile, tuning, obs, intel, home, lift_support, public_map)
        else {
            return AirAdmission::NoTarget;
        };
        if !strategic_admission_tick(obs.tick) {
            return AirAdmission::Hold;
        }
        AirAdmission::Begin(selected)
    }

    /// Opens or continues the journal episode and reports whether current
    /// sight confirms the objective gone.
    fn watch_outcomes(
        &mut self,
        obs: &Observation,
        intel: &StrategicIntelligence,
        ActiveAirOperation { op, plan }: &ActiveAirOperation,
    ) -> bool {
        use crate::experience::{
            Doctrine, EpisodeId, EpisodeOwner, ExperienceKey, ExperienceSubject,
        };
        let members: Vec<_> = op.members().chain(plan.screen().iter().copied()).collect();
        self.outcomes.watch(
            obs,
            EpisodeId {
                owner: EpisodeOwner::Air,
                serial: plan.admitted_at(),
            },
            ExperienceKey {
                doctrine: Doctrine::Air,
                y: op.target.y,
                x: op.target.x,
                subject: ExperienceSubject::Building(op.target_id),
            },
            &members,
            op.phase() as u8,
        );
        match plan {
            AirPlan::Connected(connected) => {
                let commitment = &connected.commitment;
                let live_anchors: Vec<_> = commitment
                    .live_members(intel)
                    .into_iter()
                    .map(|member| member.anchor)
                    .collect();
                self.outcomes.observe_objective(obs, commitment.primary);
                self.outcomes.observe_cluster(
                    obs,
                    commitment.player,
                    &commitment.anchors,
                    &live_anchors,
                )
            }
            AirPlan::Reacquire(_) | AirPlan::Island(_) => op
                .target_id
                .is_some_and(|id| self.outcomes.observe_objective(obs, id)),
        }
    }

    /// Applies the stage verdict, the preparation deadline, and closed
    /// funding, then issues the one return order when recovery begins.
    fn finish_stage(
        &mut self,
        ev: AirEvidence<'_>,
        inputs: &ThinkInputs<'_>,
        began_in_recovery: bool,
        staged: Result<(), AirRecoveryReason>,
        ActiveAirOperation { op, plan }: &mut ActiveAirOperation,
        out: &mut StrategicDecision,
    ) -> OperationEnd {
        let obs = ev.obs;
        if let Err(reason) = staged {
            out.intents.clear();
            recover(op, reason, obs.tick);
        }
        let preparation_expired = plan.package().is_some_and(|package| {
            op.phase() <= AirOperationPhase::Assemble
                && obs.tick >= package.preparation_deadline
                && !assembly_complete(op, plan)
        });
        if preparation_expired {
            out.intents.clear();
            out.reserved_scrap = 0;
            recover(op, AirRecoveryReason::Timeout, obs.tick);
        }
        if !inputs.procurement.allow {
            out.intents
                .retain(|intent| !matches!(intent, Intent::TrainAt { .. }));
            out.reserved_scrap = 0;
        }
        let recovery_entered_this_tick = op.phase() == AirOperationPhase::Recover
            && (!began_in_recovery || op.phase_started_at == obs.tick);
        if op.phase() == AirOperationPhase::Recover {
            if recovery_entered_this_tick {
                self.cooldown_until = obs.tick.saturating_add(cooldown(ev.profile, ev.tuning));
            }
            reconcile_recovery_return(
                op,
                plan,
                RecoveryReturnContext {
                    obs,
                    home: ev.home,
                    public_map: ev.public_map,
                    orientation: ev.orientation,
                    issue_order: recovery_entered_this_tick,
                },
                out,
            );
        }
        out.reservations = reservations(op, plan, obs);
        // Lowering fans a mixed-domain group over distinct snapped goals, and
        // bounded-turn aircraft may stop within their movement acceptance
        // radius. Once a previously dispatched return has become terminal for
        // every survivor, `idle` is the simulation's authoritative completion
        // signal; comparing every tile to the original shared goal invents a
        // stricter geometry and holds the operation forever.
        let settled = began_in_recovery
            && !recovery_entered_this_tick
            && out
                .reservations
                .iter()
                .all(|id| unit(obs, *id).is_some_and(|member| member.idle));
        if settled {
            OperationEnd::Settled
        } else if op.phase() == AirOperationPhase::Recover
            && (out.reservations.is_empty() || elapsed(op.phase_started_at, obs.tick) >= 500)
        {
            OperationEnd::Released
        } else {
            OperationEnd::Continue
        }
    }
}

/// Whether the lifecycle may begin a fresh operation this observation.
pub(super) enum AirAdmission<'i> {
    /// Nothing may begin now; standby members stay reserved.
    Hold,
    /// No target justifies an operation, so standby members are released.
    NoTarget,
    Begin(FreshAirTarget<'i>),
}

/// How one lifecycle pass leaves the active operation.
enum OperationEnd {
    Continue,
    /// Recovery finished: every survivor was lost or released, or it ran out
    /// of time.
    Released,
    /// Every survivor of a dispatched return has come to rest.
    Settled,
}

/// Journal verdict for an operation that entered recovery for `reason`.
fn recovery_outcome(
    reason: AirRecoveryReason,
    objective_gone: bool,
    op: &AirOperation,
    journal: &crate::experience::OutcomeJournal,
    obs: &Observation,
) -> (Outcome, OutcomeReason, u16, bool) {
    match reason {
        AirRecoveryReason::Complete if objective_gone => (
            Outcome::Complete,
            OutcomeReason::ObjectiveObservedGone,
            750,
            false,
        ),
        AirRecoveryReason::RequiredUnitLost => (
            Outcome::Ineffective,
            OutcomeReason::RequiredUnitLost,
            1000,
            op.membership_frozen_at.is_some(),
        ),
        AirRecoveryReason::NewAirDefense => (
            Outcome::Aborted,
            OutcomeReason::ObservedCounter,
            1000,
            false,
        ),
        AirRecoveryReason::Timeout
            if op.membership_frozen_at.is_none()
                && !journal.has_progress()
                && journal.own_lost_value(obs) == 0 =>
        {
            (Outcome::Aborted, OutcomeReason::Deadline, 1000, false)
        }
        AirRecoveryReason::Timeout => (Outcome::Ineffective, OutcomeReason::Deadline, 750, false),
        AirRecoveryReason::Complete
        | AirRecoveryReason::ObjectiveLost
        | AirRecoveryReason::StaleIntelligence => {
            (Outcome::Inconclusive, OutcomeReason::LostContact, 0, false)
        }
        AirRecoveryReason::UnreachableStaging | AirRecoveryReason::UnreachableAirRoute => {
            (Outcome::Aborted, OutcomeReason::BlockedRoute, 1000, false)
        }
        AirRecoveryReason::PreparationInfeasible => {
            (Outcome::Invalidated, OutcomeReason::Preempted, 1000, false)
        }
    }
}

/// Planned drops of a Lift waiting on this operation's exact objective.
pub(super) fn landing_sites(
    lift_support: Option<&LiftSupportRequest>,
    op: &AirOperation,
) -> Vec<TilePos> {
    lift_support
        .filter(|request| request.player == op.target_player && request.target == op.target)
        .map_or_else(Vec::new, |request| request.planned_drops.clone())
}

fn planning_context<'c>(
    ev: AirEvidence<'c>,
    procurement: AirProcurement<'c>,
    op: &AirOperation,
    plan: &AirPlan,
    landing_sites: &'c [TilePos],
) -> AirPlanningContext<'c> {
    let connected_resources = plan
        .connected()
        .filter(|_| op.phase() <= AirOperationPhase::Assemble)
        .map(|connected| {
            ConnectedProductionResources::from_package_snapshot_after_current_reserve(
                ev.obs,
                connected.commitment.player,
                &connected.package,
                ev.route(connected.focus)
                    .excluding_paid(procurement.paid_exclusions),
                &ResourceSnapshot::from_observation(ev.obs),
                procurement.reserve.current,
            )
        });
    AirPlanningContext {
        ev,
        procurement,
        landing_sites,
        connected_resources,
    }
}

struct RecoveryReturnContext<'a> {
    obs: &'a Observation,
    home: TilePos,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
    issue_order: bool,
}

/// Same-observation inputs for one emergency economy recovery decision.
pub(crate) struct EconomyEmergencyRecovery<'a> {
    pub(crate) profile: &'a ResolvedProfile,
    pub(crate) tuning: DifficultyTuning,
    pub(crate) obs: &'a Observation,
    pub(crate) home: TilePos,
    pub(crate) public_map: Option<&'a PublicMapBriefing>,
    pub(crate) orientation: Orientation,
    /// Queue occurrences the reconnaissance program already holds. This path
    /// returns before shared allocation runs, so it receives them directly
    /// instead of reading an obligation view.
    pub(crate) recon_paid_exclusions: &'a [PaidQueueClaim],
}

fn reconcile_recovery_return(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: RecoveryReturnContext<'_>,
    out: &mut StrategicDecision,
) {
    let RecoveryReturnContext {
        obs,
        home,
        public_map,
        orientation,
        issue_order,
    } = context;
    let survivors = reservations(op, plan, obs);
    let returning = if matches!(plan, AirPlan::Connected(_)) {
        connected_public_map(plan, public_map).map_or_else(
            || {
                routing::routable_command_subset_with_orientation(
                    crate::query_work::QueryPurpose::AirOperation,
                    obs,
                    &survivors,
                    home,
                    orientation,
                )
            },
            |map| {
                routing::routable_command_subset_with_public_terrain_and_orientation(
                    crate::query_work::QueryPurpose::AirOperation,
                    obs,
                    map,
                    &survivors,
                    home,
                    orientation,
                )
            },
        )
    } else {
        routing::routable_command_subset(
            crate::query_work::QueryPurpose::AirOperation,
            obs,
            &survivors,
            home,
        )
    };
    release_unroutable(op, plan, &survivors, &returning);
    if issue_order && !returning.is_empty() {
        out.intents.push(Intent::MoveUnits {
            units: returning,
            goal: home,
        });
    }
}

fn air_operation_outcome(op: &AirOperation) -> AirOperationOutcome {
    if op.recovery_reason() == Some(AirRecoveryReason::Complete) {
        AirOperationOutcome::Released {
            player: op.target_player,
            target: op.target,
        }
    } else {
        AirOperationOutcome::Aborted {
            player: op.target_player,
            target: op.target,
        }
    }
}

pub(super) fn abort_if_needed(
    op: &mut AirOperation,
    plan: &AirPlan,
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
) {
    if let Some(reason) = operation_recovery_reason(op, plan, profile, obs, intel) {
        recover(op, reason, obs.tick);
    }
}

pub(super) fn operation_recovery_reason(
    op: &AirOperation,
    plan: &AirPlan,
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> Option<AirRecoveryReason> {
    if op.phase() <= AirOperationPhase::Assemble
        && op
            .scout_dispatch
            .is_some_and(|(scout, _)| unit(obs, scout).is_none())
    {
        return Some(AirRecoveryReason::RequiredUnitLost);
    }
    let waiting_for_recon_scout =
        op.phase() == AirOperationPhase::Recon && op.scout.is_none_or(|id| unit(obs, id).is_none());
    let connected_preparation =
        matches!(plan, AirPlan::Connected(_)) && op.phase() <= AirOperationPhase::Assemble;
    if elapsed(op.started_at, obs.tick) >= operation_timeout(profile, op, plan)
        || (!connected_preparation
            && !waiting_for_recon_scout
            && elapsed(op.phase_started_at, obs.tick)
                >= phase_timeout(op.phase(), op.started_at, plan))
    {
        return Some(AirRecoveryReason::Timeout);
    }
    if !plan.airborne()
        && op.phase() < AirOperationPhase::Strike
        && operation_objective_is_stale(op, plan, obs.tick, intel)
    {
        return Some(AirRecoveryReason::StaleIntelligence);
    }
    let lost_required_force = match plan {
        AirPlan::Connected(connected) => {
            let minimum = connected.commitment.minimum_capability;
            let live_suppression = op
                .artillery
                .iter()
                .filter_map(|id| unit(obs, *id))
                .map(|member| suppression_capability(member.kind, obs.faction))
                .fold(0_u64, u64::saturating_add);
            let live_strike = op
                .strike_aircraft
                .iter()
                .filter_map(|id| unit(obs, *id))
                .map(|member| strike_capability(member.kind, obs.faction))
                .fold(0_u64, u64::saturating_add);
            live_suppression < minimum.suppression || live_strike < minimum.strike
        }
        AirPlan::Island(island) => {
            let live_strike_aircraft = op
                .strike_aircraft
                .iter()
                .filter(|id| unit(obs, **id).is_some())
                .count();
            live_strike_aircraft < island.desired_strike_aircraft.div_ceil(2).max(1)
        }
        AirPlan::Reacquire(_) => false,
    };
    if op.membership_frozen_at.is_some()
        && (op.scout.is_some_and(|id| unit(obs, id).is_none()) || lost_required_force)
    {
        return Some(AirRecoveryReason::RequiredUnitLost);
    }
    if op.phase() < AirOperationPhase::Strike && operation_objective_cleared(op, plan, obs, intel) {
        return Some(AirRecoveryReason::ObjectiveLost);
    }
    None
}

fn release_unroutable(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    survivors: &[UnitId],
    returning: &[UnitId],
) {
    let keep = |id: &UnitId| !survivors.contains(id) || returning.contains(id);
    op.scout = op.scout.filter(keep);
    op.artillery.retain(keep);
    op.strike_aircraft.retain(keep);
    if let Some(screen) = plan.screen_mut() {
        screen.retain(keep);
    }
}

fn reusable_survivors(reason: Option<AirRecoveryReason>) -> bool {
    matches!(
        reason,
        Some(
            AirRecoveryReason::Complete
                | AirRecoveryReason::RequiredUnitLost
                | AirRecoveryReason::Timeout
        )
    )
}

pub(super) fn cooldown(profile: &ResolvedProfile, tuning: DifficultyTuning) -> Tick {
    let base: Tick = match profile.stance {
        BotStance::Turtle => 900,
        BotStance::Balanced => 700,
        BotStance::Aggressive => 500,
    };
    base + u64::from(100u8.saturating_sub(profile.traits.air)) * 3 + tuning.commitment_hesitation
}
