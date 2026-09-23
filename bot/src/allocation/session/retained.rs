//! Reconcile retained ownership and funding before fresh portfolio selection.

use super::*;

pub(super) struct RetainedWork<'s, 'a> {
    context: &'s AllocationSessionContext<'a>,
    participants: &'s mut AllocationParticipants<'a>,
    advanced: &'s mut AdvancedPlannerWork,
}

pub(super) struct RetainedPreparation {
    pub(super) claims: ClaimSnapshot,
    pub(super) obligations: ObligationPreparation,
    pub(super) saved: SavedFoundryPreparation,
    pub(super) air_lift: AirLiftPreparation,
    pub(super) active_revision: ActiveRevisionPreparation,
    pub(super) prospective_carrier_floor: u32,
    pub(super) emergency_defense: Option<FreshEmergencyDefense>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetainedStep {
    Foundry,
    Island,
    Lift,
    StandingArmy,
}

struct RetainedOrder([RetainedStep; 4]);

impl RetainedOrder {
    fn new(
        island_before_lift: bool,
        island_before_foundry: bool,
        lift_before_foundry: bool,
    ) -> Self {
        use RetainedStep::*;
        let steps = match (
            island_before_lift,
            island_before_foundry,
            lift_before_foundry,
        ) {
            (true, false, _) => [Foundry, Island, Lift, StandingArmy],
            (true, true, false) => [Island, Foundry, Lift, StandingArmy],
            (true, true, true) => [Island, Lift, StandingArmy, Foundry],
            (false, _, false) => [Foundry, Lift, Island, StandingArmy],
            (false, false, true) => [Lift, Foundry, Island, StandingArmy],
            (false, true, true) => [Lift, Island, StandingArmy, Foundry],
        };
        Self(steps)
    }

    fn steps(self) -> [RetainedStep; 4] {
        self.0
    }
}

enum RetainedHorizon<'a> {
    Lift,
    Connected {
        deadline: Tick,
        lift_deadline: Tick,
    },
    Revision {
        fresh: &'a FreshInvestmentPreparation,
        lift_deadline: Tick,
    },
}

impl<'s, 'a> RetainedWork<'s, 'a> {
    pub(super) fn new(
        context: &'s AllocationSessionContext<'a>,
        participants: &'s mut AllocationParticipants<'a>,
        advanced: &'s mut AdvancedPlannerWork,
    ) -> Self {
        Self {
            context,
            participants,
            advanced,
        }
    }

    pub(super) fn prepare(
        &mut self,
        resources: ResourceSnapshot,
        recon_paid_exclusions: &mut Vec<(oxide_sim::ids::BuildingId, UnitKind, usize)>,
        support_snapshot: &SupportWorkSnapshot,
    ) -> RetainedPreparation {
        self.participants.raids.reconcile_procurement_routes(
            self.context.observation,
            Some(self.context.public_map),
            Some(self.context.orientation),
        );
        recon_paid_exclusions.extend(
            self.participants
                .raids
                .paid_claims()
                .iter()
                .map(|claim| (claim.producer, claim.kind, claim.occurrence)),
        );
        recon_paid_exclusions.sort_unstable();
        recon_paid_exclusions.dedup();

        let mut claims = snapshot_claims(self.context, self.participants);
        let mut obligations = self.collect_legacy_obligations(&claims, resources);
        self.prepare_standing_saving(&claims, &mut obligations);
        if obligations.invalid_active_connected {
            self.participants
                .strategy
                .recover_unfundable_active_connected(self.context.observation.tick);
            obligations.active_connected = None;
        }
        if obligations.invalid_active_lift {
            self.participants
                .lifts
                .recover_invalid_production(self.context.observation.tick);
            obligations.active_lift = None;
        }
        let emergency_defense = self.prepare_emergency_defense(&claims, &mut obligations);
        if let Some(plan) = self.participants.policy.economic_foundation() {
            let guard = self.participants.policy.shallow_sentinel_capital_reserve(
                self.context.dials,
                self.context.observation,
                self.context.home,
                self.context.public_map,
                &[],
            );
            let available = residual_current_after_obligations(
                &obligations.resources,
                &obligations.obligations,
                obligation_horizon(&obligations.obligations, plan.deadline),
                self.context.dials.cadence,
                &self.participants.policy.planning,
            )
            .unwrap_or(0);
            if guard > 0 {
                obligations.obligations.push(super::imported_obligation(
                    ObligationClass::PersistentPlan,
                    plan.observed_at,
                    ObligationKey::SavedEconomy(plan.key),
                    ClaimBundle::new(
                        guard.min(available),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    )
                    .expect("a current-only construction escrow has no conflicting members"),
                ));
            }
        }
        let mut air_lift = self.prepare_air_commitments(&claims, &mut obligations);
        if let Some(saving) = self.participants.policy.economic_saving() {
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                economic_investment_claims(saving)
                    .map(|claims| {
                        obligations.obligations.push(super::imported_obligation(
                            ObligationClass::PersistentPlan,
                            saving.observed_at,
                            ObligationKey::SavedEconomy(saving.key),
                            claims,
                        ))
                    })
                    .map_err(Into::into),
            );
        }
        let island_admitted_at = self
            .participants
            .strategy
            .air_operation()
            .filter(|operation| operation.assault_admitted())
            .and_then(|_| self.participants.strategy.air_admitted_at());
        let island_precedes_foundry = island_admitted_at.is_some_and(|accepted_at| {
            self.participants
                .policy
                .operation_precedes_foundry_saving(accepted_at)
        });
        let island_precedes_lift = island_admitted_at.is_some_and(|accepted_at| {
            !self.advanced.lift_was_active || accepted_at <= self.advanced.lift_started_at
        });
        let order = RetainedOrder::new(
            island_precedes_lift,
            island_precedes_foundry,
            air_lift.active_lift_precedes_foundry,
        );
        let mut saved = None;
        let mut earlier_producer_intents = Vec::new();
        for step in order.steps() {
            match step {
                RetainedStep::Foundry => {
                    saved = Some(self.prepare_saved_foundry(&claims, &air_lift, &mut obligations));
                }
                RetainedStep::Island => {
                    self.stage_active_island(&mut claims, &mut obligations, recon_paid_exclusions);
                    if island_precedes_lift
                        && let Some(staged) = obligations.staged_strategy.as_ref()
                    {
                        earlier_producer_intents.extend(staged.decision.intents.iter().cloned());
                        self.advanced
                            .lift_unavailable
                            .extend(staged.decision.reservations.iter().copied());
                        self.advanced.lift_unavailable.sort_unstable();
                        self.advanced.lift_unavailable.dedup();
                        self.advanced.initial_lift_support = lift_air_support(
                            self.participants.strategy.air_operation(),
                            self.participants.strategy.terminal_outcome(),
                        );
                    }
                }
                RetainedStep::Lift => {
                    self.advance_active_lift(
                        &mut obligations,
                        &mut air_lift,
                        &earlier_producer_intents,
                    );
                    if !island_precedes_lift {
                        earlier_producer_intents
                            .extend(air_lift.lift_decision.intents.iter().cloned());
                        self.refresh_planner_claims(&mut claims);
                    }
                }
                RetainedStep::StandingArmy => {
                    self.refresh_planner_claims(&mut claims);
                    self.prepare_standing_army(&claims, &mut obligations);
                }
            }
        }
        let mut saved = saved.expect("retained preparation visits the Foundry exactly once");
        {
            // Active revision proposals carry the exact planner snapshot they
            // revise, so settle completed queue ownership before deriving one.
            let _ = self
                .participants
                .strategy
                .paid_connected_production(self.context.observation);
        }
        let active_revision = self.prepare_active_connected_revision(
            &claims,
            &mut obligations,
            recon_paid_exclusions,
        );
        if active_revision.proposal.is_none() {
            self.downgrade_unfundable_active_connected(&mut saved, &air_lift, &mut obligations);
        }
        let prospective_carrier_floor =
            self.prospective_carrier_floor(&claims, obligations.coordinator_failure.is_none());
        self.reconcile_unfundable_reconnaissance(&claims, &mut obligations);
        self.reconcile_standing_saving(&mut obligations);
        self.renew_support(&claims, &mut obligations, support_snapshot);
        RetainedPreparation {
            claims,
            obligations,
            saved,
            air_lift,
            active_revision,
            prospective_carrier_floor,
            emergency_defense,
        }
    }

    fn renew_support(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
        support_snapshot: &SupportWorkSnapshot,
    ) {
        let available = residual_current_after_obligations(
            &obligations.resources,
            &obligations.obligations,
            obligation_horizon(
                &obligations.obligations,
                self.context
                    .observation
                    .tick
                    .saturating_add(self.context.dials.cadence),
            ),
            self.context.dials.cadence,
            &self.participants.policy.planning,
        )
        .unwrap_or(0)
        .saturating_sub(self.participants.policy.shallow_sentinel_capital_reserve(
            self.context.dials,
            self.context.observation,
            self.context.home,
            self.context.public_map,
            &[],
        ));
        let context = support_context(self.context, claims, &obligations.resources);
        let allow_repair = claims.opening_core.ready;
        let renewal_unavailable: Vec<_> = claims
            .planner_claims
            .iter()
            .copied()
            .filter(|unit| {
                !self
                    .participants
                    .policy
                    .state
                    .support_work
                    .repairs
                    .iter()
                    .any(|repair| repair.key.worker == *unit)
            })
            .collect();
        for repair in self.participants.policy.renew_prepared_repairs(
            EconomicInvestmentContext {
                unavailable: &renewal_unavailable,
                ..context
            },
            support_snapshot,
            available,
            allow_repair,
        ) {
            let observed_builder =
                self.context.observation.my_units.iter().any(|unit| {
                    unit.id == repair.key.worker && unit.kind.stats().harvest.is_some()
                });
            obligations.obligations.push(imported_obligation(
                ObligationClass::PersistentPlan,
                repair.accepted_at,
                ObligationKey::Support(repair.key),
                repair.claims(observed_builder),
            ));
        }
    }

    fn collect_legacy_obligations(
        &mut self,
        claims: &ClaimSnapshot,
        resources: ResourceSnapshot,
    ) -> ObligationPreparation {
        let air_work = economic_air_work(
            self.context,
            self.participants,
            &self.advanced.lift_unavailable,
        );
        let mut obligations = Vec::new();
        let planner = &*self.participants.raids;
        if !planner.paid_claims().is_empty() {
            obligations.push(imported_obligation(
                ObligationClass::PersistentPlan,
                planner
                    .preparation_started_at()
                    .unwrap_or(self.context.observation.tick),
                ObligationKey::Legacy {
                    channel: LegacyChannel::Raid,
                    sequence: 2,
                },
                ClaimBundle::default().with_paid_queue(planner.paid_claims().to_vec()),
            ));
        }
        for deployment in &self.participants.policy.state.support_deployments.active {
            obligations.push(imported_obligation(
                ObligationClass::PersistentPlan,
                deployment.accepted_at,
                ObligationKey::SupportDeployment(deployment.key),
                deployment.claims(),
            ));
        }
        for (key, work) in &self.participants.policy.state.reconnaissance.assignments {
            if work.unit.is_some() || work.paid_claim.is_some() || work.unpaid {
                obligations.push(imported_obligation(
                    ObligationClass::PersistentPlan,
                    work.proposal.observed_at,
                    ObligationKey::Reconnaissance(*key),
                    work.retained_claims(self.context.observation.tick),
                ));
            }
        }
        let mut coordinator_failure = None;
        let mut observed_capital = self.context.observation.scrap;
        if self.participants.policy.economic_saving().is_some()
            || self.participants.policy.has_economic_foundation()
        {
            let demand = if self.participants.policy.economic_saving().is_some() {
                let targets = standing_force_projection_targets(self.context, self.participants);
                derive_standing_force_with_demand(
                    self.context.observation,
                    self.context.intelligence,
                    self.context.profile,
                    self.context.tuning,
                    &resources,
                    StandingForceContext::new(&claims.strategic_core_exclusions, &[])
                        .with_ground_routing(
                            StandingGroundTarget::footprint(
                                self.context.home,
                                BuildingKind::Foundry.base_stats().size,
                            ),
                            Some(self.context.public_map),
                            &targets,
                            Some(self.context.orientation),
                        ),
                )
                .1
            } else {
                Vec::new()
            };
            self.participants.policy.refresh_economic_saving(
                EconomicInvestmentContext {
                    evidence: self.context.evidence,
                    obligations: &[],
                    obs: self.context.observation,
                    resources: &resources,
                    profile: self.context.profile,
                    briefing: self.context.public_map,
                    orientation: self.context.orientation,
                    unavailable: &claims.planner_claims,
                    demands: &demand,
                    cadence: self.context.dials.cadence,
                    unit_contacts: self.context.intelligence.units(),
                    building_contacts: self.context.intelligence.buildings(),
                    protected_scrap: 0,
                    air_work: &air_work,
                },
                claims.opening_core.ready,
            );
        }
        match observed_builder_obligations(
            &resources,
            self.context.observation,
            &mut observed_capital,
            self.context
                .observation
                .tick
                .saturating_add(connected_preparation_horizon()),
            self.context.dials.cadence,
        ) {
            Ok(mut observed) => obligations.append(&mut observed),
            Err(error) => retain_first_coordinator_failure(
                &mut coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                Err((&error).into()),
            ),
        }

        retain_first_coordinator_failure(
            &mut coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations,
                &resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at: self.context.observation.tick,
                    decision_at: self.context.observation.tick,
                    retained_at: self.advanced.team_started_at,
                    channel: LegacyChannel::TeamRelief,
                    decision: &self.advanced.team_decision,
                    protect_unspent_current_scrap: true,
                    prior_producer_intents: &[],
                    retained_units: self.participants.team.core_reservations(),
                    production_deadline: connected_preparation_horizon()
                        .saturating_add(self.context.observation.tick),
                },
            ),
        );
        retain_first_coordinator_failure(
            &mut coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations,
                &resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at: self.context.observation.tick,
                    decision_at: self.context.observation.tick,
                    retained_at: self.advanced.raid_started_at,
                    channel: LegacyChannel::Raid,
                    decision: &self.advanced.raid_decision,
                    protect_unspent_current_scrap: true,
                    prior_producer_intents: &self.advanced.team_decision.intents,
                    retained_units: self.participants.raids.reservations().to_vec(),
                    production_deadline: connected_preparation_horizon()
                        .saturating_add(self.context.observation.tick),
                },
            ),
        );

        let mut active_connected = self.participants.strategy.active_connected_obligation(
            FreshConnectedProposalRequest::new(
                self.context.profile,
                self.context.tuning,
                self.context.observation,
                &resources,
                self.context.intelligence,
                self.context.home,
                StrategicCoordination {
                    planning: Some(&self.participants.policy.planning),
                    enlisted: &claims.planner_claims,
                    lift_support: None,
                    allow_new_operation: false,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: Some(self.context.public_map),
                    orientation: self.context.orientation,
                },
            )
            .with_paid_exclusions(
                &self
                    .participants
                    .policy
                    .state
                    .reconnaissance
                    .paid_exclusions(),
            ),
        );
        let mut active_lift = self.participants.lifts.active_production_obligation();
        let mut invalid_active_connected = false;
        let mut invalid_active_lift = false;
        let mut connected_import = match active_connected
            .as_ref()
            .map(active_connected_obligation)
            .transpose()
        {
            Ok(obligation) => obligation,
            Err(error) => {
                invalid_active_connected = true;
                retain_first_coordinator_failure(
                    &mut coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    Err(error.into()),
                );
                None
            }
        };
        let mut lift_import = match active_lift
            .as_ref()
            .map(active_lift_production_obligation)
            .transpose()
        {
            Ok(obligation) => obligation,
            Err(error) => {
                invalid_active_lift = true;
                retain_first_coordinator_failure(
                    &mut coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    Err(error.into()),
                );
                None
            }
        };

        if invalid_active_connected {
            active_connected = None;
            connected_import = None;
        }
        if invalid_active_lift {
            active_lift = None;
            lift_import = None;
        }

        let legacy_air_claims = if active_connected.is_some() {
            obligations.push(
                connected_import
                    .take()
                    .expect("a retained connected operation has an adapted obligation"),
            );
            None
        } else {
            self.participants.strategy.air_operation().map(|operation| {
                (
                    self.participants
                        .strategy
                        .air_admitted_at()
                        .unwrap_or(self.context.observation.tick),
                    prior_planner_claims(&[], Some(operation), &[], &[], None),
                )
            })
        };

        if active_lift.is_some() {
            obligations.push(
                lift_import
                    .take()
                    .expect("a retained Lift operation has an adapted obligation"),
            );
        }

        ObligationPreparation {
            resources,
            obligations,
            coordinator_failure,
            active_connected,
            active_lift,
            invalid_active_connected,
            invalid_active_lift,
            legacy_air_claims,
            staged_strategy: None,
        }
    }

    fn prepare_standing_saving(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) {
        let Some(saving) = self.participants.policy.state.standing_saving.as_ref() else {
            return;
        };
        if !claims.opening_core.ready {
            self.participants.policy.state.standing_saving = None;
            return;
        }
        let targets = standing_force_projection_targets(self.context, self.participants);
        let (_, demands) = derive_standing_force_with_demand(
            self.context.observation,
            self.context.intelligence,
            self.context.profile,
            self.context.tuning,
            &obligations.resources,
            StandingForceContext::new(&claims.strategic_core_exclusions, &[]).with_ground_routing(
                StandingGroundTarget::footprint(
                    self.context.home,
                    BuildingKind::Foundry.base_stats().size,
                ),
                Some(self.context.public_map),
                &targets,
                Some(self.context.orientation),
            ),
        );
        if !saving.still_useful(
            self.context.observation,
            &demands,
            self.context.public_map,
            self.context.orientation,
        ) {
            self.participants.policy.state.standing_saving = None;
            return;
        }
        obligations.obligations.push(saving.obligation());
        self.reconcile_standing_saving(obligations);
    }

    fn reconcile_standing_saving(&mut self, obligations: &mut ObligationPreparation) {
        let Some(saving) = self.participants.policy.state.standing_saving.as_ref() else {
            return;
        };
        let deadline = obligation_horizon(&obligations.obligations, saving.job.ready_before);
        let feasible = CrossDomainAllocation::new(
            &obligations.resources,
            deadline,
            self.context.dials.cadence,
        )
        .ok()
        .is_some_and(|mut allocation| {
            for obligation in obligations.obligations.iter().cloned() {
                allocation.import(obligation);
            }
            matches!(
                allocation.resolve_planned(
                    AllocationPersonality::default(),
                    None,
                    &self.participants.policy.planning
                ),
                Ok(_) | Err(AllocationError::Deferred)
            )
        });
        if !feasible {
            let key = saving.proposal.key();
            obligations.obligations.retain(|obligation| !matches!(obligation.owner(),
                ClaimOwner::Obligation { key: ObligationKey::StandingForceSaving(found), .. } if found == key));
            self.participants.policy.state.standing_saving = None;
        }
    }

    fn prepare_emergency_defense(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) -> Option<FreshEmergencyDefense> {
        if claims.opening_core.ready || obligations.coordinator_failure.is_some() {
            return None;
        }
        let available_builders =
            available_allocation_builders(&obligations.resources, &obligations.obligations);
        let current_scrap = self
            .context
            .observation
            .scrap
            .saturating_sub(current_reserve_at(
                &obligations.obligations,
                self.context.observation.tick,
            ));
        let defense = self.participants.policy.fresh_emergency_defense(
            self.context.dials,
            self.context.observation,
            FreshEmergencyDefenseContext {
                home: self.context.home,
                available_builders: &available_builders,
                unit_contacts: self.context.intelligence.units(),
                building_contacts: self.context.intelligence.buildings(),
                public_map: self.context.public_map,
                same_think_intents: &self.advanced.team_decision.intents,
                current_scrap,
            },
        )?;
        let imported = push_obligation(
            &mut obligations.obligations,
            fresh_emergency_defense_obligation(self.context.observation.tick, defense),
        );
        let accepted = imported.is_ok();
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            imported,
        );
        accepted.then_some(defense)
    }

    fn prepare_air_commitments(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) -> AirLiftPreparation {
        let mut opening_bootstrap = 0;
        if !claims.opening_core.ready {
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_clamped_current_reserve(
                    &mut obligations.obligations,
                    self.context.observation.scrap,
                    self.context.observation.tick,
                    self.context.observation.tick,
                    ObligationKey::OpeningCore { sequence: 0 },
                    claims.opening_core.missing_scrap,
                ),
            );
        } else {
            opening_bootstrap = self
                .participants
                .policy
                .strategic_opening_bootstrap_reserve(
                    self.context.dials,
                    self.context.observation,
                    self.context.home,
                    self.context.public_map,
                );
            for (sequence, amount) in [(1, opening_bootstrap)] {
                if amount > 0 {
                    retain_first_coordinator_failure(
                        &mut obligations.coordinator_failure,
                        AllocationCoordinatorStageTrace::ObligationCollection,
                        push_clamped_current_reserve(
                            &mut obligations.obligations,
                            self.context.observation.scrap,
                            self.context.observation.tick,
                            self.context.observation.tick,
                            ObligationKey::OpeningCore { sequence },
                            amount,
                        ),
                    );
                }
            }
        }
        let voluntary_scrap_guard = if claims.opening_core.ready {
            self.participants.policy.shallow_sentinel_capital_reserve(
                self.context.dials,
                self.context.observation,
                self.context.home,
                self.context.public_map,
                &self.advanced.team_decision.intents,
            )
        } else {
            0
        };

        let active_lift_precedes_foundry = self.advanced.lift_was_active
            && self
                .participants
                .policy
                .operation_precedes_foundry_saving(self.advanced.lift_started_at);
        let lift_deadline = self.participants.lifts.operation().map_or_else(
            || {
                self.context
                    .observation
                    .tick
                    .saturating_add(connected_preparation_horizon())
            },
            |operation| operation.deadline,
        );

        let saved_plan_reserve_already_imported = if claims.opening_core.ready {
            opening_bootstrap
        } else {
            claims
                .opening_core
                .missing_scrap
                .min(self.context.observation.scrap)
        };
        AirLiftPreparation {
            lift_decision: StrategicDecision::default(),
            opening_bootstrap,
            active_lift_precedes_foundry,
            active_lift_spendable: 0,
            saved_plan_reserve_already_imported,
            lift_deadline,
            fresh_lift_producer_jobs: 0,
            voluntary_scrap_guard,
        }
    }

    fn prospective_carrier_floor(&self, claims: &ClaimSnapshot, allocation_possible: bool) -> u32 {
        if !allocation_possible || !claims.opening_core.ready {
            return 0;
        }
        let Some(target) =
            self.participants
                .strategy
                .prospective_recon_target(StrategicThinkContext::new(
                    self.context.profile,
                    self.context.tuning,
                    self.context.observation,
                    self.context.intelligence,
                    self.context.home,
                    StrategicCoordination {
                        planning: Some(&self.participants.policy.planning),
                        enlisted: &claims.planner_claims,
                        lift_support: self.context.lift_support,
                        allow_new_operation: true,
                        protected_current_scrap: 0,
                        protected_forecast_scrap: 0,
                        public_map: Some(self.context.public_map),
                        orientation: self.context.orientation,
                    },
                ))
        else {
            return 0;
        };
        {
            self.participants
                .lifts
                .prospective_first_carrier_commitment(
                    self.context.observation,
                    self.context.home,
                    &self.advanced.lift_unavailable,
                    &claims.strategic_core_exclusions,
                    u64::from(self.context.dials.minimum_core_equivalents),
                    target,
                )
        }
    }

    pub(super) fn advance_active_lift(
        &mut self,
        obligations: &mut ObligationPreparation,
        air_lift: &mut AirLiftPreparation,
        prior_producer_intents: &[Intent],
    ) {
        if !self.advanced.lift_was_active {
            return;
        }
        let older_saved_foundry_capital = older_saved_foundry_deferrable_capital(
            &obligations.obligations,
            air_lift.active_lift_precedes_foundry,
        );
        air_lift.active_lift_spendable = self
            .context
            .observation
            .scrap
            .saturating_sub(current_reserve_at(
                &obligations.obligations,
                self.context.observation.tick,
            ))
            .saturating_sub(older_saved_foundry_capital);
        let projected_observation =
            project_producer_intents(self.context.observation, prior_producer_intents);
        let mut preceding_producer_intents = prior_producer_intents.to_vec();
        let retained_production = lift_preceding_production_context(
            &obligations.resources,
            obligations.active_lift.as_ref(),
            obligations.active_connected.as_ref(),
            self.context.dials.cadence,
            self.context.observation.tick,
            &self.participants.policy.planning,
        );
        let Some((producer_lane_reservations, due_intents)) = retained_production else {
            // Shared adjudication distinguishes deferred search from an invalid
            // retained owner and applies recovery without emitting speculative work.
            return;
        };
        preceding_producer_intents.extend(due_intents);
        air_lift.lift_decision = self
            .participants
            .lifts
            .think_with_admission_and_producer_lanes(
                &projected_observation,
                self.context.home,
                &self.advanced.lift_unavailable,
                self.advanced.initial_lift_support,
                LiftAdmission {
                    allow_new_commitments: self.advanced.preliminary_core.ready,
                    spendable_scrap: air_lift.active_lift_spendable,
                    core_reservations: &self.advanced.preliminary_core_exclusions,
                    minimum_core_equivalents: u64::from(
                        self.context.dials.minimum_core_equivalents,
                    ),
                },
                &producer_lane_reservations,
            );
        let retained_lift_units = self
            .participants
            .lifts
            .operation()
            .map_or_else(Vec::new, |operation| {
                observable_lift_operation_reservations(operation, self.context.observation)
            });
        match feasible_active_lift_current_production_prefix(
            ActiveLiftCurrentProductionContext {
                resources: &obligations.resources,
                cadence: self.context.dials.cadence,
                decision_tick: self.context.observation.tick,
                retained_at: self.advanced.lift_started_at,
                decision: &air_lift.lift_decision,
                prior_producer_intents: &preceding_producer_intents,
                production_deadline: air_lift.lift_deadline,
            },
            &obligations.obligations,
            &self.participants.policy.planning,
        ) {
            Ok(decision) => air_lift.lift_decision = decision,
            Err(error) => retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                Err(error),
            ),
        }
        let future_context = self.participants.lifts.operation().map(|operation| {
            ActiveLiftFutureProductionContext {
                resources: &obligations.resources,
                observation: self.context.observation,
                operation,
                unavailable: &self.advanced.lift_unavailable,
                prior_producer_intents: &preceding_producer_intents,
                lift_decision: &air_lift.lift_decision,
                cadence: self.context.dials.cadence,
                accepted_at: self.advanced.lift_started_at,
            }
        });
        // The operation's desired carrier count remains a tactical target, not
        // debt the economy has already incurred. Preserve only the unpaid
        // prefix that the shared allocator can actually fund and schedule
        // beside every older obligation through the immutable Lift deadline.
        let mut feasibility_obligations = obligations.obligations.clone();
        let provisional_current = push_legacy_planner_claim(
            &mut feasibility_obligations,
            &obligations.resources,
            LegacyPlannerClaim {
                cadence: self.context.dials.cadence,
                accepted_at: self.context.observation.tick,
                decision_at: self.context.observation.tick,
                retained_at: self.advanced.lift_started_at,
                channel: LegacyChannel::Lift,
                decision: &air_lift.lift_decision,
                protect_unspent_current_scrap: false,
                prior_producer_intents: &preceding_producer_intents,
                retained_units: retained_lift_units.clone(),
                production_deadline: air_lift.lift_deadline,
            },
        );
        let future_lift_obligation =
            if provisional_current.is_ok() && obligations.active_lift.is_none() {
                future_context.map_or(Ok(None), |context| {
                    feasible_active_lift_future_production_obligation(
                        context,
                        &feasibility_obligations,
                        &self.participants.policy.planning,
                    )
                })
            } else {
                Ok(None)
            };
        let future_lift_claimed = future_lift_obligation
            .as_ref()
            .is_ok_and(|obligation| obligation.is_some());
        air_lift.fresh_lift_producer_jobs = future_lift_obligation
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map_or(0, |obligation| obligation.claims.producer_jobs().len());
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations.obligations,
                &obligations.resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at: self.context.observation.tick,
                    decision_at: self.context.observation.tick,
                    retained_at: self.advanced.lift_started_at,
                    channel: LegacyChannel::Lift,
                    decision: &air_lift.lift_decision,
                    protect_unspent_current_scrap: !future_lift_claimed,
                    prior_producer_intents: &preceding_producer_intents,
                    retained_units: retained_lift_units,
                    production_deadline: self.participants.lifts.operation().map_or_else(
                        || {
                            connected_preparation_horizon()
                                .saturating_add(self.context.observation.tick)
                        },
                        |operation| operation.deadline,
                    ),
                },
            ),
        );
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            match future_lift_obligation {
                Ok(Some(obligation)) => {
                    obligations.obligations.push(obligation);
                    Ok(())
                }
                Ok(None) => Ok(()),
                Err(error) => Err((&error).into()),
            },
        );
    }

    fn prepare_saved_foundry(
        &mut self,
        claims: &ClaimSnapshot,
        air_lift: &AirLiftPreparation,
        obligations: &mut ObligationPreparation,
    ) -> SavedFoundryPreparation {
        let current_before_saved = self
            .context
            .observation
            .scrap
            .saturating_sub(current_reserve_at(
                &obligations.obligations,
                self.context.observation.tick,
            ))
            .saturating_add(air_lift.saved_plan_reserve_already_imported)
            .min(self.context.observation.scrap);
        let mut obligation = self.participants.policy.validated_foundry_obligation(
            self.context.observation,
            &obligations.resources,
            claims.opening_core.ready,
            current_before_saved,
        );
        let mut preparation_need = None;
        if let Some(saved) = obligation.filter(|saved| !saved.blocked()) {
            let available_builders =
                available_allocation_builders(&obligations.resources, &obligations.obligations);
            match self.participants.policy.saved_foundry_readiness(
                self.context.dials,
                self.context.observation,
                saved,
                FreshFoundryProposalContext {
                    home: self.context.home,
                    available_builders: &available_builders,
                    combat_core_exclusions: &claims.strategic_core_exclusions,
                    unit_contacts: self.context.intelligence.units(),
                    building_contacts: self.context.intelligence.buildings(),
                    public_map: self.context.public_map,
                    same_think_intents: &self.advanced.team_decision.intents,
                    current_scrap: saved.planning_scrap(),
                    protected_reserve: saved.protected_reserve(),
                },
            ) {
                SavedFoundryReadiness::Ready => {
                    self.participants.policy.recover_ready_foundry_saving();
                }
                SavedFoundryReadiness::NeedsProtection {
                    anchor,
                    target_strength,
                } => {
                    self.participants
                        .policy
                        .release_saved_foundry_for_preparation();
                    obligation = None;
                    preparation_need = Some((anchor, target_strength));
                }
                SavedFoundryReadiness::Blocked => {
                    if self
                        .participants
                        .policy
                        .retain_blocked_foundry_saving(self.context.observation.tick)
                    {
                        obligation = Some(saved.blocked_by_execution());
                    } else {
                        obligation = None;
                    }
                }
            }
        }
        let mut saving = 0;
        let blocked = obligation.is_some_and(ValidatedFoundryObligation::blocked);
        if let Some(saved) = obligation {
            saving = saved
                .current_construction_capital()
                .saturating_add(saved.forecast_construction_capital())
                .saturating_add(saved.protected_reserve());
            let unrepresented_protected_reserve = saved
                .protected_reserve()
                .saturating_sub(air_lift.saved_plan_reserve_already_imported);
            if unrepresented_protected_reserve > 0 {
                retain_first_coordinator_failure(
                    &mut obligations.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    push_clamped_current_reserve(
                        &mut obligations.obligations,
                        self.context.observation.scrap,
                        saved.accepted_at(),
                        self.context.observation.tick,
                        ObligationKey::OpeningCore { sequence: 3 },
                        unrepresented_protected_reserve,
                    ),
                );
            }
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_obligation(
                    &mut obligations.obligations,
                    saved_foundry_obligation(saved),
                ),
            );
        }
        let mut saved = SavedFoundryPreparation {
            obligation,
            saving,
            blocked,
            preparation_need,
        };
        self.reconcile_lift_funding_after_saved_capital(&mut saved, obligations);
        saved
    }

    pub(super) fn reconcile_lift_funding_after_saved_capital(
        &mut self,
        saved: &mut SavedFoundryPreparation,
        obligations: &mut ObligationPreparation,
    ) {
        let Some(active) = obligations.active_lift.as_ref() else {
            return;
        };
        let accepted_at = active.accepted_at();
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at,
            key: ObligationKey::Legacy {
                channel: LegacyChannel::Lift,
                sequence: 2,
            },
        };
        if !self.requires_recovery(owner, saved, obligations, RetainedHorizon::Lift) {
            return;
        }
        self.participants
            .lifts
            .recover_invalid_production(self.context.observation.tick);
        obligations
            .obligations
            .retain(|obligation| obligation.owner() != owner);
        obligations.active_lift = None;
    }

    fn stage_active_island(
        &mut self,
        claims: &mut ClaimSnapshot,
        obligations: &mut ObligationPreparation,
        recon_paid_exclusions: &[(oxide_sim::ids::BuildingId, UnitKind, usize)],
    ) {
        if obligations.coordinator_failure.is_some() {
            return;
        }
        if !{ self.participants.strategy.has_active_island_operation() } {
            return;
        }
        let accepted_at = self.participants.strategy.air_admitted_at();
        let protected_current_scrap =
            current_reserve_at(&obligations.obligations, self.context.observation.tick);
        let production_deadline = self
            .context
            .observation
            .tick
            .saturating_add(connected_preparation_horizon());
        let protected_forecast_scrap =
            forecast_reserve_through(&obligations.obligations, production_deadline);
        let Some((producer_lane_reservations, prior_producer_intents)) = retained_producer_context(
            &obligations.resources,
            &obligations.obligations,
            self.context.dials.cadence,
            self.context.observation.tick,
            &self.participants.policy.planning,
        ) else {
            // Later shared adjudication owns recovery. Freezing here would roll
            // it back when an older island is visited before an invalid Lift.
            return;
        };
        let Some(result) = self.participants.strategy.stage_active_island(
            StrategicThinkContext::new(
                self.context.profile,
                self.context.tuning,
                self.context.observation,
                self.context.intelligence,
                self.context.home,
                StrategicCoordination {
                    planning: Some(&self.participants.policy.planning),
                    enlisted: &claims.planner_claims,
                    lift_support: self.context.lift_support,
                    allow_new_operation: claims.opening_core.ready,
                    protected_current_scrap,
                    protected_forecast_scrap,
                    public_map: Some(self.context.public_map),
                    orientation: self.context.orientation,
                },
            )
            .with_producer_lanes(&prior_producer_intents, &producer_lane_reservations)
            .with_paid_exclusions(recon_paid_exclusions),
        ) else {
            return;
        };
        let accepted_at = accepted_at.expect("a staged island operation has an admission tick");
        let retained_units = result.decision.reservations.clone();
        obligations.legacy_air_claims = None;
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations.obligations,
                &obligations.resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at,
                    decision_at: self.context.observation.tick,
                    retained_at: accepted_at,
                    channel: LegacyChannel::StrategicAir,
                    decision: &result.decision,
                    protect_unspent_current_scrap: true,
                    prior_producer_intents: &prior_producer_intents,
                    retained_units,
                    production_deadline,
                },
            ),
        );
        obligations.staged_strategy = Some(result);
    }

    fn prepare_active_connected_revision(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
        recon_paid_exclusions: &[(oxide_sim::ids::BuildingId, UnitKind, usize)],
    ) -> ActiveRevisionPreparation {
        let deadline = self
            .participants
            .strategy
            .connected_package_diagnostics()
            .map(|diagnostics| diagnostics.preparation_deadline)
            .unwrap_or_else(|| {
                self.context
                    .observation
                    .tick
                    .saturating_add(connected_preparation_horizon())
            });
        let connected_precedes_foundry =
            self.participants
                .strategy
                .air_admitted_at()
                .is_some_and(|accepted_at| {
                    self.participants
                        .policy
                        .operation_precedes_foundry_saving(accepted_at)
                });
        let other_obligations = obligations
            .obligations
            .iter()
            .filter(|obligation| {
                !matches!(obligation.key, ObligationKey::ConnectedOffense { .. })
                    && !(connected_precedes_foundry
                        && matches!(obligation.key, ObligationKey::SavedFoundry { .. }))
            })
            .cloned()
            .collect::<Vec<_>>();
        let protected_current_scrap =
            current_reserve_at(&other_obligations, self.context.observation.tick);
        let protected_forecast_scrap = forecast_reserve_through(&other_obligations, deadline);
        let request = FreshConnectedProposalRequest::new(
            self.context.profile,
            self.context.tuning,
            self.context.observation,
            &obligations.resources,
            self.context.intelligence,
            self.context.home,
            StrategicCoordination {
                planning: Some(&self.participants.policy.planning),
                enlisted: &claims.planner_claims,
                lift_support: None,
                allow_new_operation: true,
                protected_current_scrap,
                protected_forecast_scrap,
                public_map: Some(self.context.public_map),
                orientation: self.context.orientation,
            },
        );
        let request = request.with_paid_exclusions(recon_paid_exclusions);
        let revision = self
            .participants
            .strategy
            .active_connected_revision_proposal(request);
        match revision {
            Ok(None) => ActiveRevisionPreparation::default(),
            Ok(Some(proposal)) => {
                let adapted = active_connected_revision_obligation(&proposal);
                let adapted = match adapted {
                    Ok(candidate) => {
                        let horizon = obligation_horizon(&other_obligations, deadline);
                        let Ok(capacity) = super::super::AllocationCapacity::from_snapshot(
                            &obligations.resources,
                            horizon,
                            self.context.dials.cadence,
                        ) else {
                            return ActiveRevisionPreparation::default();
                        };
                        match super::super::forecast::refine_obligation(
                            &capacity,
                            &other_obligations,
                            candidate,
                            &self.participants.policy.planning,
                        ) {
                            crate::planning::Progress::Ready(refined) => Ok(refined),
                            crate::planning::Progress::Deferred
                            | crate::planning::Progress::Exhausted
                            | crate::planning::Progress::ProvenInfeasible => {
                                return ActiveRevisionPreparation::default();
                            }
                        }
                    }
                    Err(error) => Err(error),
                };
                remove_active_connected_obligation(&mut obligations.obligations);
                retain_first_coordinator_failure(
                    &mut obligations.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    push_obligation(&mut obligations.obligations, adapted),
                );
                obligations.active_connected = None;
                obligations.legacy_air_claims = None;
                ActiveRevisionPreparation {
                    proposal: Some(proposal),
                    rejected: None,
                }
            }
            Err(rejected) if rejected.reason.is_deferred() => ActiveRevisionPreparation {
                proposal: None,
                rejected: Some(rejected),
            },
            Err(rejected) => {
                let retained_units =
                    active_air_units(self.participants.strategy, self.context.observation);
                remove_active_connected_obligation(&mut obligations.obligations);
                self.participants.strategy.reject_active_connected_revision(
                    rejected.reason,
                    self.context.observation.tick,
                );
                retain_first_coordinator_failure(
                    &mut obligations.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    push_obligation(
                        &mut obligations.obligations,
                        legacy_unit_obligation(
                            self.context.observation.tick,
                            LegacyChannel::StrategicAir,
                            0,
                            retained_units,
                        ),
                    ),
                );
                obligations.active_connected = None;
                obligations.legacy_air_claims = None;
                ActiveRevisionPreparation {
                    proposal: None,
                    rejected: Some(rejected),
                }
            }
        }
    }

    pub(super) fn downgrade_unfundable_active_connected(
        &mut self,
        saved: &mut SavedFoundryPreparation,
        air_lift: &AirLiftPreparation,
        obligations: &mut ObligationPreparation,
    ) {
        let Some(active) = obligations.active_connected.clone() else {
            return;
        };
        let identity = active.identity();
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: active.accepted_at(),
            key: ObligationKey::ConnectedOffense {
                objective: identity.objective(),
                anchor: identity.anchor(),
            },
        };
        if !self.requires_recovery(
            owner,
            saved,
            obligations,
            RetainedHorizon::Connected {
                deadline: active.deadline(),
                lift_deadline: air_lift.lift_deadline,
            },
        ) {
            return;
        }
        let mut retained_units = active.units().to_vec();
        retain_observed_units(&obligations.resources, &mut retained_units);
        obligations
            .obligations
            .retain(|obligation| obligation.owner() != owner);
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_obligation(
                &mut obligations.obligations,
                legacy_unit_obligation(
                    active.accepted_at(),
                    LegacyChannel::StrategicAir,
                    0,
                    retained_units,
                ),
            ),
        );
        self.participants
            .strategy
            .recover_unfundable_active_connected(self.context.observation.tick);
        obligations.active_connected = None;
    }

    fn requires_recovery(
        &self,
        owner: ClaimOwner,
        saved: &mut SavedFoundryPreparation,
        obligations: &mut ObligationPreparation,
        horizon: RetainedHorizon<'_>,
    ) -> bool {
        loop {
            let horizon_tick = match horizon {
                RetainedHorizon::Lift => obligation_horizon(
                    &obligations.obligations,
                    self.context
                        .observation
                        .tick
                        .saturating_add(self.context.dials.cadence),
                ),
                RetainedHorizon::Connected {
                    deadline,
                    lift_deadline,
                } => {
                    let base = self
                        .context
                        .observation
                        .tick
                        .saturating_add(connected_preparation_horizon())
                        .max(
                            self.context
                                .observation
                                .tick
                                .saturating_add(self.context.dials.cadence),
                        )
                        .max(deadline)
                        .max(lift_deadline);
                    saved
                        .obligation
                        .map_or(base, |foundry| base.max(foundry.forecast_deadline()))
                }
                RetainedHorizon::Revision {
                    fresh,
                    lift_deadline,
                } => allocation_horizon(self.context, self.participants, saved, fresh, None)
                    .max(lift_deadline),
            };
            let Ok(mut allocation) = CrossDomainAllocation::new(
                &obligations.resources,
                horizon_tick,
                self.context.dials.cadence,
            ) else {
                return false;
            };
            for obligation in obligations.obligations.iter().cloned() {
                allocation.import(obligation);
            }
            let Err(AllocationError::ObligationConflict {
                obligation,
                conflict,
            }) = allocation.resolve_planned(
                AllocationPersonality::default(),
                None,
                &self.participants.policy.planning,
            )
            else {
                return false;
            };
            if obligation != owner || !connected_production_conflict(&conflict) {
                return false;
            }
            let ClaimOwner::Obligation { accepted_at, .. } = owner else {
                unreachable!("retained funding belongs to an obligation");
            };
            if !self.defer_younger_saved_foundry(saved, obligations, accepted_at) {
                return true;
            }
            // Removing the younger Foundry is the only retry; rebuild its horizon too.
        }
    }

    fn defer_younger_saved_foundry(
        &self,
        saved: &mut SavedFoundryPreparation,
        obligations: &mut ObligationPreparation,
        connected_accepted_at: Tick,
    ) -> bool {
        let Some(foundry) = saved.obligation else {
            return false;
        };
        if connected_accepted_at > foundry.accepted_at()
            || !self
                .participants
                .policy
                .operation_precedes_foundry_saving(connected_accepted_at)
        {
            return false;
        }
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: foundry.accepted_at(),
            key: ObligationKey::SavedFoundry {
                anchor: foundry.anchor(),
            },
        };
        let before = obligations.obligations.len();
        obligations
            .obligations
            .retain(|obligation| obligation.owner() != owner);
        if obligations.obligations.len() == before {
            return false;
        }
        saved.obligation = None;
        true
    }

    fn reconcile_unfundable_reconnaissance(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) {
        let mut unpaid: Vec<_> = self
            .participants
            .policy
            .state
            .reconnaissance
            .assignments
            .iter()
            .filter(|(_, work)| work.unpaid)
            .map(|(key, work)| (work.proposal.observed_at, *key))
            .collect();
        unpaid.sort_unstable();
        for (_, key) in unpaid.into_iter().rev() {
            let reason = if !claims.opening_core.ready {
                crate::trace::ReconReleaseReason::CoreRecovery
            } else if matches!(
                obligations_resolve(
                    &obligations.resources,
                    &obligations.obligations,
                    obligation_horizon(
                        &obligations.obligations,
                        self.context.observation.tick + self.context.dials.cadence,
                    ),
                    self.context.dials.cadence,
                    &self.participants.policy.planning,
                ),
                Ok(crate::planning::Progress::ProvenInfeasible) | Err(_)
            ) {
                crate::trace::ReconReleaseReason::Unfundable
            } else {
                break;
            };
            self.participants
                .policy
                .state
                .reconnaissance
                .release_unpaid(key, self.context.observation.tick, reason);
            obligations
                .obligations
                .retain(|obligation| obligation.key != ObligationKey::Reconnaissance(key));
        }
    }

    fn refresh_planner_claims(&self, claims: &mut ClaimSnapshot) {
        let refreshed = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        );
        claims.planner_claims = refreshed.all(&claims.team_core_claims);
        append_utility_assignments(self.participants, &mut claims.planner_claims);
        claims.strategic_core_exclusions = refreshed.core_exclusions(&claims.team_core_claims);
        append_utility_assignments(self.participants, &mut claims.strategic_core_exclusions);
    }

    fn prepare_standing_army(
        &self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) {
        let mut planner_owned = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        )
        .without_executive(&claims.team_core_claims);
        append_utility_assignments(self.participants, &mut planner_owned);
        let standing_army = self
            .context
            .enlisted
            .iter()
            .copied()
            .filter(|unit| planner_owned.binary_search(unit).is_err())
            .collect();
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_obligation(
                &mut obligations.obligations,
                legacy_unit_obligation(
                    self.context.observation.tick,
                    LegacyChannel::StandingArmy,
                    0,
                    standing_army,
                ),
            ),
        );
    }

    pub(super) fn reconcile_revision(
        &mut self,
        saved: &mut SavedFoundryPreparation,
        air_lift: &AirLiftPreparation,
        obligations: &mut ObligationPreparation,
        fresh: &FreshInvestmentPreparation,
    ) -> bool {
        let Some(revision) = fresh
            .connected
            .as_ref()
            .filter(|proposal| proposal.revises_active_operation())
        else {
            return false;
        };
        let identity = revision.identity();
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: revision.accepted_at(),
            key: ObligationKey::ConnectedOffense {
                objective: identity.objective(),
                anchor: identity.anchor(),
            },
        };
        if !self.requires_recovery(
            owner,
            saved,
            obligations,
            RetainedHorizon::Revision {
                fresh,
                lift_deadline: air_lift.lift_deadline,
            },
        ) {
            return false;
        }
        let retained_units = active_air_units(self.participants.strategy, self.context.observation);
        obligations
            .obligations
            .retain(|obligation| obligation.owner() != owner);
        self.participants
            .strategy
            .recover_unfundable_active_connected(self.context.observation.tick);
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_obligation(
                &mut obligations.obligations,
                legacy_unit_obligation(
                    self.context.observation.tick,
                    LegacyChannel::StrategicAir,
                    0,
                    retained_units,
                ),
            ),
        );
        true
    }

    pub(super) fn retain_legacy_air(&mut self, obligations: &mut ObligationPreparation) {
        if let Some((accepted_at, mut air_claims)) = obligations.legacy_air_claims.take() {
            retain_observed_units(&obligations.resources, &mut air_claims);
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_obligation(
                    &mut obligations.obligations,
                    legacy_unit_obligation(accepted_at, LegacyChannel::StrategicAir, 0, air_claims),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RetainedOrder, RetainedStep::*};

    #[test]
    fn retained_order_preserves_admission_priority_and_standing_army_boundary() {
        for (island, lift, foundry, expected) in [
            (10, 20, 30, [Island, Lift, StandingArmy, Foundry]),
            (10, 30, 20, [Island, Foundry, Lift, StandingArmy]),
            (20, 30, 10, [Foundry, Island, Lift, StandingArmy]),
            (20, 10, 30, [Lift, Island, StandingArmy, Foundry]),
            (30, 10, 20, [Lift, Foundry, Island, StandingArmy]),
            (30, 20, 10, [Foundry, Lift, Island, StandingArmy]),
            (10, 10, 10, [Island, Lift, StandingArmy, Foundry]),
            (20, 10, 10, [Lift, Foundry, Island, StandingArmy]),
        ] {
            assert_eq!(
                RetainedOrder::new(island <= lift, island <= foundry, lift <= foundry).steps(),
                expected,
                "admission ticks: island={island}, lift={lift}, Foundry={foundry}",
            );
        }
    }
}
