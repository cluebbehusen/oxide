//! Coordinator-facing adapters for prior accepted work and allocation results.
//!
//! The allocator deliberately knows nothing about controller lifecycle. This
//! module translates the exact work already visible at a decision boundary into
//! mandatory claims and summarizes the selected portfolio without teaching the
//! frame loop the allocator's internal accounting.

use super::{
    AllocationCapacity, AllocationError, AllocationPersonality, ClaimBundle, ClaimBundleError,
    ClaimOwner, ConnectedMarginalError, ConnectedPortfolioContext, DeferrableCapitalClaim,
    DomainAllocationResult, DomainInvestmentProposal, ForecastClaim, ImportedObligation,
    IncompatibleLayoutPair, LegacyChannel, ObligationClass, ObligationKey, ProducerJobClaim,
    ProposalKey, ScheduledProducerJob, accepted_portfolio_rank, allocate_requiring,
    allocate_with_incompatible_layouts, future_producer_lane_reservations,
};
use crate::bot::observation::Observation;
use crate::bot::resources::ProducerLaneReservations;
use crate::bot::resources::{BuilderObligation, ResourceSnapshot, SiteFootprint};
use crate::bot::strategy::{
    ActiveConnectedObligation, ConnectedProducerAssignment, ConnectedProducerFunding,
    ConnectedProducerTiming, FreshConnectedProposal, StrategicDecision,
};
use crate::bot::trace::AllocationTrace;
use crate::bot::utility::FreshEmergencyDefense;
use crate::ids::{BuildingId, UnitId};
use crate::stats::BuildingKind;
use chassis::Tick;

/// Failure to translate exact controller evidence into allocator input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinatorInputError {
    /// One domain or obligation supplied a malformed atomic claim set.
    Claims(ClaimBundleError),
    /// The resource snapshot could not support the requested bounded forecast.
    Projection(crate::bot::resources::PlanningProjectionError),
    /// A same-think command no longer has its exact observed producer slot.
    ImmediateProducerUnavailable {
        producer: BuildingId,
        kind: crate::stats::UnitKind,
    },
}

impl From<ClaimBundleError> for CoordinatorInputError {
    fn from(error: ClaimBundleError) -> Self {
        Self::Claims(error)
    }
}

impl From<crate::bot::resources::PlanningProjectionError> for CoordinatorInputError {
    fn from(error: crate::bot::resources::PlanningProjectionError) -> Self {
        Self::Projection(error)
    }
}

/// One bounded cross-domain allocation pass before portfolio selection.
pub(crate) struct CrossDomainAllocation {
    capacity: AllocationCapacity,
    current_scrap: u32,
    obligations: Vec<ImportedObligation>,
    proposals: Vec<DomainInvestmentProposal>,
    contextual_proposals: Vec<ContextualProposalSet>,
    incompatible_layouts: Vec<IncompatibleLayoutPair>,
}

#[derive(Debug, Clone)]
struct ContextualProposalSet {
    context: ConnectedPortfolioContext,
    proposals: Vec<DomainInvestmentProposal>,
}

impl CrossDomainAllocation {
    /// Starts one pass from the sole resource snapshot for this observation.
    pub(crate) fn new(
        resources: &ResourceSnapshot,
        forecast_horizon: Tick,
        cadence: Tick,
    ) -> Result<Self, CoordinatorInputError> {
        Ok(Self {
            capacity: AllocationCapacity::from_snapshot(resources, forecast_horizon, cadence)?,
            current_scrap: resources.current_scrap().amount(),
            obligations: Vec::new(),
            proposals: Vec::new(),
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        })
    }

    /// Imports prior work after its domain has supplied an exact claim bundle.
    pub(crate) fn import(&mut self, obligation: ImportedObligation) {
        self.obligations.push(obligation);
    }

    /// Offers one already-ranked domain payload. A domain may provide ordered,
    /// mutually exclusive alternatives; portfolio selection accepts at most
    /// one of them.
    pub(crate) fn offer(&mut self, proposal: DomainInvestmentProposal) {
        self.proposals.push(proposal);
    }

    /// Rejects a pair of individually legal builds when their combined layout
    /// fails a domain-owned route, egress, or resource-access preflight.
    pub(crate) fn reject_incompatible_layout(&mut self, first: ProposalKey, second: ProposalKey) {
        if let Some(pair) = IncompatibleLayoutPair::new(first, second) {
            self.incompatible_layouts.push(pair);
            self.incompatible_layouts.sort_unstable();
            self.incompatible_layouts.dedup();
        }
    }

    /// Registers every proposal derived against one exact connected state.
    ///
    /// Register empty proposal sets too: the absence of standing-force demand
    /// is itself contextual evidence that must compete with other states.
    pub(crate) fn offer_context(
        &mut self,
        context: ConnectedPortfolioContext,
        proposals: Vec<DomainInvestmentProposal>,
    ) {
        self.contextual_proposals
            .push(ContextualProposalSet { context, proposals });
    }

    /// Selects the best compatible portfolio, then tries cumulative connected
    /// additions from largest to smallest against the exact residual capacity.
    pub(crate) fn resolve(
        self,
        personality: AllocationPersonality,
        trace: Option<&mut AllocationTrace>,
    ) -> Result<CrossDomainSettlement, AllocationError> {
        let Self {
            capacity,
            current_scrap,
            obligations,
            proposals,
            contextual_proposals,
            incompatible_layouts,
        } = self;
        let mut trace = trace;
        let (mut result, considered_proposals, selected_context, considered_contexts) =
            if contextual_proposals.is_empty() {
                let result = match allocate_with_incompatible_layouts(
                    &capacity,
                    obligations.clone(),
                    proposals.clone(),
                    personality,
                    &incompatible_layouts,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        if let Some(trace) = trace.as_deref_mut() {
                            *trace = AllocationTrace::from_inputs(&obligations, &proposals);
                            trace.record_error(&error);
                        }
                        return Err(error);
                    }
                };
                (result, proposals, None, 0)
            } else {
                select_contextual_portfolio(
                    &capacity,
                    &obligations,
                    &proposals,
                    contextual_proposals,
                    personality,
                    &incompatible_layouts,
                )?
            };
        if let Some(trace) = trace.as_deref_mut() {
            *trace = AllocationTrace::from_inputs(&obligations, &considered_proposals);
            if let Some(context) = selected_context {
                trace.record_connected_portfolio_context(considered_contexts, context);
            }
        }
        if selected_context.is_none() {
            extend_connected_greedily(&capacity, &mut result, trace.as_deref_mut());
        } else if let Some(ConnectedPortfolioContext::Selected {
            key,
            marginal_depth,
        }) = selected_context
            && marginal_depth > 0
        {
            let marginal = result
                .connected_marginal_variants()
                .and_then(|variants| variants.get(marginal_depth - 1))
                .cloned()
                .expect("the selected context retains its exact marginal variant");
            let claims = super::connected_marginal_claims(&marginal)
                .expect("the selected context was already proven well formed");
            if let Some(trace) = trace.as_deref_mut() {
                trace.record_connected_marginal_accepted(
                    key,
                    &claims,
                    result.final_producer_schedule(),
                );
            }
        }
        let producer_lane_reservations =
            match future_producer_lane_reservations(&capacity, result.final_producer_schedule()) {
                Ok(reservations) => reservations,
                Err(error) => {
                    let error = AllocationError::ProducerReservation(error);
                    if let Some(trace) = trace.as_deref_mut() {
                        trace.record_error(&error);
                    }
                    return Err(error);
                }
            };
        if let Some(trace) = trace {
            trace.record_result(&result);
        }
        Ok(CrossDomainSettlement::new(
            current_scrap,
            obligations,
            result,
            producer_lane_reservations,
        ))
    }
}

fn select_contextual_portfolio(
    capacity: &AllocationCapacity,
    obligations: &[ImportedObligation],
    base_proposals: &[DomainInvestmentProposal],
    mut contextual: Vec<ContextualProposalSet>,
    personality: AllocationPersonality,
    incompatible_layouts: &[IncompatibleLayoutPair],
) -> Result<
    (
        DomainAllocationResult,
        Vec<DomainInvestmentProposal>,
        Option<ConnectedPortfolioContext>,
        u32,
    ),
    AllocationError,
> {
    contextual.sort_by_key(|set| set.context);
    for pair in contextual.windows(2) {
        assert_ne!(
            pair[0].context, pair[1].context,
            "each connected context must be registered exactly once"
        );
    }
    let considered_contexts = u32::try_from(contextual.len()).unwrap_or(u32::MAX);
    let mut best: Option<(
        super::PortfolioRank,
        usize,
        DomainAllocationResult,
        Vec<DomainInvestmentProposal>,
        ConnectedPortfolioContext,
    )> = None;
    for set in contextual {
        let mut proposals = base_proposals.to_vec();
        match set.context {
            ConnectedPortfolioContext::Absent => proposals.retain(|proposal| {
                !matches!(proposal.key(), ProposalKey::ConnectedOffenseMinimum(_))
            }),
            ConnectedPortfolioContext::Selected { key, .. } => proposals.retain(|proposal| {
                !matches!(proposal.key(), ProposalKey::ConnectedOffenseMinimum(other) if other != key)
            }),
        }
        proposals.extend(set.proposals);
        let required = match set.context {
            ConnectedPortfolioContext::Absent => None,
            ConnectedPortfolioContext::Selected { key, .. } => {
                Some(ProposalKey::ConnectedOffenseMinimum(key))
            }
        };
        let result = match required {
            Some(required) => allocate_requiring(
                capacity,
                obligations.to_vec(),
                proposals.clone(),
                personality,
                required,
                incompatible_layouts,
            )?,
            None => Some(allocate_with_incompatible_layouts(
                capacity,
                obligations.to_vec(),
                proposals.clone(),
                personality,
                incompatible_layouts,
            )?),
        };
        let Some(mut result) = result else {
            continue;
        };
        match set.context {
            ConnectedPortfolioContext::Absent => {
                debug_assert!(result.accepted_connected_key().is_none());
            }
            ConnectedPortfolioContext::Selected {
                key,
                marginal_depth,
            } => {
                if result.accepted_connected_key() != Some(key) {
                    continue;
                }
                if marginal_depth > 0 {
                    let Some(marginal) = result
                        .connected_marginal_variants()
                        .and_then(|variants| variants.get(marginal_depth - 1))
                        .cloned()
                    else {
                        continue;
                    };
                    match result.try_accept_connected_marginal(capacity, &marginal) {
                        Ok(_) => {}
                        Err(ConnectedMarginalError::Conflict(_)) => continue,
                        Err(
                            ConnectedMarginalError::NoAcceptedConnectedProposal
                            | ConnectedMarginalError::StaleVariant
                            | ConnectedMarginalError::MalformedClaims(_),
                        ) => {
                            debug_assert!(false, "a registered context must name a retained scale");
                            continue;
                        }
                    }
                }
            }
        }
        let rank = accepted_portfolio_rank(&result, personality);
        let scale = set.context.marginal_depth();
        let replace = best
            .as_ref()
            .is_none_or(|(best_rank, best_scale, _, _, _)| {
                (rank.clone(), scale) > (best_rank.clone(), *best_scale)
            });
        if replace {
            best = Some((rank, scale, result, proposals, set.context));
        }
    }
    let (_, _, result, proposals, context) =
        best.expect("registered contexts include one exact feasible portfolio state");
    Ok((result, proposals, Some(context), considered_contexts))
}

fn extend_connected_greedily(
    capacity: &AllocationCapacity,
    result: &mut DomainAllocationResult,
    mut trace: Option<&mut AllocationTrace>,
) {
    let connected_key = result.accepted_connected_key();
    let marginal_variants = result
        .connected_marginal_variants()
        .unwrap_or_default()
        .to_vec();
    let mut largest_rejection = None;
    let mut accepted_marginal = false;
    for marginal in marginal_variants.iter().rev() {
        match result.try_accept_connected_marginal(capacity, marginal) {
            Ok(claims) => {
                accepted_marginal = true;
                if let (Some(trace), Some(key)) = (trace.as_deref_mut(), connected_key) {
                    trace.record_connected_marginal_accepted(
                        key,
                        &claims,
                        result.final_producer_schedule(),
                    );
                }
                break;
            }
            Err(ConnectedMarginalError::Conflict(conflict)) => {
                if largest_rejection.is_none() {
                    largest_rejection = Some((marginal, conflict));
                }
            }
            Err(
                ConnectedMarginalError::NoAcceptedConnectedProposal
                | ConnectedMarginalError::StaleVariant
                | ConnectedMarginalError::MalformedClaims(_),
            ) => {
                debug_assert!(false, "a retained domain marginal must remain well formed");
                break;
            }
        }
    }
    if let (false, Some(trace), Some(key), Some((marginal, conflict))) =
        (accepted_marginal, trace, connected_key, largest_rejection)
    {
        let claims = super::connected_marginal_claims(marginal)
            .expect("the allocator already accepted this domain's claim shape");
        trace.record_connected_marginal_rejected(key, &claims, &conflict);
    }
}

/// Selected exact payloads plus owner-attributed residual accounting.
pub(crate) struct CrossDomainSettlement {
    current_scrap: u32,
    obligations: Vec<ImportedObligation>,
    result: DomainAllocationResult,
    producer_lane_reservations: ProducerLaneReservations,
}

impl CrossDomainSettlement {
    fn new(
        current_scrap: u32,
        obligations: Vec<ImportedObligation>,
        result: DomainAllocationResult,
        producer_lane_reservations: ProducerLaneReservations,
    ) -> Self {
        Self {
            current_scrap,
            obligations,
            result,
            producer_lane_reservations,
        }
    }

    /// Current bank left after every mandatory and selected exact claim.
    pub(crate) fn residual_current_scrap(&self) -> u32 {
        self.current_scrap
            .saturating_sub(self.current_committed_except(|_| false))
    }

    /// Current bank exposed to the connected planner. Its own accepted or active
    /// package remains spendable; every other owner stays protected.
    pub(crate) fn connected_current_scrap(&self) -> u32 {
        self.current_scrap
            .saturating_sub(self.current_committed_except(owner_is_connected))
    }

    /// Future non-production and producer capital protected from the connected
    /// operation by all other owners.
    pub(crate) fn connected_forecast_reserve_through(&self, through: Tick) -> u32 {
        self.forecast_committed_except_through(owner_is_connected, through)
    }

    /// Current bank exposed to residual utility. Utility-owned obligations and
    /// a selected Foundry remain spendable by their existing execution path;
    /// connected and external strategic owners stay protected.
    pub(crate) fn utility_current_scrap(&self) -> u32 {
        self.current_scrap
            .saturating_sub(self.current_committed_except(owner_is_utility))
    }

    /// Final producer assignments, retained until the accepted domain payload
    /// binds its exact lanes.
    pub(crate) fn producer_schedule(&self) -> &[ScheduledProducerJob] {
        self.result.final_producer_schedule()
    }

    /// Whether exact lane binding funded the proposal that discharges the
    /// shared current-only remainder.
    pub(crate) const fn voluntary_scrap_guard_satisfied(&self) -> bool {
        self.result.voluntary_scrap_guard_satisfied()
    }

    /// Final observation-relative split for one flexible capital owner.
    pub(crate) fn capital_assignment(
        &self,
        owner: ClaimOwner,
    ) -> Option<super::CapitalFundingAssignment> {
        self.result
            .capital_assignments
            .iter()
            .copied()
            .find(|assignment| assignment.owner == owner)
    }

    /// Future producer work protected from residual same-think admissions.
    pub(crate) const fn producer_lane_reservations(&self) -> &ProducerLaneReservations {
        &self.producer_lane_reservations
    }

    /// Consumes the settlement only after exact producer binding is complete.
    pub(crate) fn into_payloads(self) -> super::AcceptedDomainPayloads {
        self.result.into_domain_payloads()
    }

    fn current_committed_except(&self, excluded: impl Fn(ClaimOwner) -> bool) -> u32 {
        let nonproduction = self
            .obligation_and_selected_claims()
            .filter(|(owner, _)| !excluded(*owner))
            .filter(|(owner, _)| self.capital_assignment(*owner).is_none())
            .map(|(_, claims)| claims.current_scrap())
            .fold(0, u32::saturating_add);
        let production = self
            .result
            .final_producer_schedule()
            .iter()
            .filter(|job| !excluded(job.owner))
            .map(|job| job.current_scrap)
            .fold(nonproduction, u32::saturating_add);
        self.result
            .capital_assignments
            .iter()
            .filter(|assignment| !excluded(assignment.owner))
            .map(|assignment| assignment.current_scrap)
            .fold(production, u32::saturating_add)
    }

    fn forecast_committed_except_through(
        &self,
        excluded: impl Fn(ClaimOwner) -> bool,
        through: Tick,
    ) -> u32 {
        let nonproduction = self
            .obligation_and_selected_claims()
            .filter(|(owner, _)| !excluded(*owner))
            .filter(|(owner, _)| self.capital_assignment(*owner).is_none())
            .flat_map(|(_, claims)| claims.forecast_scrap())
            .filter(|claim| claim.through <= through)
            .map(|claim| claim.amount)
            .fold(0, u32::saturating_add);
        let production = self
            .result
            .final_producer_schedule()
            .iter()
            .filter(|job| !excluded(job.owner) && job.enqueued_at <= through)
            .map(|job| job.forecast_scrap)
            .fold(nonproduction, u32::saturating_add);
        self.result
            .capital_assignments
            .iter()
            .filter(|assignment| {
                !excluded(assignment.owner)
                    && assignment.through <= through
                    && assignment.forecast_scrap > 0
            })
            .map(|assignment| assignment.forecast_scrap)
            .fold(production, u32::saturating_add)
    }

    fn obligation_and_selected_claims(&self) -> impl Iterator<Item = (ClaimOwner, &ClaimBundle)> {
        self.obligations
            .iter()
            .map(|obligation| (obligation.owner(), &obligation.claims))
            .chain(
                self.result
                    .accepted
                    .iter()
                    .map(|proposal| (ClaimOwner::Proposal(proposal.key()), proposal.claims())),
            )
    }
}

fn owner_is_connected(owner: ClaimOwner) -> bool {
    matches!(
        owner,
        ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(_))
            | ClaimOwner::Obligation {
                key: ObligationKey::ConnectedOffense { .. },
                ..
            }
    )
}

fn owner_is_utility(owner: ClaimOwner) -> bool {
    matches!(
        owner,
        ClaimOwner::Proposal(ProposalKey::FoundryExpansion(_))
            | ClaimOwner::Obligation {
                key: ObligationKey::OpeningCore { .. }
                    | ObligationKey::PaidConstruction(_)
                    | ObligationKey::ObservedBuilderWork { .. }
                    | ObligationKey::DeferredFoundation { .. }
                    | ObligationKey::SavedFoundry { .. }
                    | ObligationKey::Legacy {
                        channel: LegacyChannel::AirworksCapacity,
                        ..
                    },
                ..
            }
    )
}

/// Creates one mandatory claim from already-accepted exact work.
pub(crate) fn imported_obligation(
    class: ObligationClass,
    accepted_at: Tick,
    key: ObligationKey,
    claims: ClaimBundle,
) -> ImportedObligation {
    ImportedObligation {
        class,
        accepted_at,
        key,
        claims,
    }
}

/// Conservative current-bank hold used only while a fresh domain sizes its
/// proposal. Joint allocation later assigns exact producer funding.
pub(crate) fn current_reserve_at(obligations: &[ImportedObligation], decision_tick: Tick) -> u32 {
    obligations
        .iter()
        .flat_map(|obligation| {
            core::iter::once(obligation.claims.current_scrap()).chain(
                obligation
                    .claims
                    .producer_jobs()
                    .iter()
                    .filter(|job| {
                        job.fixed_assignment()
                            .is_some_and(|fixed| fixed.enqueued_at <= decision_tick)
                    })
                    .map(|job| job.kind().stats().cost),
            )
        })
        .fold(0, u32::saturating_add)
}

/// Exact non-production forecast capital promised no later than one deadline.
///
/// Producer jobs remain outside this pre-sizing hint because joint allocation
/// owns their observation-relative current/forecast split.
pub(crate) fn forecast_reserve_through(obligations: &[ImportedObligation], through: Tick) -> u32 {
    obligations
        .iter()
        .map(|obligation| {
            obligation
                .claims
                .forecast_scrap()
                .iter()
                .filter(|claim| claim.through <= through)
                .map(|claim| claim.amount)
                .fold(0, u32::saturating_add)
        })
        .fold(0, u32::saturating_add)
}

/// Creates a current-bank reserve with no invented actor or site ownership.
pub(crate) fn current_reserve_obligation(
    accepted_at: Tick,
    key: ObligationKey,
    amount: u32,
) -> Result<ImportedObligation, ClaimBundleError> {
    Ok(imported_obligation(
        ObligationClass::Survival,
        accepted_at,
        key,
        ClaimBundle::new(
            amount,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )?,
    ))
}

/// Converts one exact same-think opening defense into a mandatory survival
/// claim. The scorer-selected builder and footprint stay frozen through
/// allocation and dispatch.
pub(crate) fn fresh_emergency_defense_obligation(
    accepted_at: Tick,
    defense: FreshEmergencyDefense,
) -> Result<ImportedObligation, ClaimBundleError> {
    let site = SiteFootprint::new(defense.anchor(), defense.kind().base_stats().size)
        .expect("building footprints are positive");
    Ok(imported_obligation(
        ObligationClass::Survival,
        accepted_at,
        ObligationKey::EmergencyDefense {
            kind: defense.kind(),
            anchor: defense.anchor(),
        },
        ClaimBundle::new(
            defense.construction_cost(),
            Vec::new(),
            vec![defense.builder()],
            Vec::new(),
            vec![site],
            Vec::new(),
        )?,
    ))
}

/// Clamps one prioritized current-bank reserve after every claim payable on
/// this observation's command boundary, while retaining the owner's original
/// acceptance tick for stable obligation ordering.
pub(crate) fn clamped_current_reserve_obligation(
    obligations: &[ImportedObligation],
    bank: u32,
    accepted_at: Tick,
    decision_tick: Tick,
    key: ObligationKey,
    desired: u32,
) -> Result<Option<ImportedObligation>, ClaimBundleError> {
    let available = bank.saturating_sub(current_reserve_at(obligations, decision_tick));
    let amount = desired.min(available);
    if amount == 0 {
        Ok(None)
    } else {
        current_reserve_obligation(accepted_at, key, amount).map(Some)
    }
}

/// Creates one explicit unmigrated unit-ownership adapter.
pub(crate) fn legacy_unit_obligation(
    accepted_at: Tick,
    channel: LegacyChannel,
    sequence: u32,
    units: Vec<UnitId>,
) -> Result<ImportedObligation, ClaimBundleError> {
    Ok(imported_obligation(
        ObligationClass::Legacy,
        accepted_at,
        ObligationKey::Legacy { channel, sequence },
        ClaimBundle::new(0, Vec::new(), Vec::new(), units, Vec::new(), Vec::new())?,
    ))
}

/// Exact evidence needed to import one unmigrated planner decision.
pub(crate) struct LegacyDecisionRequest<'a> {
    pub(crate) cadence: Tick,
    pub(crate) accepted_at: Tick,
    pub(crate) decision_tick: Tick,
    pub(crate) channel: LegacyChannel,
    pub(crate) sequence: u32,
    pub(crate) decision: &'a StrategicDecision,
    /// Earlier same-think producer commands, in their committed order.
    /// These are projection context only; this obligation does not claim them.
    pub(crate) prior_producer_intents: &'a [crate::bot::executive::Intent],
    pub(crate) production_deadline: Tick,
}

/// Imports one unmigrated planner decision, including the exact immediate
/// producer appends that account for part of its reported capital.
pub(crate) fn legacy_decision_obligation(
    resources: &ResourceSnapshot,
    request: LegacyDecisionRequest<'_>,
) -> Result<ImportedObligation, CoordinatorInputError> {
    let LegacyDecisionRequest {
        cadence,
        accepted_at,
        decision_tick,
        channel,
        sequence,
        decision,
        prior_producer_intents,
        production_deadline,
    } = request;
    let mut producers = resources
        .planning_projection(production_deadline, cadence)?
        .producers()
        .to_vec();
    for intent in prior_producer_intents {
        let crate::bot::executive::Intent::TrainAt { building, kind } = intent else {
            continue;
        };
        let Some(index) = producers
            .binary_search_by_key(
                building,
                crate::bot::resources::ProducerPlanningProjection::producer,
            )
            .ok()
        else {
            return Err(CoordinatorInputError::ImmediateProducerUnavailable {
                producer: *building,
                kind: *kind,
            });
        };
        if producers[index].append(*kind, decision_tick).is_none() {
            return Err(CoordinatorInputError::ImmediateProducerUnavailable {
                producer: *building,
                kind: *kind,
            });
        }
    }
    let mut jobs = Vec::new();
    for intent in &decision.intents {
        let crate::bot::executive::Intent::TrainAt { building, kind } = intent else {
            continue;
        };
        let Some(index) = producers
            .binary_search_by_key(
                building,
                crate::bot::resources::ProducerPlanningProjection::producer,
            )
            .ok()
        else {
            return Err(CoordinatorInputError::ImmediateProducerUnavailable {
                producer: *building,
                kind: *kind,
            });
        };
        let Some(projected) = producers[index].append(*kind, decision_tick) else {
            return Err(CoordinatorInputError::ImmediateProducerUnavailable {
                producer: *building,
                kind: *kind,
            });
        };
        jobs.push(ProducerJobClaim::fixed(
            *building,
            *kind,
            decision_tick,
            projected.starts_at,
            projected.ready_at,
            production_deadline,
        ));
    }
    let production_cost = jobs
        .iter()
        .map(|job| job.kind().stats().cost)
        .fold(0, u32::saturating_add);
    let current_scrap = decision.committed_scrap.saturating_sub(production_cost);
    Ok(imported_obligation(
        ObligationClass::Legacy,
        accepted_at,
        ObligationKey::Legacy { channel, sequence },
        ClaimBundle::new(
            current_scrap,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            jobs,
        )?,
    ))
}

/// Converts all observed non-preemptible builder work into canonical mandatory
/// claims. Builders sharing one construction site remain one owner so their
/// common footprint is claimed once.
pub(crate) fn observed_builder_obligations(
    resources: &ResourceSnapshot,
    obs: &Observation,
    current_capital: &mut u32,
    forecast_horizon: Tick,
    cadence: Tick,
) -> Result<Vec<ImportedObligation>, CoordinatorInputError> {
    let forecast = resources.planning_projection(forecast_horizon, cadence)?;
    let mut claimed_forecast = 0_u32;
    let mut builders = resources
        .builders()
        .iter()
        .filter_map(|builder| {
            builder
                .obligation
                .map(|obligation| (builder.id, obligation))
        })
        .collect::<Vec<_>>();
    builders.sort_unstable_by_key(|(builder, obligation)| {
        (builder_obligation_key(*obligation, *builder), *builder)
    });

    let mut result = Vec::new();
    let mut index = 0;
    while index < builders.len() {
        let obligation = builders[index].1;
        let group_key = builder_obligation_key(obligation, builders[index].0);
        let end = builders[index..]
            .iter()
            .position(|(builder, candidate)| {
                builder_obligation_key(*candidate, *builder) != group_key
            })
            .map_or(builders.len(), |offset| index + offset);
        let group = &builders[index..end];
        let ids = group
            .iter()
            .map(|(builder, _)| *builder)
            .collect::<Vec<_>>();
        let (class, accepted_at, key, current_cost, forecast_cost, sites) = match obligation {
            BuilderObligation::Build(building) => {
                let site = obs
                    .my_buildings
                    .iter()
                    .find(|site| site.id == building)
                    .map(|site| {
                        SiteFootprint::new(site.anchor, site.kind.tier_stats(site.tier).size)
                            .expect("building footprints are positive")
                    });
                (
                    ObligationClass::PaidWork,
                    obs.tick,
                    ObligationKey::PaidConstruction(building),
                    0,
                    None,
                    site.into_iter().collect(),
                )
            }
            BuilderObligation::Found { kind, anchor } => {
                let construction = kind
                    .base_stats()
                    .construction
                    .expect("a deferred foundation is constructible");
                let current_cost = construction.cost.min(*current_capital);
                *current_capital = current_capital.saturating_sub(current_cost);
                let forecast_cost = deferred_foundation_forecast_claim(
                    &forecast,
                    &mut claimed_forecast,
                    construction.cost.saturating_sub(current_cost),
                );
                (
                    ObligationClass::PersistentPlan,
                    // State retains the issued command but not its original
                    // tick. Treat work that has already crossed the command
                    // boundary as older than every controller-local plan. A
                    // current-tick stamp would let a later remembered plan
                    // consume its bank or income merely because its exact age
                    // survived serialization while the foundation's did not.
                    0,
                    ObligationKey::DeferredFoundation {
                        builder: ids[0],
                        anchor,
                    },
                    current_cost,
                    forecast_cost,
                    vec![
                        SiteFootprint::new(anchor, kind.base_stats().size)
                            .expect("building footprints are positive"),
                    ],
                )
            }
            BuilderObligation::Salvage(_)
            | BuilderObligation::Repair
            | BuilderObligation::Queued => (
                ObligationClass::PaidWork,
                obs.tick,
                ObligationKey::ObservedBuilderWork { builder: ids[0] },
                0,
                None,
                Vec::new(),
            ),
        };
        result.push(imported_obligation(
            class,
            accepted_at,
            key,
            ClaimBundle::new(
                current_cost,
                forecast_cost.into_iter().collect(),
                ids,
                Vec::new(),
                sites,
                Vec::new(),
            )?,
        ));
        index = end;
    }
    Ok(result)
}

fn deferred_foundation_forecast_claim(
    forecast: &crate::bot::resources::ResourcePlanningProjection,
    claimed_forecast: &mut u32,
    required: u32,
) -> Option<ForecastClaim> {
    let available = u32::try_from(forecast.forecast_through(forecast.horizon()))
        .unwrap_or(u32::MAX)
        .saturating_sub(*claimed_forecast);
    let amount = required.min(available);
    if amount == 0 {
        return None;
    }

    let cumulative_target = claimed_forecast.saturating_add(amount);
    let through = if amount == required {
        let mut cumulative = 0_u32;
        forecast
            .forecast_income()
            .iter()
            .find_map(|income| {
                cumulative = cumulative.saturating_add(income.amount);
                (cumulative >= cumulative_target).then_some(income.available_at)
            })
            .unwrap_or(forecast.horizon())
    } else {
        forecast.horizon()
    };
    *claimed_forecast = cumulative_target;
    Some(ForecastClaim { through, amount })
}

fn builder_obligation_key(
    obligation: BuilderObligation,
    builder: UnitId,
) -> (u8, u32, i32, i32, BuildingKind, u32) {
    match obligation {
        BuilderObligation::Build(building) => (0, building.0, 0, 0, BuildingKind::Foundry, 0),
        BuilderObligation::Found { kind, anchor } => (1, 0, anchor.y, anchor.x, kind, 0),
        BuilderObligation::Salvage(building) => (2, building.0, 0, 0, BuildingKind::Foundry, 0),
        BuilderObligation::Repair => (3, 0, 0, 0, BuildingKind::Foundry, builder.0),
        BuilderObligation::Queued => (4, 0, 0, 0, BuildingKind::Foundry, builder.0),
    }
}

/// Exact claims for one validated persistent Foundry plan.
pub(crate) fn saved_foundry_obligation(
    obligation: crate::bot::utility::ValidatedFoundryObligation,
) -> Result<ImportedObligation, ClaimBundleError> {
    let current_capital = obligation.current_construction_capital();
    let forecast_capital = obligation.forecast_construction_capital();
    let ready_to_build = obligation.ready_to_build();
    let mut claims = ClaimBundle::new(
        if ready_to_build { current_capital } else { 0 },
        Vec::new(),
        vec![obligation.builder()],
        Vec::new(),
        vec![obligation.site()],
        Vec::new(),
    )?;
    if !ready_to_build {
        claims = claims.with_deferrable_capital(DeferrableCapitalClaim {
            through: obligation.forecast_deadline(),
            amount: current_capital.saturating_add(forecast_capital),
        })?;
    }
    Ok(imported_obligation(
        ObligationClass::PersistentPlan,
        obligation.accepted_at(),
        ObligationKey::SavedFoundry {
            anchor: obligation.anchor(),
        },
        claims,
    ))
}

/// Converts one admitted connected package into mandatory exact claims for the
/// next allocation pass.
pub(crate) fn active_connected_obligation(
    obligation: &ActiveConnectedObligation,
) -> Result<ImportedObligation, ClaimBundleError> {
    let identity = obligation.identity();
    Ok(imported_obligation(
        ObligationClass::PersistentPlan,
        obligation.accepted_at(),
        ObligationKey::ConnectedOffense {
            objective: identity.objective(),
            anchor: identity.anchor(),
        },
        ClaimBundle::new(
            0,
            Vec::new(),
            Vec::new(),
            obligation.units().to_vec(),
            Vec::new(),
            obligation
                .provider_jobs()
                .iter()
                .copied()
                .map(|assignment| {
                    ProducerJobClaim::fixed(
                        assignment.producer(),
                        assignment.kind(),
                        assignment.timing().enqueued_at(),
                        assignment.timing().starts_at(),
                        assignment.timing().ready_at(),
                        assignment.timing().ready_before(),
                    )
                })
                .collect(),
        )?,
    ))
}

/// Extracts the selected connected proposal's exact producer schedule in the
/// domain-owned binding shape.
pub(crate) fn connected_producer_assignments(
    proposal: &FreshConnectedProposal,
    schedule: &[ScheduledProducerJob],
) -> Vec<ConnectedProducerAssignment> {
    let identity = proposal.identity();
    let owner = ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(
        super::ConnectedOffenseKey {
            objective: identity.objective(),
            anchor: identity.anchor(),
        },
    ));
    let mut assignments = schedule
        .iter()
        .filter(|job| job.owner == owner)
        .map(|job| connected_producer_assignment(identity, job.request_ordinal, job))
        .collect::<Vec<_>>();
    assignments.sort_unstable_by_key(|assignment| assignment.request_ordinal());
    assignments
}

/// Reassembles an active revision's mandatory minimum and optional marginal
/// jobs into the selected package's single ordinal space.
pub(crate) fn active_connected_revision_producer_assignments(
    proposal: &FreshConnectedProposal,
    schedule: &[ScheduledProducerJob],
) -> Vec<ConnectedProducerAssignment> {
    debug_assert!(proposal.revises_active_operation());
    let identity = proposal.identity();
    let key = ObligationKey::ConnectedOffense {
        objective: identity.objective(),
        anchor: identity.anchor(),
    };
    let minimum_owner = ClaimOwner::Obligation {
        class: ObligationClass::PersistentPlan,
        accepted_at: proposal.accepted_at(),
        key,
    };
    let marginal_owner = ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(
        super::ConnectedOffenseKey {
            objective: identity.objective(),
            anchor: identity.anchor(),
        },
    ));
    let minimum_count = proposal.minimum_claims().provider_jobs().len();
    let expected_count = proposal.selected_claims().provider_jobs().len();
    let delta = proposal
        .active_revision_provider_delta()
        .expect("an active revision retains its bound producer schedule");
    let minimum_order = delta.allocation_order(minimum_count);
    let mut minimum = schedule
        .iter()
        .filter(|job| job.owner == minimum_owner)
        .collect::<Vec<_>>();
    minimum.sort_unstable_by_key(|job| job.request_ordinal);
    let mut marginal = schedule
        .iter()
        .filter(|job| job.owner == marginal_owner)
        .collect::<Vec<_>>();
    marginal.sort_unstable_by_key(|job| job.request_ordinal);

    let mut scheduled = minimum
        .into_iter()
        .map(|job| {
            (
                *minimum_order
                    .get(job.request_ordinal)
                    .expect("the retained minimum schedule belongs to the revision"),
                job,
            )
        })
        .chain(
            marginal
                .into_iter()
                .map(|job| (minimum_count.saturating_add(job.request_ordinal), job)),
        )
        .collect::<Vec<_>>();
    scheduled.sort_unstable_by_key(|(proposal_ordinal, _)| *proposal_ordinal);
    let mut assignments = scheduled
        .into_iter()
        .map(|(proposal_ordinal, job)| {
            let binding = delta
                .jobs()
                .get(proposal_ordinal)
                .expect("the selected schedule belongs to the revision ladder");
            connected_producer_assignment(identity, binding.binding_ordinal(), job)
        })
        .collect::<Vec<_>>();
    assignments.sort_unstable_by_key(|assignment| assignment.request_ordinal());
    debug_assert_eq!(assignments.len(), expected_count);
    assignments
}

fn connected_producer_assignment(
    identity: crate::bot::strategy::ConnectedOffenseIdentity,
    request_ordinal: usize,
    job: &ScheduledProducerJob,
) -> ConnectedProducerAssignment {
    ConnectedProducerAssignment::new(
        identity,
        request_ordinal,
        job.producer,
        job.kind,
        ConnectedProducerTiming::new(
            job.enqueued_at,
            job.starts_at,
            job.ready_at,
            job.ready_before,
        ),
        ConnectedProducerFunding::new(job.current_scrap, job.forecast_scrap),
    )
}

/// Extracts refreshed observation-relative funding for one active connected
/// obligation while retaining its already accepted identity, lane, and timing.
pub(crate) fn active_connected_producer_assignments(
    obligation: &ActiveConnectedObligation,
    schedule: &[ScheduledProducerJob],
) -> Vec<ConnectedProducerAssignment> {
    let identity = obligation.identity();
    let owner = ClaimOwner::Obligation {
        class: ObligationClass::PersistentPlan,
        accepted_at: obligation.accepted_at(),
        key: ObligationKey::ConnectedOffense {
            objective: identity.objective(),
            anchor: identity.anchor(),
        },
    };
    let mut scheduled = schedule
        .iter()
        .filter(|job| job.owner == owner)
        .collect::<Vec<_>>();
    scheduled.sort_unstable_by_key(|job| job.request_ordinal);
    scheduled
        .into_iter()
        .zip(obligation.provider_jobs())
        .map(|(job, retained)| {
            ConnectedProducerAssignment::new(
                identity,
                retained.request_ordinal(),
                job.producer,
                job.kind,
                ConnectedProducerTiming::new(
                    job.enqueued_at,
                    job.starts_at,
                    job.ready_at,
                    job.ready_before,
                ),
                ConnectedProducerFunding::new(job.current_scrap, job.forecast_scrap),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::allocation::{
        ConnectedOffenseKey, DefenseInvestmentKey, ProposalCase, StandingForceKey,
        connected_investment_proposal, foundry_investment_proposal,
        standing_force_investment_proposals,
    };
    use crate::bot::observation::UnitObs;
    use crate::bot::profile::Specialty;
    use crate::bot::resources::{
        BuilderResource, ProducerPlanningProjection, ResourcePlanningFixture,
        ResourcePlanningProjection, ResourceSnapshot,
    };
    use crate::bot::standing_force::{
        StandingForceFixture, StandingForceProposal, StandingForceReason,
    };
    use crate::bot::strategy::{
        ConnectedConfidence, ConnectedExecutionSafety, ConnectedOffenseClaims,
        ConnectedOpportunityCase, ConnectedProviderJob, ConnectedStrategicValue,
        ConnectedTimeToImpact, ConnectedUrgency, FreshConnectedProposal,
        FreshConnectedProposalFixture, StrategicDecision,
    };
    use crate::bot::trace::{ConnectedMarginalDispositionTrace, ConnectedPortfolioSelectionTrace};
    use crate::bot::utility::{
        FoundryConfidence, FoundryExecutionSafety, FoundryOpportunityCase, FoundryStrategicValue,
        FoundryTimeToImpact, FoundryUrgency, FreshFoundryProposal,
    };
    use crate::ids::PlayerId;
    use crate::stats::UnitKind;
    use chassis::grid::TilePos;

    fn observation() -> Observation {
        Observation {
            version: crate::bot::observation::OBSERVATION_VERSION,
            me: PlayerId(0),
            map_width: 20,
            map_height: 20,
            visible: vec![true; 20 * 20],
            explored: vec![true; 20 * 20],
            ..Observation::default()
        }
    }

    fn harvester(id: u32) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(2, 2),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn connected_case() -> ConnectedOpportunityCase {
        ConnectedOpportunityCase::fixture(
            ConnectedUrgency::Timely,
            ConnectedConfidence::Current,
            ConnectedStrategicValue::Material,
            ConnectedTimeToImpact::Near,
            ConnectedExecutionSafety::Secure,
        )
    }

    fn foundry_case() -> FoundryOpportunityCase {
        FoundryOpportunityCase::fixture(
            FoundryUrgency::Timely,
            FoundryConfidence::Supported,
            FoundryStrategicValue::Material,
            FoundryTimeToImpact::Near,
            FoundryExecutionSafety::Secure,
        )
    }

    fn capacity_with_units(units: Vec<UnitId>) -> AllocationCapacity {
        AllocationCapacity::fixture(
            ResourcePlanningProjection::fixture(ResourcePlanningFixture {
                current_scrap: 0,
                observed_at: 120,
                horizon: 1_200,
                cadence: 12,
                forecast_income: Vec::new(),
                units,
                builders: Vec::new(),
                producers: Vec::new(),
            })
            .expect("the coordinator fixture has a bounded horizon"),
        )
    }

    fn contextual_capacity(
        current_scrap: u32,
        units: Vec<UnitId>,
        builders: Vec<UnitId>,
        producers: Vec<(BuildingId, Vec<UnitKind>)>,
    ) -> AllocationCapacity {
        AllocationCapacity::fixture(
            ResourcePlanningProjection::fixture(ResourcePlanningFixture {
                current_scrap,
                observed_at: 120,
                horizon: 1_200,
                cadence: 12,
                forecast_income: Vec::new(),
                units,
                builders: builders
                    .into_iter()
                    .map(|id| BuilderResource {
                        id,
                        kind: UnitKind::Harvester,
                        obligation: None,
                    })
                    .collect(),
                producers: producers
                    .into_iter()
                    .map(|(id, trainable)| {
                        ProducerPlanningProjection::fixture(
                            id,
                            120,
                            12,
                            120,
                            vec![120; crate::stats::QUEUE_CAP],
                            trainable,
                        )
                        .expect("the contextual producer fixture is valid")
                    })
                    .collect(),
            })
            .expect("the contextual resource fixture is valid"),
        )
    }

    fn standing_proposal(
        kind: UnitKind,
        producer: BuildingId,
        case: ProposalCase,
    ) -> DomainInvestmentProposal {
        standing_force_investment_proposals(vec![StandingForceProposal::fixture(
            StandingForceFixture {
                observed_at: 120,
                ready_before: 1_200,
                kind,
                reason: StandingForceReason::SiegePressure,
                specialty: Specialty::Siege,
                personality_emphasis: 100,
                case,
                eligible_producers: vec![producer],
            },
        )])
        .expect("the contextual Standing proposal is valid")
        .pop()
        .expect("the fixture creates one Standing proposal")
    }

    #[test]
    fn defense_capital_is_not_exposed_to_residual_utility() {
        let owner = ClaimOwner::Proposal(ProposalKey::Defense(DefenseInvestmentKey {
            kind: BuildingKind::Turret,
            anchor: TilePos::new(8, 9),
        }));

        assert!(!owner_is_utility(owner));
    }

    #[test]
    fn settlement_trace_records_the_final_foundry_funding_split_once() {
        let cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let builder = UnitId(3);
        let deadline = 360;
        let proposal = FreshFoundryProposal::fixture(
            TilePos::new(8, 9),
            builder,
            cost - 17,
            17,
            23,
            deadline,
            foundry_case(),
        );
        let resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: cost - 17,
            observed_at: 120,
            horizon: deadline,
            cadence: 12,
            forecast_income: vec![crate::bot::resources::ForecastAvailability {
                available_at: deadline,
                amount: 27,
            }],
            units: vec![builder],
            builders: vec![crate::bot::resources::BuilderResource {
                id: builder,
                kind: UnitKind::Harvester,
                obligation: None,
            }],
            producers: Vec::new(),
        })
        .expect("the Foundry forecast is valid");
        let survival =
            current_reserve_obligation(120, ObligationKey::OpeningCore { sequence: 0 }, 10)
                .expect("the survival reserve is valid");
        let allocation = CrossDomainAllocation {
            capacity: AllocationCapacity::fixture(resources),
            current_scrap: cost - 17,
            obligations: vec![survival],
            proposals: vec![
                foundry_investment_proposal(proposal)
                    .expect("the Foundry proposal has valid exact claims"),
            ],
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        };
        let mut trace = AllocationTrace::default();

        let settlement = allocation
            .resolve(AllocationPersonality::default(), Some(&mut trace))
            .expect("survival and the deferred Foundry fit together");

        assert_eq!(trace.proposals.entries.len(), 1);
        let claims = &trace.proposals.entries[0].claims;
        assert_eq!(claims.current_scrap, cost - 27);
        assert_eq!(claims.forecast_scrap_total, 27);
        assert!(claims.deferrable_capital.is_none());
        assert_eq!(claims.claimed_capital, u128::from(cost));
        assert_eq!(trace.capital_assignments.entries.len(), 1);
        let assignment = &trace.capital_assignments.entries[0];
        assert_eq!(assignment.through, deadline);
        assert_eq!(assignment.current_scrap, cost - 27);
        assert_eq!(assignment.forecast_scrap, 27);
        assert_eq!(settlement.residual_current_scrap(), 0);
        let mut payloads = settlement.into_payloads();
        let foundry = payloads
            .take_foundry()
            .expect("the exact Foundry payload was selected");
        assert_eq!(foundry.current_construction_capital(), cost - 27);
        assert_eq!(foundry.forecast_construction_capital(), 27);
    }

    #[test]
    fn legacy_training_is_charged_exactly_once() {
        let mut obs = observation();
        obs.tick = 120;
        obs.scrap = 100;
        obs.my_buildings.push(crate::bot::observation::BuildingObs {
            id: BuildingId(9),
            player: PlayerId(0),
            kind: BuildingKind::Foundry,
            anchor: TilePos::new(3, 3),
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(Vec::new());
        let resources = ResourceSnapshot::from_observation(&obs);
        let decision = StrategicDecision {
            intents: vec![crate::bot::executive::Intent::TrainAt {
                building: BuildingId(9),
                kind: UnitKind::Sentinel,
            }],
            reservations: vec![UnitId(3)],
            committed_scrap: UnitKind::Sentinel.stats().cost.saturating_add(17),
        };
        let obligation = legacy_decision_obligation(
            &resources,
            LegacyDecisionRequest {
                cadence: 12,
                accepted_at: 24,
                decision_tick: 120,
                channel: LegacyChannel::Lift,
                sequence: 0,
                decision: &decision,
                prior_producer_intents: &[],
                production_deadline: 1_200,
            },
        )
        .expect("the decision is a valid claim bundle");

        assert_eq!(obligation.accepted_at, 24);
        assert_eq!(obligation.claims.current_scrap(), 17);
        assert!(obligation.claims.units().is_empty());
        assert_eq!(obligation.claims.producer_jobs().len(), 1);
        assert_eq!(
            obligation.claims.producer_jobs()[0].committed_producer(),
            Some(BuildingId(9))
        );
        assert_eq!(
            obligation.claims.producer_jobs()[0]
                .fixed_timing()
                .map(|(_, enqueued_at, _, _)| enqueued_at),
            Some(120),
            "the planner's old priority must not backdate a current producer append"
        );
    }

    #[test]
    fn later_legacy_training_replays_the_earlier_same_tick_fifo_prefix() {
        let mut obs = observation();
        obs.tick = 120;
        obs.scrap = UnitKind::Skyhook
            .stats()
            .cost
            .saturating_add(UnitKind::Kestrel.stats().cost);
        obs.my_buildings.push(crate::bot::observation::BuildingObs {
            id: BuildingId(9),
            player: PlayerId(0),
            kind: BuildingKind::Airworks,
            anchor: TilePos::new(3, 3),
            hp: BuildingKind::Airworks.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(Vec::new());
        let resources = ResourceSnapshot::from_observation(&obs);
        let first = StrategicDecision {
            intents: vec![crate::bot::executive::Intent::TrainAt {
                building: BuildingId(9),
                kind: UnitKind::Skyhook,
            }],
            reservations: Vec::new(),
            committed_scrap: UnitKind::Skyhook.stats().cost,
        };
        let second = StrategicDecision {
            intents: vec![crate::bot::executive::Intent::TrainAt {
                building: BuildingId(9),
                kind: UnitKind::Kestrel,
            }],
            reservations: Vec::new(),
            committed_scrap: UnitKind::Kestrel.stats().cost,
        };
        let first_obligation = legacy_decision_obligation(
            &resources,
            LegacyDecisionRequest {
                cadence: 12,
                accepted_at: 24,
                decision_tick: obs.tick,
                channel: LegacyChannel::Lift,
                sequence: 0,
                decision: &first,
                prior_producer_intents: &[],
                production_deadline: 1_200,
            },
        )
        .expect("the older Airworks append is representable");
        let second_obligation = legacy_decision_obligation(
            &resources,
            LegacyDecisionRequest {
                cadence: 12,
                accepted_at: 36,
                decision_tick: obs.tick,
                channel: LegacyChannel::StrategicAir,
                sequence: 0,
                decision: &second,
                prior_producer_intents: &first.intents,
                production_deadline: 1_200,
            },
        )
        .expect("the later append can follow the exact older FIFO prefix");
        let first_timing = first_obligation.claims.producer_jobs()[0]
            .fixed_timing()
            .expect("the older append has fixed timing");
        let second_timing = second_obligation.claims.producer_jobs()[0]
            .fixed_timing()
            .expect("the later append has fixed timing");
        assert_eq!(first_timing.0, BuildingId(9));
        assert_eq!(second_timing.0, BuildingId(9));
        assert_eq!(first_timing.1, obs.tick);
        assert_eq!(second_timing.1, obs.tick);
        assert_eq!(second_timing.2, first_timing.3.saturating_add(1));

        let mut allocation = CrossDomainAllocation::new(&resources, 1_200, 12)
            .expect("the shared producer projection is valid");
        allocation.import(first_obligation);
        allocation.import(second_obligation);
        allocation
            .resolve(AllocationPersonality::default(), None)
            .expect("consecutive compatible appends must not conflict");
    }

    #[test]
    fn builders_sharing_a_foundation_claim_one_site_and_one_cost() {
        let mut obs = observation();
        obs.tick = 120;
        obs.scrap = 500;
        obs.my_units = vec![harvester(3), harvester(4)];
        let anchor = TilePos::new(8, 9);
        for id in [3, 4] {
            let unit = obs
                .my_units
                .iter_mut()
                .find(|unit| unit.id == UnitId(id))
                .expect("fixture has requested worker");
            unit.kind = UnitKind::Harvester;
            unit.founding = Some((BuildingKind::Turret, anchor));
        }
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut capital = obs.scrap;
        let obligations =
            observed_builder_obligations(&resources, &obs, &mut capital, obs.tick + 1_080, 12)
                .expect("observed work forms valid claims");

        assert_eq!(obligations.len(), 1);
        assert_eq!(obligations[0].claims.builders(), &[UnitId(3), UnitId(4)]);
        assert_eq!(obligations[0].claims.sites().len(), 1);
        assert_eq!(
            obligations[0].claims.current_scrap(),
            BuildingKind::Turret
                .base_stats()
                .construction
                .expect("Turrets are constructible")
                .cost
        );
    }

    #[test]
    fn unpaid_foundation_claims_current_bank_then_earliest_needed_income() {
        let mut obs = observation();
        obs.tick = 120;
        let cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        obs.scrap = cost / 3;
        obs.my_units = vec![harvester(3)];
        obs.my_units[0].founding = Some((BuildingKind::Foundry, TilePos::new(8, 9)));
        obs.my_buildings.push(crate::bot::observation::BuildingObs {
            id: BuildingId(20),
            player: PlayerId(0),
            kind: BuildingKind::Reclaimer,
            anchor: TilePos::new(1, 1),
            hp: BuildingKind::Reclaimer.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(Vec::new());
        let resources = ResourceSnapshot::from_observation(&obs);
        let horizon = obs
            .tick
            .saturating_add(Tick::from(cost).saturating_mul(crate::stats::RECLAIMER_PERIOD))
            .saturating_add(24);
        let forecast = resources
            .planning_projection(horizon, 12)
            .expect("the completed source supports a bounded forecast");
        let mut capital = obs.scrap;

        let obligations = observed_builder_obligations(&resources, &obs, &mut capital, horizon, 12)
            .expect("the unpaid foundation forms an exact current-plus-forecast claim");

        let claims = &obligations[0].claims;
        let expected_forecast = cost - obs.scrap;
        assert_eq!(capital, 0);
        assert_eq!(claims.current_scrap(), obs.scrap);
        assert_eq!(claims.forecast_scrap().len(), 1);
        assert_eq!(claims.forecast_scrap()[0].amount, expected_forecast);
        assert!(
            forecast.forecast_through(claims.forecast_scrap()[0].through)
                >= u64::from(expected_forecast)
        );
        assert!(
            forecast.forecast_through(claims.forecast_scrap()[0].through - 12)
                < u64::from(expected_forecast),
            "the older order must claim no later income than it actually needs"
        );
    }

    #[test]
    fn unfunded_foundation_owns_all_bounded_income_before_fresh_connected_work() {
        let mut obs = observation();
        obs.tick = 120;
        obs.scrap = 0;
        obs.my_units = vec![harvester(3)];
        obs.my_units[0].founding = Some((BuildingKind::Foundry, TilePos::new(8, 9)));
        obs.my_buildings.push(crate::bot::observation::BuildingObs {
            id: BuildingId(9),
            player: PlayerId(0),
            kind: BuildingKind::Foundry,
            anchor: TilePos::new(3, 3),
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        for id in 20..70 {
            obs.my_buildings.push(crate::bot::observation::BuildingObs {
                id: BuildingId(id),
                player: PlayerId(0),
                kind: BuildingKind::Reclaimer,
                anchor: TilePos::new(12, 12),
                hp: BuildingKind::Reclaimer.base_stats().max_hp,
                built: true,
                seen: true,
                tier: 0,
            });
        }
        obs.my_queues = vec![Vec::new(); obs.my_buildings.len()];
        let resources = ResourceSnapshot::from_observation(&obs);
        let horizon = 240;
        let forecast = resources
            .planning_projection(horizon, 12)
            .expect("the completed sources support a bounded forecast");
        let bounded_income = u32::try_from(forecast.forecast_through(horizon))
            .expect("the focused forecast fits u32");
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        assert!(bounded_income < foundry_cost);
        assert!(bounded_income >= UnitKind::Harvester.stats().cost);
        let mut capital = 0;
        let obligations = observed_builder_obligations(&resources, &obs, &mut capital, horizon, 12)
            .expect("the older foundation claims every protectable forecast unit");
        assert_eq!(
            obligations[0].claims.forecast_scrap(),
            &[ForecastClaim {
                through: horizon,
                amount: bounded_income,
            }]
        );

        let connected = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(90),
            anchor: TilePos::new(12, 8),
            deadline: horizon,
            case: connected_case(),
            minimum_claims: ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![crate::bot::strategy::ConnectedProviderJob::fixture(
                    UnitKind::Harvester,
                    obs.tick,
                    horizon,
                    vec![BuildingId(9)],
                )],
            ),
            marginal_additions: Vec::new(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let proposal = connected_investment_proposal(connected)
            .expect("the connected proposal has exact production claims");

        let mut control = CrossDomainAllocation::new(&resources, horizon, 12)
            .expect("the resource forecast is valid");
        control.offer(proposal.clone());
        assert!(
            control
                .resolve(AllocationPersonality::default(), None)
                .expect("the unclaimed forecast can fund connected work")
                .into_payloads()
                .take_connected()
                .is_some(),
            "the fixture must isolate forecast ownership rather than producer timing"
        );

        let mut contested = CrossDomainAllocation::new(&resources, horizon, 12)
            .expect("the resource forecast is valid");
        for obligation in obligations {
            contested.import(obligation);
        }
        contested.offer(proposal);
        assert!(
            contested
                .resolve(AllocationPersonality::default(), None)
                .expect("the mandatory older order remains feasible")
                .into_payloads()
                .take_connected()
                .is_none(),
            "fresh connected work must not spend forecast already promised to an unpaid foundation"
        );
    }

    #[test]
    fn marginal_fallback_traces_the_smaller_accepted_variant() {
        let objective = BuildingId(90);
        let first = UnitId(3);
        let blocked = UnitId(4);
        let proposal = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective,
            anchor: TilePos::new(12, 8),
            deadline: 1_200,
            case: connected_case(),
            minimum_claims: ConnectedOffenseClaims::default(),
            marginal_additions: vec![
                ConnectedOffenseClaims::fixture(vec![first], Vec::new()),
                ConnectedOffenseClaims::fixture(vec![first, blocked], Vec::new()),
            ],
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let mut expected = proposal.clone();
        assert!(expected.select_marginal(&proposal.marginal_variants()[0]));
        let obligation = legacy_unit_obligation(100, LegacyChannel::StandingArmy, 0, vec![blocked])
            .expect("the existing unit claim is valid");
        let allocation = CrossDomainAllocation {
            capacity: capacity_with_units(vec![first, blocked]),
            current_scrap: 0,
            obligations: vec![obligation],
            proposals: vec![
                connected_investment_proposal(proposal)
                    .expect("the connected proposal has valid claims"),
            ],
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        };
        let mut trace = AllocationTrace::default();

        let settlement = allocation
            .resolve(AllocationPersonality::default(), Some(&mut trace))
            .expect("the minimum and smaller marginal fit");
        let mut payloads = settlement.into_payloads();

        assert_eq!(payloads.take_connected(), Some(expected));
        assert!(matches!(
            trace
                .connected_marginal
                .as_ref()
                .map(|marginal| &marginal.disposition),
            Some(ConnectedMarginalDispositionTrace::Accepted)
        ));
    }

    #[test]
    fn contextual_selection_never_pairs_connected_with_absent_inventory() {
        let objective = BuildingId(90);
        let anchor = TilePos::new(12, 8);
        let connected_key = ConnectedOffenseKey { objective, anchor };
        let bombard = UnitId(3);
        let airworks = BuildingId(10);
        let foundry = BuildingId(11);
        let crucible = BuildingId(12);
        let connected = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective,
            anchor,
            deadline: 1_200,
            case: connected_case(),
            minimum_claims: ConnectedOffenseClaims::fixture(
                vec![bombard],
                vec![ConnectedProviderJob::fixture(
                    UnitKind::Buzzard,
                    120,
                    1_200,
                    vec![airworks],
                )],
            ),
            marginal_additions: Vec::new(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let common_case = ProposalCase::from(connected_case());
        let mut allocation = CrossDomainAllocation {
            capacity: contextual_capacity(
                300,
                vec![bombard],
                Vec::new(),
                vec![
                    (airworks, vec![UnitKind::Buzzard]),
                    (foundry, vec![UnitKind::Sentinel]),
                    (crucible, vec![UnitKind::Lancer]),
                ],
            ),
            current_scrap: 300,
            obligations: Vec::new(),
            proposals: vec![
                connected_investment_proposal(connected)
                    .expect("the connected minimum has valid exact claims")
                    .with_voluntary_scrap_guard(UnitKind::Sentinel.stats().cost),
            ],
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        };
        allocation.offer_context(
            ConnectedPortfolioContext::Absent,
            vec![
                standing_proposal(UnitKind::Sentinel, foundry, common_case)
                    .with_voluntary_scrap_guard(UnitKind::Sentinel.stats().cost),
            ],
        );
        allocation.offer_context(
            ConnectedPortfolioContext::Selected {
                key: connected_key,
                marginal_depth: 0,
            },
            vec![
                standing_proposal(UnitKind::Lancer, crucible, common_case)
                    .with_voluntary_scrap_guard(UnitKind::Sentinel.stats().cost),
            ],
        );
        let mut trace = AllocationTrace::default();

        let settlement = allocation
            .resolve(AllocationPersonality::default(), Some(&mut trace))
            .expect("the exact connected-only context remains feasible");

        assert_eq!(
            settlement
                .producer_schedule()
                .iter()
                .map(|job| (job.owner, job.producer, job.kind))
                .collect::<Vec<_>>(),
            vec![(
                ClaimOwner::Proposal(ProposalKey::StandingForce(StandingForceKey::fixture(
                    UnitKind::Sentinel,
                ))),
                foundry,
                UnitKind::Sentinel,
            )]
        );
        let mut payloads = settlement.into_payloads();
        assert!(payloads.take_connected().is_none());
        assert_eq!(
            payloads
                .take_standing_force()
                .map(|proposal| proposal.key_kind()),
            Some(UnitKind::Sentinel)
        );
        assert!(matches!(
            trace
                .connected_context
                .as_ref()
                .map(|context| &context.selected),
            Some(ConnectedPortfolioSelectionTrace::Absent)
        ));
        assert_eq!(
            trace
                .connected_context
                .as_ref()
                .map(|context| context.considered),
            Some(2)
        );
        assert!(
            trace.proposals.entries.iter().any(|proposal| proposal.key
                == ProposalKey::StandingForce(StandingForceKey::fixture(UnitKind::Sentinel))
                    .into()),
            "trace proposals must come from the selected inventory context"
        );
        assert!(
            trace.proposals.entries.iter().all(|proposal| proposal.key
                != ProposalKey::StandingForce(StandingForceKey::fixture(UnitKind::Lancer)).into()),
            "the selected-operation context's Standing choice must not be cloned into the absent trace"
        );
    }

    #[test]
    fn contextual_scale_binds_the_exact_deepest_producer_schedule() {
        let objective = BuildingId(90);
        let anchor = TilePos::new(12, 8);
        let connected_key = ConnectedOffenseKey { objective, anchor };
        let airworks = BuildingId(10);
        let crucible = BuildingId(12);
        let connected = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective,
            anchor,
            deadline: 1_200,
            case: connected_case(),
            minimum_claims: ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![ConnectedProviderJob::fixture(
                    UnitKind::Buzzard,
                    120,
                    1_200,
                    vec![airworks],
                )],
            ),
            marginal_additions: vec![ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![ConnectedProviderJob::fixture(
                    UnitKind::Moth,
                    120,
                    1_200,
                    vec![airworks],
                )],
            )],
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let expected_marginal = connected.marginal_variants()[0].clone();
        let common_case = ProposalCase::from(connected_case());
        let mut allocation = CrossDomainAllocation {
            capacity: contextual_capacity(
                900,
                Vec::new(),
                Vec::new(),
                vec![
                    (airworks, vec![UnitKind::Buzzard, UnitKind::Moth]),
                    (crucible, vec![UnitKind::Lancer]),
                ],
            ),
            current_scrap: 900,
            obligations: Vec::new(),
            proposals: vec![
                connected_investment_proposal(connected.clone())
                    .expect("the connected ladder has valid exact claims"),
            ],
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        };
        let standing = || standing_proposal(UnitKind::Lancer, crucible, common_case);
        allocation.offer_context(ConnectedPortfolioContext::Absent, vec![standing()]);
        allocation.offer_context(
            ConnectedPortfolioContext::Selected {
                key: connected_key,
                marginal_depth: 0,
            },
            vec![standing()],
        );
        allocation.offer_context(
            ConnectedPortfolioContext::Selected {
                key: connected_key,
                marginal_depth: 1,
            },
            vec![standing()],
        );
        let mut trace = AllocationTrace::default();

        let settlement = allocation
            .resolve(AllocationPersonality::default(), Some(&mut trace))
            .expect("the deepest exact context fits current cash and both lanes");

        assert_eq!(
            settlement
                .producer_schedule()
                .iter()
                .map(|job| (job.owner, job.producer, job.kind))
                .collect::<Vec<_>>(),
            vec![
                (
                    ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(connected_key)),
                    airworks,
                    UnitKind::Buzzard,
                ),
                (
                    ClaimOwner::Proposal(ProposalKey::StandingForce(StandingForceKey::fixture(
                        UnitKind::Lancer,
                    ))),
                    crucible,
                    UnitKind::Lancer,
                ),
                (
                    ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(connected_key)),
                    airworks,
                    UnitKind::Moth,
                ),
            ]
        );
        let mut expected = connected;
        assert!(expected.select_marginal(&expected_marginal));
        let mut payloads = settlement.into_payloads();
        assert_eq!(payloads.take_connected(), Some(expected));
        assert_eq!(
            payloads
                .take_standing_force()
                .map(|proposal| proposal.key_kind()),
            Some(UnitKind::Lancer)
        );
        assert!(matches!(
            trace
                .connected_context
                .as_ref()
                .map(|context| &context.selected),
            Some(ConnectedPortfolioSelectionTrace::Selected {
                key: _,
                marginal_depth: 1,
            })
        ));
        assert!(matches!(
            trace
                .connected_marginal
                .as_ref()
                .map(|marginal| &marginal.disposition),
            Some(ConnectedMarginalDispositionTrace::Accepted)
        ));
    }

    #[test]
    fn contextual_search_preserves_foundry_versus_connected_ordering() {
        let objective = BuildingId(90);
        let anchor = TilePos::new(12, 8);
        let connected_key = ConnectedOffenseKey { objective, anchor };
        let builder = UnitId(3);
        let producer = BuildingId(9);
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let foundry = FreshFoundryProposal::fixture(
            TilePos::new(8, 9),
            builder,
            foundry_cost,
            0,
            23,
            1_200,
            foundry_case(),
        );
        let connected = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective,
            anchor,
            deadline: 1_200,
            case: connected_case(),
            minimum_claims: ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![ConnectedProviderJob::fixture(
                    UnitKind::Sentinel,
                    120,
                    1_200,
                    vec![producer],
                )],
            ),
            marginal_additions: Vec::new(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let capacity = contextual_capacity(
            foundry_cost,
            vec![builder],
            vec![builder],
            vec![(producer, vec![UnitKind::Sentinel])],
        );
        let proposals = || {
            vec![
                foundry_investment_proposal(foundry.clone())
                    .expect("the Foundry fixture has valid exact claims"),
                connected_investment_proposal(connected.clone())
                    .expect("the connected fixture has valid exact claims"),
            ]
        };
        let control = CrossDomainAllocation {
            capacity: capacity.clone(),
            current_scrap: foundry_cost,
            obligations: Vec::new(),
            proposals: proposals(),
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        }
        .resolve(AllocationPersonality::default(), None)
        .expect("the ordinary portfolio resolves");
        let mut contextual = CrossDomainAllocation {
            capacity,
            current_scrap: foundry_cost,
            obligations: Vec::new(),
            proposals: proposals(),
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        };
        contextual.offer_context(ConnectedPortfolioContext::Absent, Vec::new());
        contextual.offer_context(
            ConnectedPortfolioContext::Selected {
                key: connected_key,
                marginal_depth: 0,
            },
            Vec::new(),
        );
        let contextual = contextual
            .resolve(AllocationPersonality::default(), None)
            .expect("the contextual portfolio resolves");

        let accepted_keys = |settlement: &CrossDomainSettlement| {
            settlement
                .result
                .accepted
                .iter()
                .map(DomainInvestmentProposal::key)
                .collect::<Vec<_>>()
        };
        assert_eq!(accepted_keys(&contextual), accepted_keys(&control));
        assert_eq!(
            contextual.producer_schedule(),
            control.producer_schedule(),
            "introducing equivalent conditional inventory must not change exact lane ownership"
        );
        assert_eq!(
            accepted_keys(&control),
            vec![ProposalKey::ConnectedOffenseMinimum(connected_key)],
            "the fixture must exercise the existing Foundry-versus-offense ordering"
        );
    }

    #[test]
    fn historical_owner_tick_does_not_hide_work_due_on_the_current_tick() {
        let kind = UnitKind::Sentinel;
        let due = 200;
        let ready_at = due + Tick::from(kind.stats().train_ticks) - 1;
        let active = imported_obligation(
            ObligationClass::PersistentPlan,
            90,
            ObligationKey::ConnectedOffense {
                objective: BuildingId(90),
                anchor: TilePos::new(12, 8),
            },
            ClaimBundle::new(
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![ProducerJobClaim::fixed(
                    BuildingId(9),
                    kind,
                    due,
                    due,
                    ready_at,
                    ready_at + 2,
                )],
            )
            .expect("the active job is a valid exact claim"),
        );
        let obligations = vec![active];
        let bank = kind.stats().cost.saturating_add(10);

        assert_eq!(current_reserve_at(&obligations, 90), 0);
        assert_eq!(current_reserve_at(&obligations, due), kind.stats().cost);
        let reserve = clamped_current_reserve_obligation(
            &obligations,
            bank,
            100,
            due,
            ObligationKey::OpeningCore { sequence: 3 },
            50,
        )
        .expect("the reserve remains structurally valid")
        .expect("ten current scrap remain after the due job");

        assert_eq!(reserve.accepted_at, 100);
        assert_eq!(reserve.claims.current_scrap(), 10);
    }

    #[test]
    fn later_foundry_claim_does_not_preconsume_earlier_connected_income() {
        let producer = BuildingId(9);
        let connected_deadline = 500;
        let foundry_deadline = 600;
        let unit = UnitKind::Harvester;
        let unit_cost = unit.stats().cost;
        let saved_foundry = imported_obligation(
            ObligationClass::PersistentPlan,
            100,
            ObligationKey::SavedFoundry {
                anchor: TilePos::new(4, 4),
            },
            ClaimBundle::new(
                0,
                vec![ForecastClaim {
                    through: foundry_deadline,
                    amount: unit_cost,
                }],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .expect("the saved Foundry forecast is a valid claim"),
        );
        let connected = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(90),
            anchor: TilePos::new(12, 8),
            deadline: connected_deadline,
            case: connected_case(),
            minimum_claims: ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![crate::bot::strategy::ConnectedProviderJob::fixture(
                    unit,
                    120,
                    connected_deadline,
                    vec![producer],
                )],
            ),
            marginal_additions: Vec::new(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: 0,
            observed_at: 120,
            horizon: foundry_deadline,
            cadence: 12,
            forecast_income: vec![
                crate::bot::resources::ForecastAvailability {
                    available_at: 300,
                    amount: unit_cost,
                },
                crate::bot::resources::ForecastAvailability {
                    available_at: foundry_deadline,
                    amount: unit_cost,
                },
            ],
            units: Vec::new(),
            builders: Vec::new(),
            producers: vec![
                ProducerPlanningProjection::fixture(
                    producer,
                    120,
                    12,
                    120,
                    vec![120; crate::stats::QUEUE_CAP],
                    vec![unit],
                )
                .expect("the connected producer is valid"),
            ],
        })
        .expect("the shared forecast has a valid bounded horizon");
        assert_eq!(
            forecast_reserve_through(core::slice::from_ref(&saved_foundry), connected_deadline),
            0,
            "a later claim must not become a scalar reserve at every earlier tick"
        );
        let allocation = CrossDomainAllocation {
            capacity: AllocationCapacity::fixture(resources),
            current_scrap: 0,
            obligations: vec![saved_foundry],
            proposals: vec![
                connected_investment_proposal(connected)
                    .expect("the connected proposal has valid exact claims"),
            ],
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        };

        let settlement = allocation
            .resolve(AllocationPersonality::default(), None)
            .expect("both claims fit against income at their own deadlines");

        assert_eq!(settlement.residual_current_scrap(), 0);
        assert_eq!(settlement.producer_schedule().len(), 1);
        assert_eq!(settlement.producer_schedule()[0].forecast_scrap, unit_cost);
        let mut payloads = settlement.into_payloads();
        assert!(payloads.take_connected().is_some());
    }

    #[test]
    fn connected_binding_restores_request_order_across_busy_and_idle_lanes() {
        let busy = BuildingId(8);
        let idle = BuildingId(9);
        let kind = UnitKind::Harvester;
        let deadline = 600;
        let proposal = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(90),
            anchor: TilePos::new(12, 8),
            deadline,
            case: connected_case(),
            minimum_claims: ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![
                    crate::bot::strategy::ConnectedProviderJob::fixture(
                        kind,
                        120,
                        deadline,
                        vec![busy],
                    ),
                    crate::bot::strategy::ConnectedProviderJob::fixture(
                        kind,
                        120,
                        deadline,
                        vec![idle],
                    ),
                ],
            ),
            marginal_additions: Vec::new(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: kind.stats().cost.saturating_mul(2),
            observed_at: 120,
            horizon: deadline,
            cadence: 12,
            forecast_income: Vec::new(),
            units: Vec::new(),
            builders: Vec::new(),
            producers: vec![
                ProducerPlanningProjection::fixture(
                    busy,
                    120,
                    12,
                    300,
                    vec![120; crate::stats::QUEUE_CAP],
                    vec![kind],
                )
                .expect("the busy producer projection is valid"),
                ProducerPlanningProjection::fixture(
                    idle,
                    120,
                    12,
                    120,
                    vec![120; crate::stats::QUEUE_CAP],
                    vec![kind],
                )
                .expect("the idle producer projection is valid"),
            ],
        })
        .expect("the two-lane resource projection is valid");
        let allocation = CrossDomainAllocation {
            capacity: AllocationCapacity::fixture(resources),
            current_scrap: kind.stats().cost.saturating_mul(2),
            obligations: Vec::new(),
            proposals: vec![
                connected_investment_proposal(proposal.clone())
                    .expect("the connected package has valid claims"),
            ],
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        };
        let settlement = allocation
            .resolve(AllocationPersonality::default(), None)
            .expect("both independent producer lanes fit");

        assert_eq!(
            settlement
                .producer_schedule()
                .iter()
                .map(|job| job.request_ordinal)
                .collect::<Vec<_>>(),
            vec![1, 0],
            "the global schedule is intentionally ordered by actual start time"
        );
        let assignments = connected_producer_assignments(&proposal, settlement.producer_schedule());
        assert_eq!(
            assignments
                .iter()
                .map(|assignment| assignment.request_ordinal())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        let mut bound = proposal;
        bound
            .bind_producer_assignments(assignments)
            .expect("domain binding consumes assignments in request order");
    }

    #[test]
    fn future_connected_job_cannot_consume_the_current_survival_reserve() {
        let producer = BuildingId(9);
        let future_tick = 120;
        let kind = UnitKind::Moth;
        let ready_at = future_tick + Tick::from(kind.stats().train_ticks) - 1;
        let future = imported_obligation(
            ObligationClass::PersistentPlan,
            10,
            ObligationKey::ConnectedOffense {
                objective: BuildingId(90),
                anchor: TilePos::new(12, 8),
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
                    future_tick,
                    future_tick,
                    ready_at,
                    ready_at + 2,
                )],
            )
            .expect("the future connected job is valid"),
        );
        let bank = UnitKind::Sentinel.stats().cost;
        let mut obligations = vec![future];
        let survival = clamped_current_reserve_obligation(
            &obligations,
            bank,
            0,
            0,
            ObligationKey::OpeningCore { sequence: 0 },
            bank,
        )
        .expect("the current survival reserve is valid")
        .expect("the future job must not hide the current bank");
        assert_eq!(survival.claims.current_scrap(), bank);
        obligations.push(survival);

        let resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: bank,
            observed_at: 0,
            horizon: ready_at + 2,
            cadence: 1,
            forecast_income: vec![crate::bot::resources::ForecastAvailability {
                available_at: future_tick,
                amount: kind.stats().cost,
            }],
            units: Vec::new(),
            builders: Vec::new(),
            producers: vec![
                ProducerPlanningProjection::fixture(
                    producer,
                    0,
                    1,
                    0,
                    vec![0; crate::stats::QUEUE_CAP],
                    vec![kind],
                )
                .expect("the future producer is valid"),
            ],
        })
        .expect("the current-plus-future resource projection is valid");
        let settlement = CrossDomainAllocation {
            capacity: AllocationCapacity::fixture(resources),
            current_scrap: bank,
            obligations,
            proposals: Vec::new(),
            contextual_proposals: Vec::new(),
            incompatible_layouts: Vec::new(),
        }
        .resolve(AllocationPersonality::default(), None)
        .expect("current survival and forecast-funded future production are jointly feasible");

        assert_eq!(settlement.residual_current_scrap(), 0);
        assert_eq!(settlement.producer_schedule()[0].current_scrap, 0);
        assert_eq!(
            settlement.producer_schedule()[0].forecast_scrap,
            kind.stats().cost
        );
    }
}
