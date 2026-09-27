//! Typed decision inputs and one decision's exclusive turn over the air
//! planner.

use super::*;

/// Exact transport objective and landing envelope offered to the air planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiftSupportRequest {
    pub player: PlayerId,
    pub target: TilePos,
    pub planned_drops: Vec<TilePos>,
}

/// Same-observation evidence every air-planner call in one decision reads.
#[derive(Clone, Copy)]
pub(crate) struct AirEvidence<'a> {
    pub(crate) profile: &'a ResolvedProfile,
    pub(crate) tuning: DifficultyTuning,
    pub(crate) obs: &'a Observation,
    pub(crate) intel: &'a StrategicIntelligence,
    pub(crate) home: TilePos,
    pub(crate) public_map: Option<&'a PublicMapBriefing>,
    pub(crate) orientation: Orientation,
}

impl<'a> AirEvidence<'a> {
    pub(super) fn route(self, target: TilePos) -> ConnectedRouteContext<'a> {
        ConnectedRouteContext::new(
            self.intel,
            self.public_map,
            self.orientation,
            self.home,
            target,
        )
    }
}

/// Current scrap, and forecast scrap through the preparation window, that
/// earlier owners already hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CapitalReserve {
    pub(crate) current: u32,
    pub(crate) forecast: u32,
}

/// Allocation evidence for deriving one connected-operation proposal.
#[derive(Clone, Copy)]
pub(crate) struct ConnectedInputs<'a> {
    pub(crate) planning: &'a crate::planning::PlanningWork,
    pub(crate) resources: &'a ResourceSnapshot,
    /// Units other owners hold.
    pub(crate) unavailable: &'a [UnitId],
    /// Paid queue occurrences other programs own.
    pub(crate) paid_exclusions: &'a [PaidQueueClaim],
    pub(crate) reserve: CapitalReserve,
}

/// Producer work already accepted ahead of the air planner this decision.
#[derive(Clone, Copy)]
pub(crate) struct ProducerLanes<'a> {
    pub(crate) prior_intents: &'a [Intent],
    pub(crate) reservations: &'a ProducerLaneReservations,
}

impl ProducerLanes<'static> {
    pub(crate) fn empty() -> Self {
        Self {
            prior_intents: &[],
            reservations: ProducerLaneReservations::empty(),
        }
    }
}

/// Allocation evidence for recruiting and buying an air operation's members.
#[derive(Clone, Copy)]
pub(crate) struct AirProcurement<'a> {
    pub(crate) planning: &'a crate::planning::PlanningWork,
    /// Units other owners hold.
    pub(crate) unavailable: &'a [UnitId],
    /// Paid queue occurrences other programs own.
    pub(crate) paid_exclusions: &'a [PaidQueueClaim],
    pub(crate) reserve: CapitalReserve,
    pub(crate) lanes: ProducerLanes<'a>,
    /// Whether the operation may buy members. The lifecycle also begins a
    /// new operation only when it may.
    pub(crate) allow: bool,
}

/// Allocation verdicts the post-adjudication lifecycle acts on.
pub(crate) struct ThinkInputs<'a> {
    pub(crate) procurement: AirProcurement<'a>,
    /// Units another owner claimed after allocation; the operation waits
    /// rather than acting through them.
    pub(crate) claimed_elsewhere: &'a [UnitId],
    pub(crate) lift_support: Option<&'a LiftSupportRequest>,
    /// Excludes every unit the operation does not already own.
    pub(crate) owned_only: bool,
}

/// One decision's exclusive access to the air planner, bound to the evidence
/// it observed. Proposals read the planner; only the named transitions change
/// it, in the order allocation reaches them.
pub(crate) struct AirTurn<'a> {
    pub(super) planner: &'a mut StrategicPlanner,
    pub(super) ev: AirEvidence<'a>,
}

impl core::ops::Deref for AirTurn<'_> {
    type Target = StrategicPlanner;

    fn deref(&self) -> &StrategicPlanner {
        self.planner
    }
}

/// What the accepted allocation settles for the air planner.
pub(crate) enum AirAdjudication {
    Unchanged,
    /// Retained membership the allocation validated.
    Members(AirMembership),
    /// The accepted fresh proposal or active revision.
    Connected(Box<FreshConnectedProposal>),
}

/// The accepted connected proposal, otherwise the membership of the retained
/// connected or island operation the allocation imported.
pub(crate) fn air_adjudication(
    connected: Option<FreshConnectedProposal>,
    active_connected: Option<&ActiveConnectedObligation>,
    island: Option<&IslandPreparation>,
) -> AirAdjudication {
    if let Some(proposal) = connected {
        return AirAdjudication::Connected(Box::new(proposal));
    }
    active_connected
        .map(|active| &active.membership)
        .or_else(|| island.map(|island| &island.membership))
        .map_or(AirAdjudication::Unchanged, |membership| {
            AirAdjudication::Members(membership.clone())
        })
}

impl<'a> AirTurn<'a> {
    /// Lends this turn to a shorter-lived owner such as one allocation pass.
    pub(crate) fn reborrow(&mut self) -> AirTurn<'_> {
        AirTurn {
            planner: self.planner,
            ev: self.ev,
        }
    }

    /// Reconstructs unpaid demand from the retained package and current inventory.
    pub(crate) fn retained_obligation(
        &self,
        resources: &ResourceSnapshot,
        unavailable: &[UnitId],
        paid_exclusions: &[PaidQueueClaim],
    ) -> Option<ActiveConnectedObligation> {
        let active = self
            .air
            .as_ref()
            .filter(|active| active.op.assault_admitted())?;
        let connected = active.plan.connected()?;
        let package = &connected.package;
        let mut membership = AirMembership::from_active(active);
        let obs = self.ev.obs;
        let provider_jobs = if active.op.phase() <= AirOperationPhase::Assemble
            && obs.tick < package.preparation_deadline
            && operation_recovery_reason(
                &active.op,
                &active.plan,
                self.ev.profile,
                obs,
                self.ev.intel,
            )
            .is_none()
        {
            let route = self
                .ev
                .route(connected.focus)
                .excluding_paid(paid_exclusions);
            let resources =
                ConnectedProductionResources::from_package_snapshot_after_current_reserve(
                    obs,
                    connected.commitment.player,
                    package,
                    route,
                    resources,
                    0,
                );
            if active.op.phase() <= AirOperationPhase::Assemble
                && active.op.membership_frozen_at.is_none()
            {
                let owned = reservations(&active.op, &active.plan, obs);
                let unavailable = excluding_owned(unavailable, &owned);
                let mut unavailable =
                    connected_provider_unavailable(obs, &resources.targets, &unavailable, route);
                unavailable.retain(|id| !owned.contains(id));
                if membership.scout.is_none() && active.op.scout_dispatch.is_none() {
                    let mut scouts = Vec::new();
                    assign_provider_demands(&mut scouts, &package.recon, obs, &unavailable);
                    membership.scout = scouts.into_iter().next();
                }
                assign_artillery(&mut membership.artillery, &active.plan, obs, &unavailable);
                assign_strike_aircraft(
                    &mut membership.strike_aircraft,
                    &active.plan,
                    obs,
                    &unavailable,
                );
            }
            missing_package_demands(
                package,
                AirRoster::from(&membership),
                obs,
                &resources.snapshot,
                package.preparation_deadline,
                &resources.access,
            )
            .into_iter()
            .flat_map(|demand| {
                let job = ConnectedProviderJob {
                    kind: demand.kind,
                    enqueue_not_before: obs.tick,
                    ready_before: package.preparation_deadline,
                    eligible_producers: eligible_producers(
                        &resources.snapshot,
                        &resources.access,
                        demand.kind,
                        Some(package.preparation_deadline),
                    ),
                };
                std::iter::repeat_n(job, demand.count)
            })
            .collect()
        } else {
            Vec::new()
        };
        Some(ActiveConnectedObligation {
            identity: connected.commitment.key(),
            accepted_at: connected.commitment.admitted_at,
            deadline: package.preparation_deadline,
            units: membership.units(obs),
            membership,
            provider_jobs,
        })
    }

    pub(crate) fn island_preparation(
        &self,
        procurement: AirProcurement<'_>,
    ) -> Option<IslandPreparation> {
        let active = self.air.as_ref()?;
        let AirPlan::Island(island) = &active.plan else {
            return None;
        };
        if !active.op.assault_admitted() {
            return None;
        }
        let op = &active.op;
        let plan = &active.plan;
        let obs = self.ev.obs;
        let mut membership = AirMembership::from_active(active);
        let mut purchases = ProductionPlan::default();
        if op.phase() <= AirOperationPhase::Assemble {
            let unavailable =
                excluding_owned(procurement.unavailable, &reservations(op, plan, obs));
            let scout = Role::Scout.unit_for(obs.faction);
            membership.scout = retained_scout(membership.scout, obs, &unavailable);
            assign_artillery(&mut membership.artillery, plan, obs, &unavailable);
            assign_strike_aircraft(&mut membership.strike_aircraft, plan, obs, &unavailable);
            assign_exact(
                &mut membership.screen,
                island.desired_screen,
                obs,
                &unavailable,
                |kind| kind == Role::AirGround.unit_for(obs.faction),
            );
            let planning = AirPlanningContext {
                ev: self.ev,
                procurement: AirProcurement {
                    unavailable: &unavailable,
                    ..procurement
                },
                landing_sites: &[],
                connected_resources: None,
            };
            let demands = missing_island_members(
                AirRoster::from(&membership),
                membership.screen.len(),
                island,
                &planning,
                scout,
            );
            purchases = schedule(&planning, &demands);
        }
        Some(IslandPreparation {
            membership,
            purchases,
        })
    }

    /// Re-derives one admitted connected operation from current evidence while
    /// its membership remains revisable. The fixed preparation deadline and
    /// the committed identity and target set are retained; the package is
    /// sized against the committed members only.
    pub(crate) fn connected_revision(
        &self,
        inputs: ConnectedInputs<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let AirEvidence {
            profile,
            obs,
            intel,
            public_map,
            orientation,
            ..
        } = self.ev;
        if intel.observed_at() != Some(obs.tick) {
            return Ok(None);
        }
        let Some(active) = self.air.as_ref() else {
            return Ok(None);
        };
        let AirPlan::Connected(connected) = &active.plan else {
            return Ok(None);
        };
        let package = &connected.package;
        if !active.op.assault_admitted()
            || active.op.phase() > AirOperationPhase::Assemble
            || active.op.membership_frozen_at.is_some()
            || package.derived_at >= obs.tick
            || operation_recovery_reason(&active.op, &active.plan, profile, obs, intel).is_some()
            || obs.tick > connected.commitment.deadline
        {
            return Ok(None);
        }
        let Some(target) = best_current_member(&connected.commitment, connected.focus, intel)
        else {
            return Ok(None);
        };
        let owned = reservations(&active.op, &active.plan, obs);
        let unavailable = excluding_owned(inputs.unavailable, &owned);
        let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
        let context = FreshConnectedDerivationContext {
            ev: self.ev,
            inputs: ConnectedInputs {
                unavailable: &unavailable,
                ..inputs
            },
            minimum_only: false,
            campaign_routes: &campaign_routes,
            preferred_artillery: &active.op.artillery,
        };
        // A revision sizes the admitted members only; it never searches the
        // radius around its current primary again.
        let proposal = derive_connected_proposal_with_resources(
            context,
            target,
            connected.commitment.sized_members(intel, obs.tick),
            ConnectedProposalOrigin::Active {
                op: active.op.clone(),
                plan: connected.clone(),
            },
            connected.commitment.deadline,
        )
        .map_err(|reason| RejectedConnectedCandidate {
            target: target.clone(),
            reason,
        })?;
        // Tactics act on every live committed member, so a revision that could
        // not size one of them keeps the current package instead.
        let sized = connected.commitment.sized_members(intel, obs.tick);
        if !proposal.variants.iter().all(|variant| {
            sized
                .iter()
                .all(|member| variant.plan.package.target_anchors.contains(&member.anchor))
        }) {
            return Ok(None);
        }
        Ok(Some(proposal))
    }

    /// Proposes one exact common-minimum connected assault without mutating
    /// planner state. Island admission and an already-admitted assault remain
    /// on the ordinary lifecycle path.
    pub(crate) fn fresh_connected(
        &self,
        experience: &crate::experience::Experience,
        inputs: ConnectedInputs<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let AirEvidence {
            profile,
            tuning,
            obs,
            intel,
            home,
            public_map,
            orientation,
        } = self.ev;
        if intel.observed_at() != Some(obs.tick)
            || !strategic_admission_tick(obs.tick)
            || obs.tick < self.cooldown_until
        {
            return Ok(None);
        }

        if let Some(active) = &self.air {
            if active.op.assault_admitted() {
                return Ok(None);
            }
            let mut refreshed = active.clone();
            refresh_target(&mut refreshed.op, &refreshed.plan, intel);
            let Some(target) = current_target_contact(&refreshed.op, intel) else {
                return Ok(None);
            };
            if wealthy_island_target(profile, obs, home, target, public_map) {
                return Ok(None);
            }
            let owned = reservations(&refreshed.op, &refreshed.plan, obs);
            let unavailable = excluding_owned(inputs.unavailable, &owned);
            let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
            return derive_connected_proposal_with_resources(
                FreshConnectedDerivationContext {
                    ev: self.ev,
                    inputs: ConnectedInputs {
                        unavailable: &unavailable,
                        ..inputs
                    },
                    minimum_only: false,
                    campaign_routes: &campaign_routes,
                    preferred_artillery: &refreshed.op.artillery,
                },
                target,
                current_target_cluster(intel, target.player, target.anchor),
                ConnectedProposalOrigin::Remembered {
                    active: active.clone(),
                },
                obs.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
            )
            .map(Some)
            .map_err(|reason| RejectedConnectedCandidate {
                target: target.clone(),
                reason,
            });
        }

        if select_wealthy_island_target(profile, obs, home, intel, public_map).is_some() {
            return Ok(None);
        }
        let mut current = select_target_candidates(intel, obs.tick, tuning.tactical_memory)
            .into_iter()
            .filter(|target| target.evidence == ContactEvidence::Current)
            .collect::<Vec<_>>();
        current.sort_unstable_by_key(|target| {
            let context = crate::experience::ExperienceKey {
                doctrine: crate::experience::Doctrine::Air,
                x: target.anchor.x,
                y: target.anchor.y,
                subject: crate::experience::ExperienceSubject::Building(target.id),
            };
            let preference = (1024 + i32::from(experience.score(context)) / 2) as u64;
            (
                Reverse(u64::from(building_value(target.kind)) * preference),
                Reverse(target.confidence_at(obs.tick)),
                target.anchor.y,
                target.anchor.x,
                target.player,
                target.kind,
            )
        });
        let Some(first) = current.first().copied() else {
            return Ok(None);
        };
        let combat_roster = combat_roster(obs);
        if combat_roster < CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER {
            return Err(RejectedConnectedCandidate {
                target: first.clone(),
                reason: ConnectedPlanRejection::InsufficientStandingForce {
                    current: combat_roster,
                    required: CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER,
                },
            });
        }

        let mut standby = self.standby.clone();
        standby.prune(obs);
        let unavailable = excluding_owned(inputs.unavailable, &standby.reservations());
        let origin = ConnectedProposalOrigin::Idle {
            standby: self.standby.clone(),
        };
        let mut first_rejection = None;
        let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
        for target in current {
            match derive_connected_proposal_with_resources(
                FreshConnectedDerivationContext {
                    ev: self.ev,
                    inputs: ConnectedInputs {
                        unavailable: &unavailable,
                        ..inputs
                    },
                    minimum_only: false,
                    campaign_routes: &campaign_routes,
                    preferred_artillery: &standby.artillery,
                },
                target,
                current_target_cluster(intel, target.player, target.anchor),
                origin.clone(),
                obs.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
            ) {
                Ok(proposal) => return Ok(Some(proposal)),
                Err(reason) if first_rejection.is_none() => {
                    first_rejection = Some(RejectedConnectedCandidate {
                        target: target.clone(),
                        reason,
                    });
                }
                Err(_) => {}
            }
        }
        Err(first_rejection.expect("at least one current target was considered"))
    }

    /// Returns the exact remembered reconnaissance target that the ordinary
    /// post-allocation lifecycle will retain or admit on this observation.
    /// This preview is read-only so shared allocation can preserve capital
    /// needed by the immediately following Lift handoff.
    pub(crate) fn prospective_recon_target(
        &self,
        unavailable: &[UnitId],
        lift_support: Option<&LiftSupportRequest>,
    ) -> Option<&'a BuildingContact> {
        let AirEvidence {
            profile,
            obs,
            intel,
            public_map,
            ..
        } = self.ev;
        if intel.observed_at() != Some(obs.tick) {
            return None;
        }
        let ActiveAirOperation { mut op, plan } = match &self.air {
            Some(active) if unadmitted_recon(&active.op) => active.clone(),
            // Target selection is skipped when nothing could begin anyway.
            None if strategic_admission_tick(obs.tick) => {
                let AirAdmission::Begin(selected) = self.admission(self.ev, true, lift_support)
                else {
                    return None;
                };
                let mut standby = self.standby.clone();
                standby.prune(obs);
                fresh_air_operation(profile, obs, ProducerLanes::empty(), selected, standby)
            }
            Some(_) | None => return None,
        };
        refresh_target(&mut op, &plan, intel);
        let target = remembered_objective(&op, intel)?;
        if operation_recovery_reason(&op, &plan, profile, obs, intel).is_some() {
            return None;
        }
        let owned = reservations(&op, &plan, obs);
        op.scout = remembered_recon_scout(&op, obs, &excluding_owned(unavailable, &owned));
        reachable_scout_goal(
            &op,
            &plan,
            obs,
            intel,
            &landing_sites(lift_support, &op),
            connected_public_map(&plan, public_map),
        )
        .map(|_| target)
    }

    /// Moves the admitted connected operation into bounded recovery as soon as
    /// preparation proves it cannot continue, so every later read in this
    /// allocation sees the recovery phase. The post-allocation think owns the
    /// one return-home order.
    pub(crate) fn recover_connected(&mut self, reason: AirRecoveryReason) {
        let active = self
            .planner
            .air
            .as_mut()
            .expect("an active connected obligation can only come from its planner");
        debug_assert!(active.op.assault_admitted());
        debug_assert!(matches!(active.plan, AirPlan::Connected(_)));
        recover(&mut active.op, reason, self.ev.obs.tick);
    }

    /// Installs the accepted allocation's verdict. Only purchases emitted now
    /// cross from forecast evidence into the connected ledger.
    pub(crate) fn apply(
        &mut self,
        verdict: AirAdjudication,
        schedule: &[crate::allocation::ScheduledProducerJob],
    ) {
        let now = self.ev.obs.tick;
        match verdict {
            AirAdjudication::Unchanged => {}
            AirAdjudication::Members(membership) => self.planner.apply_membership(membership, now),
            AirAdjudication::Connected(proposal) => self.planner.commit_connected(*proposal),
        }
        self.planner.record_connected_purchases(schedule, now);
    }

    /// Runs the ordinary tactical lifecycle after the coordinator has already
    /// accepted or rejected the fresh connected-offense proposal for this
    /// observation. Island and remembered reconnaissance behavior is unchanged.
    pub(crate) fn think(&mut self, inputs: ThinkInputs<'_>) -> StrategicDecision {
        self.planner.think(self.ev, inputs)
    }
}
