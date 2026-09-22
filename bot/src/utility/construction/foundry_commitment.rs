//! Accepted Foundry identity, funding, safety recovery, and exact dispatch.

use super::*;

/// A fresh proposal cannot replace another persistent Foundry obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExistingFoundryCommitment;

/// Accepted expansion whose exact build has not yet survived command lowering.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::utility) struct FoundrySavingCommitment {
    pub(in crate::utility) plan: FoundryExpansionPlan,
    pub(in crate::utility) accepted_at: Tick,
    pub(in crate::utility) required_scrap: u32,
    forecast_basis: FoundryForecastBasis,
    pub(in crate::utility) blocked_since: Option<u64>,
}

/// The immutable economic basis under which cross-domain allocation admitted
/// a Foundry that may need future completed-source income.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::utility) struct FoundryForecastBasis {
    pub(in crate::utility) planning_scrap: u32,
    pub(in crate::utility) protected_reserve: u32,
    pub(in crate::utility) deadline: Tick,
    pub(in crate::utility) decision_cadence: Tick,
}

/// Exact surviving claims for one structurally valid saved Foundry plan.
///
/// The split is recomputed by the expansion domain from the current bank and
/// the accepted fixed-horizon forecast. Cross-domain coordination can import
/// these claims without reading policy internals or reconstructing the quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValidatedFoundryObligation {
    accepted_at: Tick,
    anchor: TilePos,
    builder: UnitId,
    current_construction_capital: u32,
    forecast_construction_capital: u32,
    planning_scrap: u32,
    protected_reserve: u32,
    forecast_deadline: Tick,
    blocked: bool,
}

impl ValidatedFoundryObligation {
    pub(crate) const fn accepted_at(self) -> Tick {
        self.accepted_at
    }

    pub(crate) const fn anchor(self) -> TilePos {
        self.anchor
    }

    pub(crate) const fn builder(self) -> UnitId {
        self.builder
    }

    pub(crate) const fn current_construction_capital(self) -> u32 {
        self.current_construction_capital
    }

    pub(crate) const fn forecast_construction_capital(self) -> u32 {
        self.forecast_construction_capital
    }

    pub(crate) const fn planning_scrap(self) -> u32 {
        self.planning_scrap
    }

    pub(crate) const fn protected_reserve(self) -> u32 {
        self.protected_reserve
    }

    pub(crate) const fn forecast_deadline(self) -> Tick {
        self.forecast_deadline
    }

    pub(crate) const fn blocked(self) -> bool {
        self.blocked
    }

    pub(crate) const fn blocked_by_execution(mut self) -> Self {
        self.blocked = true;
        self
    }

    pub(crate) fn ready_to_build(self) -> bool {
        !self.blocked
            && self.current_construction_capital
                == BuildingKind::Foundry
                    .base_stats()
                    .construction
                    .expect("Foundries are constructible")
                    .cost
            && self.forecast_construction_capital == 0
    }

    /// Reattributes the unchanged saved construction cost after shared
    /// allocation considers older persistent work in the same forecast.
    pub(crate) fn with_allocated_funding(
        mut self,
        current_construction_capital: u32,
        forecast_construction_capital: u32,
    ) -> Option<Self> {
        let expected = self
            .current_construction_capital
            .saturating_add(self.forecast_construction_capital);
        if current_construction_capital.saturating_add(forecast_construction_capital) != expected {
            return None;
        }
        self.current_construction_capital = current_construction_capital;
        self.forecast_construction_capital = forecast_construction_capital;
        Some(self)
    }

    pub(crate) fn site(self) -> SiteFootprint {
        SiteFootprint::new(self.anchor, BuildingKind::Foundry.base_stats().size)
            .expect("Foundries have a positive footprint")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::utility) struct FoundryFundingRevalidation {
    pub(in crate::utility) planning_scrap: u32,
    pub(in crate::utility) current_construction_capital: u32,
    pub(in crate::utility) forecast_construction_capital: u32,
    pub(in crate::utility) protected_reserve: u32,
    pub(in crate::utility) deadline: Tick,
    pub(in crate::utility) viable: bool,
}

/// How long a temporarily unexecutable frozen plan keeps its fund before the
/// policy releases it for deterministic replanning.
pub(in crate::utility) const FOUNDRY_RECOVERY_TICKS: u64 = 600;

pub(super) fn foundry_command_spendable_forecast(
    resources: &ResourceSnapshot,
    deadline: Tick,
    decision_cadence: Tick,
) -> u32 {
    if decision_cadence == 0 || deadline <= resources.forecast().observed_at() {
        return 0;
    }
    let observed_at = resources.forecast().observed_at();
    let last_command = observed_at.saturating_add(
        deadline
            .saturating_sub(observed_at)
            .checked_div(decision_cadence)
            .unwrap_or(0)
            .saturating_mul(decision_cadence),
    );
    resources
        .forecast()
        .income_through(last_command.saturating_sub(1))
        .amount()
}

impl UtilityPolicy {
    /// Whether an existing operation owns its place in the legacy ordering
    /// ahead of the saved Foundry. Equal ticks mean the operation was admitted
    /// earlier in the same normal decision pass, before utility expansion.
    pub(crate) fn operation_precedes_foundry_saving(&self, started_at: Tick) -> bool {
        self.state
            .foundry_saving
            .as_ref()
            .is_none_or(|saving| started_at <= saving.accepted_at)
    }

    fn foundry_saving_transitioned(obs: &Observation, saving: &FoundrySavingCommitment) -> bool {
        obs.my_buildings.iter().any(|building| {
            building.kind == BuildingKind::Foundry && building.anchor == saving.plan.anchor
        }) || obs.my_units.iter().any(|unit| {
            unit.id == saving.plan.builder
                && unit.founding == Some((BuildingKind::Foundry, saving.plan.anchor))
        })
    }

    fn foundry_saving_invalid(&self, obs: &Observation, saving: &FoundrySavingCommitment) -> bool {
        self.state.dead_anchors.contains(&saving.plan.anchor)
            || obs
                .my_units
                .iter()
                .find(|unit| unit.id == saving.plan.builder)
                .is_none_or(|builder| !builder_is_free(obs, builder))
    }

    pub(crate) fn foundry_builder_lease(&self, obs: &Observation) -> Option<BuilderLease> {
        self.state
            .foundry_saving
            .as_ref()
            .filter(|saving| {
                !Self::foundry_saving_transitioned(obs, saving)
                    && !self.foundry_saving_invalid(obs, saving)
            })
            .map(|saving| {
                BuilderLease::new(
                    saving.plan.builder,
                    BuildingKind::Foundry,
                    saving.plan.anchor,
                )
            })
    }

    /// Returns the currently valid saved expansion capital. Survival releases
    /// the persistent claim before allocation imports its capital.
    pub(crate) fn validated_foundry_saving(
        &mut self,
        obs: &Observation,
        opening_core_ready: bool,
    ) -> u32 {
        let release = !opening_core_ready
            || self.state.foundry_saving.as_ref().is_some_and(|saving| {
                Self::foundry_saving_transitioned(obs, saving)
                    || self.foundry_saving_invalid(obs, saving)
            });
        if release {
            self.state.foundry_saving = None;
        }
        self.state
            .foundry_saving
            .as_ref()
            .map_or(0, |saving| saving.required_scrap)
    }

    pub(in crate::utility) fn foundry_funding_revalidation(
        resources: &ResourceSnapshot,
        saving: &FoundrySavingCommitment,
        current_before_protected_reserve: u32,
    ) -> FoundryFundingRevalidation {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let basis = saving.forecast_basis;

        let surviving_forecast =
            foundry_command_spendable_forecast(resources, basis.deadline, basis.decision_cadence);
        let planning_scrap = current_before_protected_reserve
            .saturating_add(surviving_forecast)
            .min(basis.planning_scrap);
        let current_construction_capital = current_before_protected_reserve
            .saturating_sub(basis.protected_reserve)
            .min(foundry_cost);
        let available_construction_capital = planning_scrap
            .saturating_sub(basis.protected_reserve)
            .min(foundry_cost);
        let protected_reserve_funded = current_before_protected_reserve >= basis.protected_reserve;
        FoundryFundingRevalidation {
            planning_scrap,
            current_construction_capital,
            forecast_construction_capital: available_construction_capital
                .saturating_sub(current_construction_capital),
            protected_reserve: basis.protected_reserve,
            deadline: basis.deadline,
            viable: protected_reserve_funded && available_construction_capital == foundry_cost,
        }
    }

    /// Validates and exposes one saved Foundry as exact allocator claims.
    ///
    /// `current_before_protected_reserve` is the current bank left after every
    /// older obligation except this plan's frozen protected reserve. The
    /// caller imports that reserve separately and must import only the returned
    /// construction-capital split for the saved Foundry itself.
    pub(crate) fn validated_foundry_obligation(
        &mut self,
        obs: &Observation,
        resources: &ResourceSnapshot,
        opening_core_ready: bool,
        current_before_protected_reserve: u32,
    ) -> Option<ValidatedFoundryObligation> {
        self.validated_foundry_saving(obs, opening_core_ready);
        let saving = self.state.foundry_saving.as_ref()?.clone();
        let funding = Self::foundry_funding_revalidation(
            resources,
            &saving,
            current_before_protected_reserve,
        );
        if !funding.viable && !self.retain_blocked_foundry_saving(obs.tick) {
            return None;
        }
        let saving = self
            .state
            .foundry_saving
            .as_ref()
            .expect("bounded recovery retained the validated Foundry");
        Some(ValidatedFoundryObligation {
            accepted_at: saving.accepted_at,
            anchor: saving.plan.anchor,
            builder: saving.plan.builder,
            current_construction_capital: funding.current_construction_capital,
            forecast_construction_capital: funding.forecast_construction_capital,
            planning_scrap: funding.planning_scrap,
            protected_reserve: funding.protected_reserve,
            forecast_deadline: funding.deadline,
            blocked: !funding.viable,
        })
    }

    pub(crate) fn foundry_handoff(&self) -> FoundryHandoff {
        self.state
            .foundry_saving
            .as_ref()
            .map_or_else(FoundryHandoff::default, |saving| FoundryHandoff {
                protected_scrap: saving.required_scrap,
                committed: true,
                current_scrap: 0,
            })
    }

    /// Late safety evidence can cancel unpaid work, but cannot fund or dispatch it.
    pub(in crate::utility) fn finish_foundry_safety(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: ProductionContext<'_>,
        handoff: FoundryHandoff,
        intents: &[Intent],
    ) {
        let Some(saving) = self.state.foundry_saving.clone() else {
            return;
        };
        if saving.accepted_at == obs.tick
            || intents.iter().any(|intent| {
                matches!(intent,
            Intent::BuildWith { builder, kind: BuildingKind::Foundry, anchor }
                if *builder == saving.plan.builder && *anchor == saving.plan.anchor)
            })
        {
            return;
        }
        let mut unavailable = Vec::new();
        for intent in intents {
            Self::claim_non_preemptible_intent_units(intent, &mut unavailable);
        }
        let (foundries, pending) =
            Self::projected_foundries_after(obs, context.claims.cancellations);
        let builders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|builder| {
                builder.id == saving.plan.builder
                    && context.claims.cancellations.builder_is_free(obs, builder)
                    && !context.claims.enlisted.contains(&builder.id)
                    && !context.claims.reserved.contains(&builder.id)
                    && !unavailable.contains(&builder.id)
                    && self.state.scout != Some(builder.id)
            })
            .collect();
        let resources = ResourceSnapshot::from_observation(obs);
        let funding =
            Self::foundry_funding_revalidation(&resources, &saving, handoff.current_scrap);
        let assessment = context
            .public_map
            .filter(|_| dials.expansion && pending == 0 && funding.viable)
            .and_then(|public_map| {
                self.player_facing_foundry_assessment(
                    dials,
                    obs,
                    FoundryAssessmentContext {
                        claim: FoundryClaimContext {
                            cancellations: context.claims.cancellations,
                            home: context.home,
                            projected_foundries: &foundries,
                            builders: &builders,
                            support_extractors: obs.my_buildings.iter().any(|building| {
                                building.kind == BuildingKind::Fabricator && building.built
                            }),
                            ordinary_frontiers: true,
                            unit_contacts: context.unit_contacts,
                            building_contacts: context.building_contacts,
                        },
                        public_map,
                        combat_core_exclusions: context.combat_core_exclusions,
                        spendable_scrap: funding.planning_scrap,
                        voluntary_scrap_guard: Reserve::Exact(funding.protected_reserve),
                        required_anchor: Some(saving.plan.anchor),
                    },
                    intents,
                )
            });
        match assessment.map(|assessment| assessment.disposition) {
            Some(expansion::ExpansionDisposition::Prepare { .. }) => {
                self.release_saved_foundry_for_preparation()
            }
            Some(expansion::ExpansionDisposition::Build) => {
                self.recover_ready_foundry_saving();
            }
            None => {
                self.retain_blocked_foundry_saving(obs.tick);
            }
            _ => {}
        }
    }

    /// Emits the exact saved expansion once adjudication proves its current
    /// construction capital is available. The retained anchor and builder are
    /// never re-ranked at this boundary.
    pub(crate) fn dispatch_validated_foundry(
        &self,
        obligation: ValidatedFoundryObligation,
        intents: &mut Vec<Intent>,
    ) -> bool {
        if !obligation.ready_to_build()
            || self.state.foundry_saving.as_ref().is_none_or(|saving| {
                saving.accepted_at != obligation.accepted_at
                    || saving.plan.anchor != obligation.anchor
                    || saving.plan.builder != obligation.builder
            })
        {
            return false;
        }
        Self::insert_build_before_harvest(
            intents,
            BuildingKind::Foundry,
            obligation.anchor,
            Intent::BuildWith {
                builder: obligation.builder,
                kind: BuildingKind::Foundry,
                anchor: obligation.anchor,
            },
        );
        true
    }

    pub(crate) fn retain_blocked_foundry_saving(&mut self, now: u64) -> bool {
        let Some(saving) = self.state.foundry_saving.as_mut() else {
            return false;
        };
        let blocked_since = *saving.blocked_since.get_or_insert(now);
        if now.saturating_sub(blocked_since) >= FOUNDRY_RECOVERY_TICKS {
            self.state.foundry_saving = None;
            false
        } else {
            true
        }
    }

    pub(crate) fn recover_ready_foundry_saving(&mut self) {
        if let Some(saving) = self.state.foundry_saving.as_mut() {
            saving.blocked_since = None;
        }
    }

    /// Releases a retained plan before its capital is imported into the shared
    /// allocator. The expansion can be reconsidered after mobile protection
    /// changes, without retaining a stale site or builder.
    pub(crate) fn release_saved_foundry_for_preparation(&mut self) {
        self.state.foundry_saving = None;
    }

    /// Rechecks the exact retained Foundry against current security and route
    /// evidence before the allocator imports or dispatches its capital.
    pub(crate) fn saved_foundry_readiness(
        &self,
        dials: &Dials,
        obs: &Observation,
        obligation: ValidatedFoundryObligation,
        context: FreshFoundryProposalContext<'_>,
    ) -> SavedFoundryReadiness {
        let Some(saving) = self.state.foundry_saving.as_ref() else {
            return SavedFoundryReadiness::Blocked;
        };
        let (_, pending_foundries) = Self::projected_foundries(obs);
        if pending_foundries != 0 {
            return SavedFoundryReadiness::Blocked;
        }
        let builders = obs
            .my_units
            .iter()
            .filter(|builder| builder.id == obligation.builder())
            .filter(|builder| context.available_builders.contains(&builder.id))
            .filter(|builder| builder_is_free(obs, builder))
            .filter(|builder| self.state.scout != Some(builder.id))
            .collect::<Vec<_>>();
        let danger = self.harvest_danger_projection(
            obs,
            Some(context.unit_contacts),
            Some(context.building_contacts),
        );
        self.prepare_ground_producer_egress(obs);
        if self.legal_foundry_builder_prepared(
            obs,
            &BuildRouteProjection::new(
                QueryPurpose::FoundryLogistics,
                obs,
                Some(context.public_map),
            ),
            saving.plan.anchor,
            &builders,
            &danger,
            FoundationCancellations::default(),
        ) != Some(saving.plan.builder)
        {
            return SavedFoundryReadiness::Blocked;
        }

        let hostile_starts = self.uncleared_hostile_starts(context.public_map, obs.me);
        let assessment_context = expansion::ExpansionAssessmentContext {
            obs,
            public_map: context.public_map,
            unit_contacts: context.unit_contacts,
            uncleared_hostile_starts: &hostile_starts,
            combat_core_exclusions: context.combat_core_exclusions,
            same_think_intents: context.same_think_intents,
            minimum_core_equivalents: dials.minimum_core_equivalents,
            own_strength_scale: dials.own_strength_scale,
            economy: expansion_economy(
                dials,
                obs,
                obligation.planning_scrap(),
                Reserve::Exact(obligation.protected_reserve()),
            ),
        };
        let assessment = expansion::assess_retained_foundry(
            saving.plan.opportunity,
            saving.plan.builder,
            &assessment_context,
            &mut self.queries.expansion_routing_cache.borrow_mut(),
        );
        match assessment.disposition {
            expansion::ExpansionDisposition::Build => SavedFoundryReadiness::Ready,
            expansion::ExpansionDisposition::Prepare { .. } => {
                SavedFoundryReadiness::NeedsProtection {
                    anchor: saving.plan.anchor,
                    target_strength: assessment.preparation_target_strength,
                }
            }
            expansion::ExpansionDisposition::Reject => SavedFoundryReadiness::Blocked,
        }
    }

    /// Freezes and optionally dispatches the exact proposal selected by the
    /// cross-domain allocator. No observation is accepted here, so commitment
    /// cannot silently rerank the proposal to a different site or builder.
    pub(crate) fn commit_adjudicated_foundry(
        &mut self,
        proposal: FreshFoundryProposal,
        accepted_at: Tick,
        intents: &mut Vec<Intent>,
    ) -> Result<(), ExistingFoundryCommitment> {
        if self.state.foundry_saving.is_some() {
            return Err(ExistingFoundryCommitment);
        }

        let disposition = proposal.adjudicated_commit();
        let required_scrap = proposal.saving_threshold();
        let forecast_basis = FoundryForecastBasis {
            planning_scrap: proposal.planning_scrap,
            protected_reserve: proposal.protected_reserve,
            deadline: proposal.forecast_deadline,
            decision_cadence: proposal.decision_cadence,
        };
        let plan = proposal.plan;
        self.state.foundry_saving = Some(FoundrySavingCommitment {
            plan: plan.clone(),
            accepted_at,
            required_scrap,
            forecast_basis,
            blocked_since: None,
        });
        if disposition == AdjudicatedFoundryCommit::Build {
            Self::insert_build_before_harvest(
                intents,
                BuildingKind::Foundry,
                plan.anchor,
                Intent::BuildWith {
                    builder: plan.builder,
                    kind: BuildingKind::Foundry,
                    anchor: plan.anchor,
                },
            );
        }
        Ok(())
    }

    /// Releases a frozen expansion lease only once its exact construction
    /// command survives lowering. A refused or displaced intent remains owned
    /// and can be retried on the next observation.
    pub(crate) fn record_dispatched_foundry_build(
        &mut self,
        builders: &[UnitId],
        kind: BuildingKind,
        anchor: TilePos,
    ) {
        let dispatched = self.state.foundry_saving.as_ref().is_some_and(|saving| {
            kind == BuildingKind::Foundry
                && saving.plan.anchor == anchor
                && builders.contains(&saving.plan.builder)
        });
        if dispatched {
            self.state.foundry_saving = None;
        }
    }
}
