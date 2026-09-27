//! Connected-operation package derivation, claims, and cross-domain
//! proposals and obligations.

use super::*;

#[cfg(test)]
thread_local! {
    static AIRWORKS_PACKAGE_DERIVATIONS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn airworks_package_derivations() -> usize {
    AIRWORKS_PACKAGE_DERIVATIONS.with(core::cell::Cell::get)
}

/// Production resources and fixed deadline one package derivation sizes
/// against.
#[derive(Debug, Clone, Copy)]
pub(super) struct PackageBasis<'a> {
    /// Frozen identity of the admitted operation a revision resizes.
    pub(super) committed: Option<crate::allocation::ConnectedOffenseKey>,
    pub(super) resources: &'a ConnectedProductionResources,
    pub(super) deadline: Tick,
}

impl PackageBasis<'_> {
    fn preparation(self, context: FreshConnectedDerivationContext<'_>) -> PreparationConstraints {
        PreparationConstraints {
            deadline: self.deadline,
            decision_cadence: context.ev.tuning.cadence,
            protected_forecast_scrap: context.inputs.reserve.forecast,
        }
    }
}

pub(super) fn derive_connected_package_options(
    context: FreshConnectedDerivationContext<'_>,
    basis: PackageBasis<'_>,
    target: &BuildingContact,
) -> Result<ConnectedForcePackageOptions, ConnectedPlanRejection> {
    let AirEvidence {
        obs,
        intel,
        home,
        public_map,
        ..
    } = context.ev;
    if known_ground_connection(
        obs,
        home,
        target.anchor,
        target.kind.base_stats().size,
        public_map,
    ) != Some(true)
    {
        return Err(ConnectedPlanRejection::DisconnectedGroundRoute);
    }
    // The package must refuse the same paid queue work as the resources it
    // is derived against, or it can lean on an occurrence the lowered claims
    // will not be allowed to take.
    let route = context
        .ev
        .route(target.anchor)
        .with_routes(Some(context.campaign_routes))
        .excluding_paid(basis.resources.access.paid_exclusions());
    let mut selected = connected_target_subset(obs, intel, target, &[target.anchor]);
    let mut packages =
        derive_connected_package_options_for_targets(context, basis, target, &selected, route)?;
    if context.minimum_only {
        return Ok(packages);
    }
    for anchor in &basis.resources.targets.growth_order {
        let mut proposed_anchors = selected.target_anchors.clone();
        proposed_anchors.push(*anchor);
        let proposed = connected_target_subset(obs, intel, target, &proposed_anchors);
        if let Ok(proposed_packages) =
            derive_connected_package_options_for_targets(context, basis, target, &proposed, route)
        {
            selected = proposed;
            packages = proposed_packages;
        }
    }
    Ok(packages)
}

fn derive_connected_package_options_for_targets(
    context: FreshConnectedDerivationContext<'_>,
    basis: PackageBasis<'_>,
    target: &BuildingContact,
    targets: &ConnectedTargetSelection,
    route: ConnectedRouteContext<'_>,
) -> Result<ConnectedForcePackageOptions, ConnectedPlanRejection> {
    let AirEvidence { obs, intel, .. } = context.ev;
    let access = connected_production_access(obs, targets, &basis.resources.snapshot, route);
    let unavailable =
        connected_provider_unavailable(obs, targets, context.inputs.unavailable, route);
    let preparation = basis.preparation(context);
    let protected_forecast_scrap = preparation.protected_forecast_scrap.min(
        basis
            .resources
            .snapshot
            .forecast()
            .income_through(preparation.deadline)
            .amount(),
    );
    let cluster =
        sized_target_contacts_at_anchors(intel, target.player, &targets.target_anchors, obs.tick);
    let mut packages = derive_connected_force_package_options_for_cluster(
        context.ev,
        ConnectedTargetEvidence {
            primary: target,
            cluster: &cluster,
            committed: basis.committed,
        },
        ProductionEvidence::with_planning(
            &basis.resources.snapshot,
            &access,
            Some(context.inputs.planning),
        ),
        &unavailable,
        preparation,
        context.minimum_only,
    )
    .map_err(|reason| ConnectedPlanRejection::Package {
        reason,
        protected_current_scrap: context.inputs.reserve.current,
        protected_forecast_scrap,
    })?;
    let mut campaign_routes = BTreeMap::new();
    let mut package_has_routes = |package: &ConnectedForcePackage| {
        let roster: Vec<_> = package
            .suppression
            .iter()
            .map(|demand| (demand.kind, demand.count))
            .collect();
        *campaign_routes.entry(roster).or_insert_with(|| {
            connected_artillery_group_has_staging(
                obs,
                route,
                &package.suppression,
                context.preferred_artillery,
                &unavailable,
            ) && connected_suppression_roster_has_firing_assignments(
                obs,
                route,
                &package.suppression,
                &targets.suppression_targets,
            )
        })
    };
    if !package_has_routes(&packages.minimum) {
        return Err(ConnectedPlanRejection::UnreachableGroupStaging {
            requested: demand_count(&packages.minimum.suppression),
        });
    }
    packages.marginal.truncate(
        packages
            .marginal
            .iter()
            .take_while(|package| package_has_routes(package))
            .count(),
    );
    Ok(packages)
}

fn connected_target_subset(
    obs: &Observation,
    intel: &StrategicIntelligence,
    target: &BuildingContact,
    anchors: &[TilePos],
) -> ConnectedTargetSelection {
    let cluster = sized_target_contacts_at_anchors(intel, target.player, anchors, obs.tick);
    let mut target_anchors: Vec<_> = cluster.iter().map(|contact| contact.anchor).collect();
    target_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
    target_anchors.dedup();
    ConnectedTargetSelection {
        target_anchors,
        suppression_targets: current_cluster_suppression_needs(intel, &cluster).targets,
        growth_order: Vec::new(),
    }
}

fn connected_proposal_claims(
    op: &AirOperation,
    package: &ConnectedForcePackage,
    resources: &ConnectedProductionResources,
    obs: &Observation,
) -> ConnectedOffenseClaims {
    let provider_jobs = package
        .funded_providers
        .iter()
        .map(|provider| ConnectedProviderJob {
            kind: provider.kind,
            enqueue_not_before: provider.command_tick,
            ready_before: package.preparation_deadline,
            eligible_producers: eligible_producers(
                &resources.snapshot,
                &resources.access,
                provider.kind,
                Some(package.preparation_deadline),
            ),
        })
        .collect::<Vec<_>>();
    debug_assert!(
        provider_jobs
            .iter()
            .all(|job| !job.eligible_producers.is_empty()),
        "package funding requires at least one exact preflighted producer per job"
    );
    ConnectedOffenseClaims {
        units: AirRoster::from(op).live_members(&[], obs),
        paid_providers: connected_paid_provider_claims(op, package, resources, obs),
        provider_jobs,
    }
}

fn connected_paid_provider_claims(
    op: &AirOperation,
    package: &ConnectedForcePackage,
    resources: &ConnectedProductionResources,
    obs: &Observation,
) -> Vec<PaidQueueClaim> {
    let mut needed = BTreeMap::<UnitKind, usize>::new();
    for demand in &package.provider_priority {
        let count = needed.entry(demand.kind).or_default();
        *count = count.saturating_add(demand.count);
    }
    for id in op.members() {
        if let Some(member) = unit(obs, id)
            && let Some(count) = needed.get_mut(&member.kind)
        {
            *count = count.saturating_sub(1);
        }
    }
    for provider in &package.funded_providers {
        if let Some(count) = needed.get_mut(&provider.kind) {
            *count = count.saturating_sub(1);
        }
    }

    let mut paid = Vec::new();
    for (kind, count) in needed {
        let producers = paid_queued_ready_occurrences_with_access(
            &resources.snapshot,
            kind,
            package.preparation_deadline,
            &resources.access,
        );
        debug_assert!(
            producers.len() >= count,
            "a derived connected package must retain every paid provider it used"
        );
        paid.extend(
            producers
                .into_iter()
                .take(count)
                .map(|(producer, occurrence)| PaidQueueClaim {
                    producer,
                    kind,
                    occurrence,
                }),
        );
    }
    paid.sort_unstable();
    paid
}

fn claim_additions(
    minimum: &ConnectedOffenseClaims,
    scaled: &ConnectedOffenseClaims,
) -> Option<ConnectedOffenseClaims> {
    if !minimum
        .units
        .iter()
        .all(|unit| scaled.units.binary_search(unit).is_ok())
        || !scaled.provider_jobs.starts_with(&minimum.provider_jobs)
        || !multiset_contains(&scaled.paid_providers, &minimum.paid_providers)
    {
        return None;
    }
    Some(ConnectedOffenseClaims {
        units: scaled
            .units
            .iter()
            .copied()
            .filter(|unit| minimum.units.binary_search(unit).is_err())
            .collect(),
        paid_providers: multiset_difference(&scaled.paid_providers, &minimum.paid_providers),
        provider_jobs: scaled.provider_jobs[minimum.provider_jobs.len()..].to_vec(),
    })
}

fn multiset_contains<T: Ord + Copy>(superset: &[T], subset: &[T]) -> bool {
    multiset_difference(subset, superset).is_empty()
}

fn multiset_difference<T: Ord + Copy>(left: &[T], right: &[T]) -> Vec<T> {
    let mut right_counts = BTreeMap::<T, usize>::new();
    for &item in right {
        let count = right_counts.entry(item).or_default();
        *count = count.saturating_add(1);
    }
    left.iter()
        .copied()
        .filter(|item| {
            let Some(count) = right_counts.get_mut(item).filter(|count| **count > 0) else {
                return true;
            };
            *count -= 1;
            false
        })
        .collect()
}

/// Why a currently considered connected operation could not be admitted or
/// revised. This value is returned only with the current think; it never
/// becomes controller memory or simulation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedPlanRejection {
    InsufficientStandingForce {
        current: usize,
        required: usize,
    },
    DisconnectedGroundRoute,
    UnreachableGroupStaging {
        requested: usize,
    },
    Package {
        reason: ForcePackageRejection,
        protected_current_scrap: u32,
        protected_forecast_scrap: u32,
    },
}

impl ConnectedPlanRejection {
    /// The recovery an admitted operation enters when its revision fails
    /// this way.
    pub(crate) fn recovery_reason(self) -> AirRecoveryReason {
        match self {
            Self::DisconnectedGroundRoute | Self::UnreachableGroupStaging { .. } => {
                AirRecoveryReason::UnreachableStaging
            }
            Self::Package {
                reason: ForcePackageRejection::UntargetableCurrentAirDefense { .. },
                ..
            } => AirRecoveryReason::NewAirDefense,
            Self::Package {
                reason: ForcePackageRejection::TargetNotActionable,
                ..
            } => AirRecoveryReason::ObjectiveLost,
            Self::InsufficientStandingForce { .. } | Self::Package { .. } => {
                AirRecoveryReason::PreparationInfeasible
            }
        }
    }

    pub(crate) fn is_deferred(self) -> bool {
        matches!(
            self,
            Self::Package {
                reason: ForcePackageRejection::Deferred,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RejectedConnectedCandidate {
    pub(crate) target: BuildingContact,
    pub(crate) reason: ConnectedPlanRejection,
}

/// One unpaid provider retained by an exact connected-offense proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectedProviderJob {
    pub(super) kind: UnitKind,
    pub(super) enqueue_not_before: Tick,
    pub(super) ready_before: Tick,
    pub(super) eligible_producers: Vec<BuildingId>,
}

impl ConnectedProviderJob {
    /// Concrete provider selected by the offense domain.
    pub(crate) const fn kind(&self) -> UnitKind {
        self.kind
    }

    /// First command boundary at which the package's funding evidence permits
    /// this provider.
    pub(crate) const fn enqueue_not_before(&self) -> Tick {
        self.enqueue_not_before
    }

    /// Immutable preparation deadline shared by the complete package.
    pub(crate) const fn ready_before(&self) -> Tick {
        self.ready_before
    }

    /// Exact completed producers that passed both production and route
    /// preflight for this provider.
    pub(crate) fn eligible_producers(&self) -> &[BuildingId] {
        &self.eligible_producers
    }
}

/// Atomic shared claims for one exact connected package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ConnectedOffenseClaims {
    pub(super) units: Vec<UnitId>,
    pub(super) paid_providers: Vec<PaidQueueClaim>,
    pub(super) provider_jobs: Vec<ConnectedProviderJob>,
}

impl ConnectedOffenseClaims {
    /// Exact live providers reserved by this package.
    pub(crate) fn units(&self) -> &[UnitId] {
        &self.units
    }

    /// Exact paid queue occurrences that satisfy this package's demand.
    pub(crate) fn paid_providers(&self) -> &[PaidQueueClaim] {
        &self.paid_providers
    }

    /// Exact unpaid production requests retained by this package.
    pub(crate) fn provider_jobs(&self) -> &[ConnectedProviderJob] {
        &self.provider_jobs
    }
}

/// How quickly the currently observed connected opportunity warrants action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedUrgency {
    Developmental,
    Timely,
    Pressing,
}

/// Quality of the evidence supporting a connected operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedConfidence {
    Prior,
    Supported,
    Current,
}

/// Strategic consequence of successfully prosecuting the retained cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedStrategicValue {
    Incremental,
    Material,
    Decisive,
}

/// Time until the retained minimum can begin affecting the objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedTimeToImpact {
    Patient,
    Near,
    Immediate,
}

/// Confidence that the retained minimum can execute against known defenses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedExecutionSafety {
    Speculative,
    Managed,
    Secure,
}

/// Named, fog-honest comparison case for one exact connected opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectedOpportunityCase {
    pub(super) urgency: ConnectedUrgency,
    pub(super) confidence: ConnectedConfidence,
    pub(super) value: ConnectedStrategicValue,
    pub(super) time_to_impact: ConnectedTimeToImpact,
    pub(super) safety: ConnectedExecutionSafety,
}

impl ConnectedOpportunityCase {
    pub(crate) const fn urgency(self) -> ConnectedUrgency {
        self.urgency
    }

    pub(crate) const fn confidence(self) -> ConnectedConfidence {
        self.confidence
    }

    pub(crate) const fn value(self) -> ConnectedStrategicValue {
        self.value
    }

    pub(crate) const fn time_to_impact(self) -> ConnectedTimeToImpact {
        self.time_to_impact
    }

    pub(crate) const fn safety(self) -> ConnectedExecutionSafety {
        self.safety
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedProposalVariant {
    pub(super) op: AirOperation,
    pub(super) plan: ConnectedPlan,
    pub(super) claims: ConnectedOffenseClaims,
}

impl ConnectedProposalVariant {
    pub(super) fn into_active(self) -> ActiveAirOperation {
        ActiveAirOperation {
            op: self.op,
            plan: AirPlan::Connected(Box::new(self.plan)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConnectedProposalOrigin {
    Idle {
        standby: AirStandby,
    },
    Remembered {
        active: ActiveAirOperation,
    },
    Active {
        op: AirOperation,
        plan: Box<ConnectedPlan>,
    },
}

/// One deterministic cumulative addition above an accepted connected minimum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectedMarginalVariant {
    pub(super) variant_index: usize,
    pub(super) additions: ConnectedOffenseClaims,
}

impl ConnectedMarginalVariant {
    /// Cumulative claims added above the common minimum.
    pub(crate) const fn additions(&self) -> &ConnectedOffenseClaims {
        &self.additions
    }
}

/// Pure, exact connected-offense proposal submitted for cross-domain
/// adjudication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshConnectedProposal {
    pub(super) origin: ConnectedProposalOrigin,
    pub(super) variants: Vec<ConnectedProposalVariant>,
    pub(super) marginal: Vec<ConnectedMarginalVariant>,
    pub(super) selected_variant: usize,
    pub(super) case: ConnectedOpportunityCase,
}

impl FreshConnectedProposal {
    pub(crate) fn revises_active_operation(&self) -> bool {
        matches!(self.origin, ConnectedProposalOrigin::Active { .. })
    }

    /// Identity every variant commits to. A fresh proposal takes it from the
    /// objective selected by domain ranking; a revision keeps the identity its
    /// operation was admitted under.
    pub(crate) fn identity(&self) -> crate::allocation::ConnectedOffenseKey {
        self.variants[0].plan.commitment.key()
    }

    /// Fixed deadline shared by the minimum and all marginal variants.
    pub(crate) fn deadline(&self) -> Tick {
        self.variants[0].plan.package.preparation_deadline
    }

    /// Original strategic admission tick retained across remembered
    /// reconnaissance and fresh cross-domain assault adjudication.
    pub(crate) fn accepted_at(&self) -> Tick {
        self.variants[0].plan.commitment.admitted_at
    }

    /// Named evidence and consequence bands used by cross-domain ranking.
    pub(crate) const fn case(&self) -> ConnectedOpportunityCase {
        self.case
    }

    /// Shared claims of the independently admissible common minimum.
    pub(crate) fn minimum_claims(&self) -> &ConnectedOffenseClaims {
        &self.variants[0].claims
    }

    /// Deterministic cumulative additions above the common minimum. Every row
    /// preserves all earlier claims.
    pub(crate) fn marginal_variants(&self) -> &[ConnectedMarginalVariant] {
        &self.marginal
    }

    /// Selects one exact marginal variant previously returned by
    /// [`Self::marginal_variants`].
    pub(crate) fn select_marginal(&mut self, marginal: &ConnectedMarginalVariant) -> bool {
        if self.marginal.get(marginal.variant_index.saturating_sub(1)) != Some(marginal) {
            return false;
        }
        self.selected_variant = marginal.variant_index;
        true
    }
}

/// Mandatory continuation imported into the next allocation pass for an
/// already-admitted connected operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveConnectedObligation {
    pub(crate) membership: AirMembership,
    pub(super) identity: crate::allocation::ConnectedOffenseKey,
    pub(super) accepted_at: Tick,
    pub(super) deadline: Tick,
    pub(super) units: Vec<UnitId>,
    pub(super) provider_jobs: Vec<ConnectedProviderJob>,
}

impl ActiveConnectedObligation {
    pub(crate) const fn identity(&self) -> crate::allocation::ConnectedOffenseKey {
        self.identity
    }

    pub(crate) const fn accepted_at(&self) -> Tick {
        self.accepted_at
    }

    pub(crate) const fn deadline(&self) -> Tick {
        self.deadline
    }

    pub(crate) fn units(&self) -> &[UnitId] {
        &self.units
    }

    pub(crate) fn provider_jobs(&self) -> &[ConnectedProviderJob] {
        &self.provider_jobs
    }
}

#[derive(Clone, Copy)]
pub(super) struct FreshConnectedDerivationContext<'a> {
    pub(super) ev: AirEvidence<'a>,
    pub(super) inputs: ConnectedInputs<'a>,
    pub(super) minimum_only: bool,
    pub(super) campaign_routes: &'a CampaignRoutes<'a>,
    pub(super) preferred_artillery: &'a [UnitId],
}

impl<'a> FreshConnectedDerivationContext<'a> {
    fn route(self, target: TilePos) -> ConnectedRouteContext<'a> {
        self.ev
            .route(target)
            .with_routes(Some(self.campaign_routes))
            .excluding_paid(self.inputs.paid_exclusions)
    }
}

fn connected_opportunity_case(
    observed_at: Tick,
    intel: &StrategicIntelligence,
    target: &BuildingContact,
    targets: &ConnectedTargetSelection,
    minimum: &ConnectedProposalVariant,
) -> ConnectedOpportunityCase {
    let cluster = sized_target_contacts_at_anchors(
        intel,
        target.player,
        &targets.target_anchors,
        observed_at,
    );
    let contains = |kind| cluster.iter().any(|contact| contact.kind == kind);
    let urgency = if contains(BuildingKind::Foundry) {
        ConnectedUrgency::Pressing
    } else if [
        BuildingKind::Airworks,
        BuildingKind::Fabricator,
        BuildingKind::Crucible,
    ]
    .into_iter()
    .any(contains)
    {
        ConnectedUrgency::Timely
    } else {
        ConnectedUrgency::Developmental
    };
    let confidence = match target.evidence {
        ContactEvidence::Current => ConnectedConfidence::Current,
        ContactEvidence::Remembered if target.last_seen.is_some() => ConnectedConfidence::Supported,
        ContactEvidence::Remembered => ConnectedConfidence::Prior,
    };
    let value = if contains(BuildingKind::Foundry) {
        ConnectedStrategicValue::Decisive
    } else if cluster.len() > 1
        || cluster
            .iter()
            .any(|contact| building_value(contact.kind) >= 4)
    {
        ConnectedStrategicValue::Material
    } else {
        ConnectedStrategicValue::Incremental
    };
    let time_to_impact = if minimum.claims.provider_jobs.is_empty() {
        ConnectedTimeToImpact::Immediate
    } else if minimum
        .claims
        .provider_jobs
        .iter()
        .any(|job| job.enqueue_not_before > observed_at)
    {
        ConnectedTimeToImpact::Patient
    } else {
        ConnectedTimeToImpact::Near
    };
    let package = &minimum.plan.package;
    let meets_known_opportunity = package.chosen_capability.suppression
        >= package.useful_capability.suppression
        && package.chosen_capability.strike >= package.useful_capability.strike
        && package.chosen_bombing >= package.useful_bombing;
    let safety = if !meets_known_opportunity {
        ConnectedExecutionSafety::Speculative
    } else if package.observed_aa_firepower == 0 {
        ConnectedExecutionSafety::Secure
    } else {
        ConnectedExecutionSafety::Managed
    };
    ConnectedOpportunityCase {
        urgency,
        confidence,
        value,
        time_to_impact,
        safety,
    }
}

/// Whether a remembered or current structure can anchor a prospective first
/// Airworks campaign.
pub(crate) fn prospective_air_target(contact: &BuildingContact, now: Tick) -> bool {
    contact.built && contact.hp > 0 && contact.confidence_at(now) > 0
}

/// Values a complete, route-serviceable minimum after a proposed first Airworks.
/// The hypothetical producer is confined to sizing; it never becomes an owned claim.
/// Sizing assumes remembered targets still stand as last seen, so each value is
/// discounted by that target's confidence.
#[expect(
    clippy::too_many_arguments,
    reason = "the sizing witness mirrors the exact quote inputs it must respect"
)]
pub(crate) fn prospective_airworks_package_value(
    ev: AirEvidence<'_>,
    reserve: CapitalReserve,
    candidate: crate::observation::BuildingObs,
    candidate_sites: &[TilePos],
    ready_after: Tick,
    fund_by: Tick,
    deadline: Tick,
    obligations: &[crate::allocation::ImportedObligation],
    planning: &crate::planning::PlanningWork,
) -> Option<u64> {
    use crate::allocation::{
        AllocationCapacity, AllocationPersonality, ClaimBundle, DeferrableCapitalClaim,
        ImportedObligation, ObligationClass, ObligationKey, allocate_requiring_planned,
        connected_investment_proposal, current_reserve_at,
    };
    use crate::planning::Progress;
    let site = candidate.anchor;
    if !planning.campaign_site_selected(ev.obs.tick, site, candidate_sites) {
        return None;
    }
    let mut prospective = ev.obs.clone();
    let cost = BuildingKind::Airworks.base_stats().construction?.cost;
    let bank = prospective.scrap.saturating_sub(reserve.current);
    let paid_now = bank.min(cost);
    let shortfall = cost - paid_now;
    prospective.scrap = bank - paid_now;
    prospective.my_buildings.push(candidate);
    prospective.my_queues.push(Vec::new());
    prospective.my_queue_progress.push(0);
    let resources = ResourceSnapshot::from_observation(&prospective);
    // The sizing bank excludes current promises; exact allocation imports them itself.
    let restored = reserve
        .current
        .min(current_reserve_at(obligations, prospective.tick))
        .min(ev.obs.scrap - bank);
    prospective.scrap += restored;
    let capacity = AllocationCapacity::from_snapshot(
        &ResourceSnapshot::from_observation(&prospective),
        deadline,
        ev.tuning.cadence,
    )
    .ok()?;
    // Capital the bank cannot cover now is owed from forecast by the factory's
    // funding deadline, exactly as the proposed saving will be charged.
    let mut obligations = obligations.to_vec();
    if shortfall > 0 {
        obligations.push(ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: prospective.tick,
            key: ObligationKey::SavedEconomy(crate::utility::EconomicInvestmentKey::Build {
                kind: BuildingKind::Airworks,
                anchor: site,
            }),
            claims: ClaimBundle::new(
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .ok()?
            .with_deferrable_capital(DeferrableCapitalClaim {
                through: fund_by,
                amount: shortfall,
            })
            .ok()?,
        });
    }
    // A new campaign cannot make retained obligations fit after buying its factory.
    if !matches!(
        crate::allocation::forecast::refine_obligations(&capacity, &obligations, planning),
        crate::planning::Progress::Ready(())
    ) {
        return None;
    }
    prospective.scrap -= restored;
    let unavailable: Vec<_> = prospective.my_units.iter().map(|unit| unit.id).collect();
    let intel = ev.intel.assuming_remembered_buildings(prospective.tick);
    let campaign_routes = CampaignRoutes::new(&prospective, &intel, ev.public_map, ev.orientation);
    let context = FreshConnectedDerivationContext {
        ev: AirEvidence {
            obs: &prospective,
            intel: &intel,
            ..ev
        },
        inputs: ConnectedInputs {
            planning,
            resources: &resources,
            unavailable: &unavailable,
            paid_exclusions: &[],
            reserve: CapitalReserve {
                current: 0,
                forecast: reserve.forecast.saturating_add(shortfall),
            },
        },
        minimum_only: true,
        campaign_routes: &campaign_routes,
        preferred_artillery: &[],
    };
    let deadline = deadline.checked_sub(ready_after)?;
    if deadline <= prospective.tick {
        return None;
    }
    let confidence = |target: &BuildingContact| {
        ev.intel
            .buildings()
            .iter()
            .find(|remembered| {
                remembered.player == target.player && remembered.anchor == target.anchor
            })
            .map_or(0, |remembered| remembered.confidence_at(prospective.tick))
    };
    let mut targets: Vec<_> = intel
        .buildings()
        .iter()
        .filter(|target| target.built && target.hp > 0 && confidence(target) > 0)
        .collect();
    let distances = ev.public_map.map(|map| map.regions().distances(ev.home));
    targets.sort_by_key(|target| {
        (
            std::cmp::Reverse(
                u64::from(target.hp)
                    * u64::from(building_value(target.kind))
                    * u64::from(confidence(target)),
            ),
            distances
                .as_ref()
                .and_then(|distances| distances.estimate(target.anchor))
                .unwrap_or(u32::MAX),
            target.anchor.y,
            target.anchor.x,
            target.id,
        )
    });
    let keys: Vec<_> = targets
        .iter()
        .map(|target| (target.player, target.anchor, target.id))
        .collect();
    let result = planning.campaign_candidate(prospective.tick, site, &keys, |key| {
        let target = targets
            .iter()
            .find(|target| (target.player, target.anchor, target.id) == key)
            .unwrap();
        #[cfg(test)]
        AIRWORKS_PACKAGE_DERIVATIONS.with(|count| count.set(count.get() + 1));
        let derived = derive_connected_proposal_with_resources(
            context,
            target,
            current_target_cluster(&intel, target.player, target.anchor),
            ConnectedProposalOrigin::Idle {
                standby: AirStandby::default(),
            },
            deadline,
        );
        let proposal = match derived {
            Ok(proposal) => proposal,
            Err(ConnectedPlanRejection::Package {
                reason: ForcePackageRejection::Deferred,
                ..
            }) => return Progress::Deferred,
            Err(_) => return Progress::ProvenInfeasible,
        };
        let investment = connected_investment_proposal(proposal.clone());
        match allocate_requiring_planned(
            &capacity,
            obligations.clone(),
            vec![investment.clone()],
            AllocationPersonality::default(),
            investment.key(),
            planning,
        ) {
            Ok(Some(_)) => {}
            Ok(None) => return Progress::Deferred,
            Err(_) => return Progress::ProvenInfeasible,
        }
        let package_cost: u64 = proposal
            .minimum_claims()
            .provider_jobs()
            .iter()
            .map(|job| u64::from(job.kind().stats().cost))
            .sum();
        Progress::Ready(
            package_cost * u64::from(confidence(target))
                / u64::from(crate::intelligence::MAX_CONFIDENCE),
        )
    });
    match result {
        Progress::Ready(value) => Some(value),
        Progress::Deferred | Progress::Exhausted | Progress::ProvenInfeasible => None,
    }
}

/// Sizes a connected proposal against `target` with `candidates` as the
/// members its production resources may serve.
pub(super) fn derive_connected_proposal_with_resources(
    context: FreshConnectedDerivationContext<'_>,
    target: &BuildingContact,
    candidates: Vec<&BuildingContact>,
    origin: ConnectedProposalOrigin,
    deadline: Tick,
) -> Result<FreshConnectedProposal, ConnectedPlanRejection> {
    let FreshConnectedDerivationContext {
        ev: AirEvidence { obs, intel, .. },
        inputs:
            ConnectedInputs {
                resources: resource_snapshot,
                unavailable,
                reserve,
                ..
            },
        ..
    } = context;
    let route = context.route(target.anchor);
    let initial_resources = ConnectedProductionResources::from_candidates(
        obs,
        target,
        candidates,
        unavailable,
        route,
        resource_snapshot,
        reserve.current,
    );
    let committed = match &origin {
        ConnectedProposalOrigin::Active { plan, .. } => Some(plan.commitment.key()),
        ConnectedProposalOrigin::Idle { .. } | ConnectedProposalOrigin::Remembered { .. } => None,
    };
    let packages = derive_connected_package_options(
        context,
        PackageBasis {
            committed,
            resources: &initial_resources,
            deadline,
        },
        target,
    )?;
    if packages.refinement_pending && committed.is_some() {
        return Err(ConnectedPlanRejection::Package {
            reason: ForcePackageRejection::Deferred,
            protected_current_scrap: reserve.current,
            protected_forecast_scrap: reserve.forecast,
        });
    }
    let resources = ConnectedProductionResources::from_package_snapshot_after_current_reserve(
        obs,
        target.player,
        &packages.minimum,
        route,
        resource_snapshot,
        reserve.current,
    );
    let commitment = match &origin {
        ConnectedProposalOrigin::Idle { .. } => {
            ConnectedCommitment::admit(target, &packages.minimum, obs.tick)
        }
        ConnectedProposalOrigin::Remembered { active } => {
            ConnectedCommitment::admit(target, &packages.minimum, active.plan.admitted_at())
        }
        ConnectedProposalOrigin::Active { plan, .. } => plan.commitment.clone(),
    };
    let route_unavailable =
        connected_provider_unavailable(obs, &resources.targets, unavailable, route);
    let mut variants = Vec::with_capacity(packages.marginal.len().saturating_add(1));
    for package in std::iter::once(packages.minimum).chain(packages.marginal) {
        let mut plan = ConnectedPlan::new(commitment.clone(), package);
        // The sizing primary is the focus whenever the focus is in current
        // sight, and otherwise the member the focus moves to.
        plan.focus = target.anchor;
        let mut op = connected_proposal_operation(&origin, target, obs.tick);
        let package = &plan.package;
        let previous_scout = op.scout;
        let previous_artillery = op.artillery.clone();
        let previous_strike_aircraft = op.strike_aircraft.clone();
        let mut scouts = op.scout.into_iter().collect::<Vec<_>>();
        assign_provider_demands(&mut scouts, &package.recon, obs, &route_unavailable);
        op.scout = scouts.into_iter().next();
        assign_provider_demands(
            &mut op.artillery,
            &package.suppression,
            obs,
            &route_unavailable,
        );
        assign_provider_demands(
            &mut op.strike_aircraft,
            &package.strike,
            obs,
            &route_unavailable,
        );
        invalidate_reassigned_member_orders(
            &mut op,
            previous_scout,
            &previous_artillery,
            &previous_strike_aircraft,
        );
        let claims = connected_proposal_claims(&op, package, &resources, obs);
        variants.push(ConnectedProposalVariant { op, plan, claims });
    }
    let minimum_claims = variants[0].claims.clone();
    let marginal = variants
        .iter()
        .enumerate()
        .skip(1)
        .map(|(variant_index, variant)| ConnectedMarginalVariant {
            variant_index,
            additions: claim_additions(&minimum_claims, &variant.claims)
                .expect("marginal package variants only add to their exact minimum"),
        })
        .collect();
    let case =
        connected_opportunity_case(obs.tick, intel, target, &resources.targets, &variants[0]);
    Ok(FreshConnectedProposal {
        origin,
        variants,
        marginal,
        selected_variant: 0,
        case,
    })
}

pub(super) fn invalidate_reassigned_member_orders(
    op: &mut AirOperation,
    previous_scout: Option<UnitId>,
    previous_artillery: &[UnitId],
    previous_strike_aircraft: &[UnitId],
) {
    if op.scout != previous_scout {
        op.scout_dispatch = None;
    }
    if op.artillery != previous_artillery {
        op.artillery_staging = None;
    }
    if op.strike_aircraft != previous_strike_aircraft {
        op.strike_hold = None;
    }
}

fn connected_proposal_operation(
    origin: &ConnectedProposalOrigin,
    target: &BuildingContact,
    admitted_at: Tick,
) -> AirOperation {
    match origin {
        ConnectedProposalOrigin::Idle { standby, .. } => AirOperation {
            target_player: target.player,
            target_kind: target.kind,
            target: target.anchor,
            target_id: target.id,
            stage: AirStage::Recon,
            started_at: admitted_at,
            phase_started_at: admitted_at,
            scout: standby.scout,
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: standby.artillery.clone(),
            strike_aircraft: standby.strike_aircraft.clone(),
            strike_issued_at: None,
            membership_frozen_at: None,
        },
        ConnectedProposalOrigin::Remembered { active } => {
            let mut op = active.op.clone();
            op.target_player = target.player;
            op.target_kind = target.kind;
            op.target = target.anchor;
            op.target_id = target.id;
            op.admit_assault(admitted_at);
            op
        }
        ConnectedProposalOrigin::Active { op, .. } => op.clone(),
    }
}
