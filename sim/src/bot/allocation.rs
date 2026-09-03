//! Deterministic portfolio selection across the first migrated strategy domains.
//!
//! Domain planners submit already-legal, exact proposals. This module owns only
//! cross-domain compatibility and selection: it imports prior obligations,
//! proves that shared claims fit together, and returns the original payloads of
//! the best compatible portfolio without asking either domain to plan twice.

use core::cmp::{Ordering, Reverse};
use std::collections::BTreeSet;

use super::resources::{
    PlanningProjectionError, ProducerLaneReservations, ProducerPlanningProjection,
    ReservedProducerJob, ResourcePlanningProjection, ResourceSnapshot, SiteFootprint,
};
use crate::ids::{BuildingId, UnitId};
use crate::stats::UnitKind;
use chassis::Tick;
use chassis::grid::TilePos;

mod adapters;
mod coordinator;
mod session;

pub(crate) use adapters::*;
pub(crate) use coordinator::*;
pub(crate) use session::*;

const BASE_PERSONALITY_WEIGHT: u128 = 100;

/// Stable identity of one fresh Foundry opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoundryExpansionKey {
    /// Exact proposed Foundry anchor.
    pub(crate) anchor: TilePos,
}

impl Ord for FoundryExpansionKey {
    fn cmp(&self, other: &Self) -> Ordering {
        tile_key(self.anchor).cmp(&tile_key(other.anchor))
    }
}

impl PartialOrd for FoundryExpansionKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Stable identity of one connected-offense opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectedOffenseKey {
    /// Current primary building that anchors the admitted target cluster.
    pub(crate) objective: BuildingId,
    /// Current row-major anchor of the primary objective.
    pub(crate) anchor: TilePos,
}

impl Ord for ConnectedOffenseKey {
    fn cmp(&self, other: &Self) -> Ordering {
        (tile_key(self.anchor), self.objective).cmp(&(tile_key(other.anchor), other.objective))
    }
}

impl PartialOrd for ConnectedOffenseKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Stable identity used for canonical proposal order and decision traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProposalKey {
    /// One fresh Foundry opportunity.
    FoundryExpansion(FoundryExpansionKey),
    /// One minimum viable connected operation.
    ConnectedOffenseMinimum(ConnectedOffenseKey),
}

impl ProposalKey {
    fn same_domain(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::FoundryExpansion(_), Self::FoundryExpansion(_))
                | (
                    Self::ConnectedOffenseMinimum(_),
                    Self::ConnectedOffenseMinimum(_)
                )
        )
    }
}

/// How soon the observed situation calls for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Urgency {
    /// Useful long-term development with no immediate pressure.
    Developmental,
    /// A current opportunity or concern that should not drift indefinitely.
    Timely,
    /// An immediate threat or unusually perishable opportunity.
    Pressing,
}

/// Strength of the fog-honest evidence behind a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confidence {
    /// Public-map knowledge or a remembered prior supports the proposal.
    Prior,
    /// Multiple current or remembered observations support the proposal.
    Supported,
    /// Current direct observation supports the proposal.
    Current,
}

/// Strategic consequence if the proposal succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StrategicValue {
    /// Improves the position without changing its basic shape.
    Incremental,
    /// Creates or protects a meaningful strategic advantage.
    Material,
    /// Can decide the current strategic contest.
    Decisive,
}

/// Delay before the proposal can materially affect the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeToImpact {
    /// Pays off beyond the allocator's immediate tactical window.
    Patient,
    /// Can affect the next planned contest.
    Near,
    /// Can affect the current contest immediately.
    Immediate,
}

/// Confidence that the proposal can be executed without losing its investment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionSafety {
    /// Material route, exposure, or counterplay risks remain unresolved.
    Speculative,
    /// Known risks have a credible mitigation.
    Managed,
    /// Current evidence supports a protected execution path.
    Secure,
}

/// Coarse, domain-independent case used to compare unlike investments.
///
/// Each domain translates its own evidence into these named bands. Raw Foundry
/// yield and target hit points therefore never masquerade as comparable units.
/// Personality may decide only when every semantic-band histogram ties; those
/// deliberately coarse ties are the allocator's explicit near-tie boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProposalCase {
    /// Time pressure behind the proposal.
    pub(crate) urgency: Urgency,
    /// Quality and freshness of its supporting evidence.
    pub(crate) confidence: Confidence,
    /// Strategic consequence of successful execution.
    pub(crate) value: StrategicValue,
    /// Delay before the investment changes the position.
    pub(crate) time_to_impact: TimeToImpact,
    /// Execution risk under current knowledge.
    pub(crate) safety: ExecutionSafety,
}

/// Positive personality emphasis for the two migrated domains.
///
/// Preferences add to a fixed base weight, so no seed can remove either domain
/// from consideration. They are consulted only after every semantic band ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AllocationPersonality {
    /// Greed-derived emphasis on economic investment.
    pub(crate) economy: u16,
    /// Air- and siege-derived emphasis on connected offense.
    pub(crate) offense: u16,
}

impl AllocationPersonality {
    fn weight(self, proposal: ProposalKey) -> u128 {
        let preference = match proposal {
            ProposalKey::FoundryExpansion(_) => self.economy,
            ProposalKey::ConnectedOffenseMinimum(_) => self.offense,
        };
        BASE_PERSONALITY_WEIGHT + u128::from(preference)
    }
}

/// Non-production capital that must be fundable by one fixed deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ForecastClaim {
    /// Fixed deadline that bounded the owning proposal.
    pub(crate) through: Tick,
    /// Future income reserved in addition to current-bank scrap.
    pub(crate) amount: u32,
}

/// Non-production capital whose current-versus-forecast funding split is
/// assigned jointly with producer payments.
///
/// The owning domain has already frozen the total amount and last safe
/// deadline. Allocation may only choose which part comes from the observed
/// bank and which part comes from conservative income through that deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeferrableCapitalClaim {
    /// Fixed last tick by which the capital must be available.
    pub(crate) through: Tick,
    /// Exact total capital retained by the domain.
    pub(crate) amount: u32,
}

/// One fixed-horizon production request.
///
/// Fresh proposals may name every exact producer that passed their access
/// preflight. Selection jointly assigns those requests so two independently
/// reasonable plans cannot both consume the same lane. These are future,
/// unpaid `Train` commands; work already present in the observed paid queue is
/// represented by the resource projection and must not be claimed again. An
/// imported obligation uses [`Self::fixed`] to retain the exact lane and timing
/// it already owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProducerJobClaim {
    kind: UnitKind,
    enqueue_not_before: Tick,
    ready_before: Tick,
    access: ProducerJobAccess,
}

/// Whether allocation may choose a lane or must preserve an accepted one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProducerJobAccess {
    /// Canonical producer set that passed fresh proposal preflight.
    Flexible(Vec<BuildingId>),
    /// Exact lane and timing retained by a typed persistent plan.
    Fixed(FixedProducerJob),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FixedProducerJob {
    producer: BuildingId,
    enqueued_at: Tick,
    starts_at: Tick,
    ready_at: Tick,
}

impl ProducerJobAccess {
    fn producers(&self) -> &[BuildingId] {
        match self {
            Self::Flexible(producers) => producers,
            Self::Fixed(job) => core::slice::from_ref(&job.producer),
        }
    }
}

impl ProducerJobClaim {
    /// Creates a fresh request over a canonical set of preflighted producers.
    pub(crate) fn flexible(
        kind: UnitKind,
        enqueue_not_before: Tick,
        ready_before: Tick,
        mut eligible_producers: Vec<BuildingId>,
    ) -> Self {
        eligible_producers.sort_unstable();
        eligible_producers.dedup();
        Self {
            kind,
            enqueue_not_before,
            ready_before,
            access: ProducerJobAccess::Flexible(eligible_producers),
        }
    }

    /// Retains an accepted job's exact lane and FIFO timing.
    ///
    /// Funding is observation-relative: forecast income supporting an accepted
    /// job becomes current bank once it is paid, so each allocation pass
    /// reattributes the unchanged total cost from its fresh resource snapshot.
    pub(crate) fn fixed(
        producer: BuildingId,
        kind: UnitKind,
        enqueued_at: Tick,
        starts_at: Tick,
        ready_at: Tick,
        ready_before: Tick,
    ) -> Self {
        Self {
            kind,
            enqueue_not_before: enqueued_at,
            ready_before,
            access: ProducerJobAccess::Fixed(FixedProducerJob {
                producer,
                enqueued_at,
                starts_at,
                ready_at,
            }),
        }
    }

    /// Unit requested by this exact production claim.
    pub(crate) const fn kind(&self) -> UnitKind {
        self.kind
    }

    /// First decision tick on which the request may enter a queue.
    pub(crate) const fn enqueue_not_before(&self) -> Tick {
        self.enqueue_not_before
    }

    /// Observation deadline, strictly after the unit must be ready.
    pub(crate) const fn ready_before(&self) -> Tick {
        self.ready_before
    }

    /// Canonical producers that passed the owning domain's preflight.
    pub(crate) fn eligible_producers(&self) -> &[BuildingId] {
        self.access.producers()
    }

    /// Exact producer retained by an imported obligation, when applicable.
    #[cfg(test)]
    pub(crate) const fn committed_producer(&self) -> Option<BuildingId> {
        match self.access {
            ProducerJobAccess::Flexible(_) => None,
            ProducerJobAccess::Fixed(job) => Some(job.producer),
        }
    }

    fn fixed_assignment(&self) -> Option<FixedProducerJob> {
        match self.access {
            ProducerJobAccess::Fixed(job) => Some(job),
            ProducerJobAccess::Flexible(_) => None,
        }
    }

    pub(crate) fn fixed_timing(&self) -> Option<(BuildingId, Tick, Tick, Tick)> {
        self.fixed_assignment().map(|fixed| {
            (
                fixed.producer,
                fixed.enqueued_at,
                fixed.starts_at,
                fixed.ready_at,
            )
        })
    }
}

/// Canonical resources against which obligations and proposals compete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AllocationCapacity {
    resources: ResourcePlanningProjection,
}

impl AllocationCapacity {
    /// Builds the sole production capacity model from authoritative resources.
    pub(crate) fn from_snapshot(
        resources: &ResourceSnapshot,
        forecast_horizon: Tick,
        decision_cadence: Tick,
    ) -> Result<Self, PlanningProjectionError> {
        Ok(Self {
            resources: resources.planning_projection(forecast_horizon, decision_cadence)?,
        })
    }

    fn forecast_through(&self, deadline: Tick) -> u64 {
        self.resources.forecast_through(deadline)
    }

    fn producer(&self, id: BuildingId) -> Option<&ProducerPlanningProjection> {
        self.resources.producer(id)
    }

    #[cfg(test)]
    fn fixture(resources: ResourcePlanningProjection) -> Self {
        Self { resources }
    }
}

/// A malformed exact claim bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimBundleError {
    /// The unit roster repeats an exact force member.
    DuplicateUnit(UnitId),
    /// The builder roster repeats an exact builder.
    DuplicateBuilder(UnitId),
    /// One actor is claimed as both a builder and a force member.
    ActorRoleOverlap(UnitId),
    /// Two footprints inside one supposedly atomic plan overlap.
    OverlappingSites {
        /// First canonical footprint.
        first: SiteFootprint,
        /// Second canonical footprint.
        second: SiteFootprint,
    },
    /// Same-deadline future-capital rows overflow the simulation scrap type.
    ForecastScrapOverflow(Tick),
    /// A bundle mixed a flexible capital claim with an already-fixed split.
    MixedCapitalFunding,
    /// A bundle attempted to carry more than one flexible capital deadline.
    DuplicateDeferrableCapital,
}

/// Every shared resource required by one exact proposal or prior obligation.
///
/// `current_scrap` and `forecast_scrap` describe non-production capital such as
/// construction or a protected reserve. Every producer job is charged exactly
/// once from [`UnitKind`] by the joint scheduler and must not be duplicated in
/// either capital field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ClaimBundle {
    current_scrap: u32,
    forecast_scrap: Vec<ForecastClaim>,
    deferrable_capital: Option<DeferrableCapitalClaim>,
    builders: Vec<UnitId>,
    units: Vec<UnitId>,
    sites: Vec<SiteFootprint>,
    producer_jobs: Vec<ProducerJobClaim>,
}

impl ClaimBundle {
    /// Canonicalizes one atomic set of exact claims.
    pub(crate) fn new(
        current_scrap: u32,
        mut forecast_scrap: Vec<ForecastClaim>,
        mut builders: Vec<UnitId>,
        mut units: Vec<UnitId>,
        mut sites: Vec<SiteFootprint>,
        producer_jobs: Vec<ProducerJobClaim>,
    ) -> Result<Self, ClaimBundleError> {
        builders.sort_unstable();
        if let Some(duplicate) = first_duplicate(&builders) {
            return Err(ClaimBundleError::DuplicateBuilder(duplicate));
        }
        units.sort_unstable();
        if let Some(duplicate) = first_duplicate(&units) {
            return Err(ClaimBundleError::DuplicateUnit(duplicate));
        }
        if let Some(&overlap) = builders
            .iter()
            .find(|builder| units.binary_search(builder).is_ok())
        {
            return Err(ClaimBundleError::ActorRoleOverlap(overlap));
        }

        forecast_scrap.retain(|claim| claim.amount > 0);
        forecast_scrap.sort_unstable_by_key(|claim| claim.through);
        let mut canonical_forecast = Vec::<ForecastClaim>::new();
        for claim in forecast_scrap {
            if let Some(prior) = canonical_forecast
                .last_mut()
                .filter(|prior| prior.through == claim.through)
            {
                prior.amount = prior
                    .amount
                    .checked_add(claim.amount)
                    .ok_or(ClaimBundleError::ForecastScrapOverflow(claim.through))?;
            } else {
                canonical_forecast.push(claim);
            }
        }
        sites.sort_unstable();
        for (index, &first) in sites.iter().enumerate() {
            if let Some(&second) = sites
                .iter()
                .skip(index + 1)
                .find(|&&other| first.overlaps(other))
            {
                return Err(ClaimBundleError::OverlappingSites { first, second });
            }
        }

        Ok(Self {
            current_scrap,
            forecast_scrap: canonical_forecast,
            deferrable_capital: None,
            builders,
            units,
            sites,
            producer_jobs,
        })
    }

    pub(crate) fn claimed_capital(&self) -> u128 {
        self.deferrable_capital.map_or_else(
            || {
                u128::from(self.current_scrap)
                    + self
                        .forecast_scrap
                        .iter()
                        .map(|claim| u128::from(claim.amount))
                        .sum::<u128>()
            },
            |claim| u128::from(claim.amount),
        ) + self
            .producer_jobs
            .iter()
            .map(|job| u128::from(job.kind.stats().cost))
            .sum::<u128>()
    }

    /// Non-production capital required from the current bank.
    pub(crate) const fn current_scrap(&self) -> u32 {
        self.current_scrap
    }

    /// Canonical future non-production capital claims.
    pub(crate) fn forecast_scrap(&self) -> &[ForecastClaim] {
        &self.forecast_scrap
    }

    /// Capital whose final bank-versus-income split remains allocator-owned.
    pub(crate) const fn deferrable_capital(&self) -> Option<DeferrableCapitalClaim> {
        self.deferrable_capital
    }

    /// Converts this bundle's fixed capital input into one allocator-owned
    /// deadline without changing actors, sites, or producer work.
    pub(crate) fn with_deferrable_capital(
        mut self,
        claim: DeferrableCapitalClaim,
    ) -> Result<Self, ClaimBundleError> {
        if self.deferrable_capital.is_some() {
            return Err(ClaimBundleError::DuplicateDeferrableCapital);
        }
        if self.current_scrap > 0 || !self.forecast_scrap.is_empty() {
            return Err(ClaimBundleError::MixedCapitalFunding);
        }
        if claim.amount > 0 {
            self.deferrable_capital = Some(claim);
        }
        Ok(self)
    }

    fn bind_deferrable_capital(&mut self, assignment: CapitalFundingAssignment) {
        let prior_total = self.deferrable_capital.map_or_else(
            || {
                self.current_scrap.saturating_add(
                    self.forecast_scrap
                        .iter()
                        .map(|claim| claim.amount)
                        .fold(0, u32::saturating_add),
                )
            },
            |claim| {
                assert_eq!(claim.through, assignment.through);
                claim.amount
            },
        );
        assert_eq!(
            prior_total,
            assignment
                .current_scrap
                .saturating_add(assignment.forecast_scrap)
        );
        self.deferrable_capital = None;
        self.current_scrap = assignment.current_scrap;
        self.forecast_scrap = (assignment.forecast_scrap > 0)
            .then_some(ForecastClaim {
                through: assignment.through,
                amount: assignment.forecast_scrap,
            })
            .into_iter()
            .collect();
    }

    /// Canonical exact builders owned by this bundle.
    pub(crate) fn builders(&self) -> &[UnitId] {
        &self.builders
    }

    /// Canonical exact non-builder units owned by this bundle.
    pub(crate) fn units(&self) -> &[UnitId] {
        &self.units
    }

    /// Canonical exact construction footprints owned by this bundle.
    pub(crate) fn sites(&self) -> &[SiteFootprint] {
        &self.sites
    }

    /// Ordered future production requests owned by this bundle.
    pub(crate) fn producer_jobs(&self) -> &[ProducerJobClaim] {
        &self.producer_jobs
    }
}

/// A proposal and the exact domain token to commit if it wins allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InvestmentProposal<FoundryPayload, OffensePayload> {
    /// One safe, legal Foundry candidate.
    FoundryExpansion {
        /// Stable opportunity identity.
        key: FoundryExpansionKey,
        /// Named evidence and consequence bands.
        case: ProposalCase,
        /// Exact shared-resource claims.
        claims: ClaimBundle,
        /// Exact domain plan, retained without recomputation.
        payload: FoundryPayload,
    },
    /// One minimum viable connected operation.
    ConnectedOffenseMinimum {
        /// Stable opportunity identity.
        key: ConnectedOffenseKey,
        /// Named evidence and consequence bands.
        case: ProposalCase,
        /// Original admission tick, retained when reconnaissance promotes to
        /// an assault proposal on a later allocation pass.
        accepted_at: Tick,
        /// Exact shared-resource claims.
        claims: ClaimBundle,
        /// Exact domain plan, retained without recomputation.
        payload: OffensePayload,
    },
}

impl<FoundryPayload, OffensePayload> InvestmentProposal<FoundryPayload, OffensePayload> {
    /// Stable structural identity.
    pub(crate) const fn key(&self) -> ProposalKey {
        match self {
            Self::FoundryExpansion { key, .. } => ProposalKey::FoundryExpansion(*key),
            Self::ConnectedOffenseMinimum { key, .. } => ProposalKey::ConnectedOffenseMinimum(*key),
        }
    }

    /// Named comparison case supplied by the owning domain.
    pub(crate) const fn case(&self) -> ProposalCase {
        match self {
            Self::FoundryExpansion { case, .. } | Self::ConnectedOffenseMinimum { case, .. } => {
                *case
            }
        }
    }

    /// Shared resources required by this proposal.
    pub(crate) const fn claims(&self) -> &ClaimBundle {
        match self {
            Self::FoundryExpansion { claims, .. }
            | Self::ConnectedOffenseMinimum { claims, .. } => claims,
        }
    }

    /// Original domain admission, or this allocation tick for a genuinely
    /// fresh proposal that has no earlier lifecycle.
    fn accepted_at(&self, observed_at: Tick) -> Tick {
        match self {
            Self::FoundryExpansion { .. } => observed_at,
            Self::ConnectedOffenseMinimum { accepted_at, .. } => *accepted_at,
        }
    }
}

/// Why a mandatory claim or selectable proposal could not fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AllocationConflict {
    /// A marginal claim names a proposal that was not selected.
    InactiveProposal(ProposalKey),
    /// Current-bank claims exceed observed spendable scrap.
    CurrentScrap {
        /// Total requested current scrap.
        requested: u64,
        /// Observed current-bank capacity.
        available: u32,
    },
    /// Forecast claims exceed completed-source income available by a deadline.
    ForecastScrap {
        /// First deadline at which demand exceeds supply.
        through: Tick,
        /// Cumulative forecast demand through that deadline.
        requested: u64,
        /// Cumulative completed-source income through that deadline.
        available: u64,
    },
    /// A claim extends beyond the deliberately bounded forecast.
    ForecastHorizon {
        /// Requested claim deadline.
        through: Tick,
        /// Last tick covered by the forecast.
        horizon: Tick,
    },
    /// A proposed force member is absent from the current own-unit roster.
    UnknownUnit(UnitId),
    /// A proposed builder is not currently an available exact builder.
    UnknownBuilder(UnitId),
    /// Two owners claim the same actor in different or identical roles.
    Actor {
        /// Contested actor.
        unit: UnitId,
        /// Owner that already holds the actor.
        existing: ClaimOwner,
    },
    /// Two owners claim overlapping construction footprints.
    Site {
        /// Requested footprint.
        requested: SiteFootprint,
        /// Existing conflicting footprint.
        existing: SiteFootprint,
        /// Owner of the existing footprint.
        owner: ClaimOwner,
    },
    /// A requested producer is absent from current completed capacity.
    UnknownProducer(BuildingId),
    /// No named producer can train and deliver the exact requested kind.
    ProducerAccess {
        /// Exact requested kind.
        kind: UnitKind,
        /// Canonical preflighted producer set supplied by the domain.
        eligible_producers: Vec<BuildingId>,
    },
    /// A prior obligation did not retain one exact producer assignment.
    UnassignedObligationJob {
        /// Exact obligation owner.
        owner: ClaimOwner,
        /// Exact requested kind.
        kind: UnitKind,
    },
    /// No ordering of fresh jobs can preserve every prior claim and deadline.
    ProducerSchedule {
        /// Producers participating in the impossible joint schedule.
        producers: Vec<BuildingId>,
        /// Owners participating in the impossible lane schedule.
        owners: Vec<ClaimOwner>,
    },
    /// Production costs cannot be paid by their latest legal start ticks.
    ProductionFunding {
        /// First tick at which required production spending exceeds funding.
        through: Tick,
        /// Current capital, future capital, and production due through the tick.
        requested: u128,
        /// Current bank plus completed-source income spendable through the tick.
        available: u128,
    },
}

/// Priority class of already-accepted work imported before fresh selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ObligationClass {
    /// Immediate survival or protected ordinary-core work.
    Survival,
    /// Construction or accepted future production that has already been paid.
    PaidWork,
    /// A previously accepted domain plan that still owns its resources.
    PersistentPlan,
    /// Explicit adapter for a not-yet-migrated controller channel.
    Legacy,
}

/// Unmigrated controller channel protected by an explicit adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LegacyChannel {
    /// Units already enlisted by the Executive's standing army.
    StandingArmy,
    /// Team-role relief package.
    TeamRelief,
    /// Airlift operation.
    Lift,
    /// Harassment or resource raid.
    Raid,
    /// Already-admitted air operation without a connected force package.
    StrategicAir,
    /// Operation-driven Airworks construction.
    AirworksCapacity,
}

impl LegacyChannel {
    const fn sort_key(self) -> i32 {
        match self {
            Self::StandingArmy => 0,
            Self::TeamRelief => 1,
            Self::Lift => 2,
            Self::Raid => 3,
            Self::StrategicAir => 4,
            Self::AirworksCapacity => 5,
        }
    }
}

/// Stable typed identity of one imported obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObligationKey {
    /// One protected opening- or recovery-core tranche.
    OpeningCore { sequence: u16 },
    /// One already-paid construction site.
    PaidConstruction(BuildingId),
    /// One observed builder occupation that owns no construction footprint.
    ObservedBuilderWork {
        /// Exact occupied builder.
        builder: UnitId,
    },
    /// One accepted deferred foundation.
    DeferredFoundation {
        /// Exact builder already carrying the promise.
        builder: UnitId,
        /// Exact promised anchor.
        anchor: TilePos,
    },
    /// One accepted but not yet dispatched Foundry plan.
    SavedFoundry { anchor: TilePos },
    /// One already-active connected operation.
    ConnectedOffense {
        /// Exact primary objective when admitted.
        objective: BuildingId,
        /// Exact objective anchor when admitted.
        anchor: TilePos,
    },
    /// One explicit not-yet-migrated owner.
    Legacy {
        /// Strategic channel behind the adapter.
        channel: LegacyChannel,
        /// Stable domain-local identity.
        sequence: u32,
    },
}

impl ObligationKey {
    fn sort_key(self) -> (u8, i32, i32, u32) {
        match self {
            Self::OpeningCore { sequence } => (0, 0, 0, u32::from(sequence)),
            Self::PaidConstruction(building) => (1, 0, 0, building.0),
            Self::ObservedBuilderWork { builder } => (2, 0, 0, builder.0),
            Self::DeferredFoundation { builder, anchor } => (3, anchor.y, anchor.x, builder.0),
            Self::SavedFoundry { anchor } => (4, anchor.y, anchor.x, 0),
            Self::ConnectedOffense { objective, anchor } => (5, anchor.y, anchor.x, objective.0),
            Self::Legacy { channel, sequence } => (6, channel.sort_key(), 0, sequence),
        }
    }
}

impl Ord for ObligationKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for ObligationKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Stable owner identity retained in conflicts and production schedules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimOwner {
    /// Mandatory work accepted before this allocation pass.
    Obligation {
        /// Priority class of the obligation.
        class: ObligationClass,
        /// Tick at which the controller accepted the work.
        accepted_at: Tick,
        /// Stable obligation identity.
        key: ObligationKey,
    },
    /// Fresh selectable work.
    Proposal(ProposalKey),
}

impl Ord for ClaimOwner {
    fn cmp(&self, other: &Self) -> Ordering {
        match (*self, *other) {
            (
                Self::Obligation {
                    accepted_at: left_tick,
                    class: left_class,
                    key: left_key,
                },
                Self::Obligation {
                    accepted_at: right_tick,
                    class: right_class,
                    key: right_key,
                },
            ) => (left_tick, left_class, left_key).cmp(&(right_tick, right_class, right_key)),
            (Self::Obligation { .. }, Self::Proposal(_)) => Ordering::Less,
            (Self::Proposal(_), Self::Obligation { .. }) => Ordering::Greater,
            (Self::Proposal(left), Self::Proposal(right)) => left.cmp(&right),
        }
    }
}

impl PartialOrd for ClaimOwner {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One mandatory resource owner imported before fresh proposals are compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedObligation {
    /// Priority class of this already-accepted work.
    pub(crate) class: ObligationClass,
    /// Original acceptance tick.
    pub(crate) accepted_at: Tick,
    /// Stable typed identity.
    pub(crate) key: ObligationKey,
    /// Exact resources that remain owned.
    pub(crate) claims: ClaimBundle,
}

impl ImportedObligation {
    pub(crate) fn owner(&self) -> ClaimOwner {
        ClaimOwner::Obligation {
            class: self.class,
            accepted_at: self.accepted_at,
            key: self.key,
        }
    }
}

/// Invalid allocator input that cannot be resolved by portfolio selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AllocationError {
    /// This migration stage accepts at most one proposal from each of two domains.
    TooManyProposals(usize),
    /// The same structural opportunity was submitted more than once.
    DuplicateProposalKey(ProposalKey),
    /// More than one candidate was submitted for one currently migrated domain.
    DuplicateProposalDomain(ProposalKey),
    /// The same prior obligation was imported more than once.
    DuplicateObligation(ClaimOwner),
    /// Mandatory prior work conflicts with the resource basis or another obligation.
    ObligationConflict {
        /// Obligation that could not be imported.
        obligation: ClaimOwner,
        /// Exact failed claim.
        conflict: AllocationConflict,
    },
}

/// Why one well-formed fresh proposal was not selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProposalRejection {
    /// The proposal cannot fit even without the competing fresh domain.
    Infeasible(AllocationConflict),
    /// The chosen portfolio owns a resource this proposal also requires.
    ConflictsWithSelected {
        /// Exact selected structural identities.
        selected: Vec<ProposalKey>,
        /// First canonical failed claim against that portfolio.
        conflict: AllocationConflict,
    },
    /// The proposal fits beside the winner but loses the documented rank.
    Outranked {
        /// Exact selected structural identities.
        selected: Vec<ProposalKey>,
        /// First allocator rank component that favored the selected portfolio.
        basis: OutrankingBasis,
    },
}

/// First deterministic rank component that favored one feasible portfolio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutrankingBasis {
    /// The selected portfolio had the stronger urgency histogram.
    Urgency,
    /// The selected portfolio had the stronger evidence-confidence histogram.
    Confidence,
    /// The selected portfolio had the stronger strategic-value histogram.
    StrategicValue,
    /// The selected portfolio could affect the match sooner.
    TimeToImpact,
    /// The selected portfolio had the stronger execution-safety histogram.
    Safety,
    /// Semantic bands tied and positive personality emphasis broke the tie.
    Personality,
    /// Higher ranks tied and the selected portfolio claimed less capital.
    LowerCapital,
    /// Every other component tied and canonical structural identity broke the tie.
    StructuralKey,
}

/// One proposal's complete allocation disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProposalDisposition {
    /// The exact payload and claims were selected.
    Accepted,
    /// The proposal remained unfunded for an explicit reason.
    Rejected(ProposalRejection),
}

/// Traceable result for one submitted proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProposalDecision {
    /// Stable proposal identity.
    pub(crate) key: ProposalKey,
    /// Named comparison case supplied by the domain.
    pub(crate) case: ProposalCase,
    /// Positive personality weight used only after all semantic bands tie.
    pub(crate) personality_weight: u128,
    /// Final disposition.
    pub(crate) disposition: ProposalDisposition,
}

/// One exact producer job in the selected deterministic lane order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ScheduledProducerJob {
    /// Owner whose payload contains this job.
    pub(crate) owner: ClaimOwner,
    /// Exact completed producer.
    pub(crate) producer: BuildingId,
    /// Exact unit kind.
    pub(crate) kind: UnitKind,
    /// Position in the owning domain's ordered request.
    pub(crate) request_ordinal: usize,
    /// Decision tick on which the Train command pays and enters the queue.
    pub(crate) enqueued_at: Tick,
    /// First production-phase tick occupied by this job.
    pub(crate) starts_at: Tick,
    /// Tick on which production completes.
    pub(crate) ready_at: Tick,
    /// Fixed observation deadline, strictly after `ready_at`.
    pub(crate) ready_before: Tick,
    /// Unit cost assigned from the observed current bank.
    pub(crate) current_scrap: u32,
    /// Unit cost assigned from income available when this job is enqueued.
    pub(crate) forecast_scrap: u32,
}

/// Final observation-relative funding split for one flexible capital claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CapitalFundingAssignment {
    /// Exact owner whose capital was assigned.
    pub(crate) owner: ClaimOwner,
    /// Fixed last tick by which the capital must be available.
    pub(crate) through: Tick,
    /// Capital assigned from the observed bank.
    pub(crate) current_scrap: u32,
    /// Capital assigned from completed-source income through `through`.
    pub(crate) forecast_scrap: u32,
}

/// Converts an accepted schedule into the typed future-lane overlay consumed
/// by residual production planners.
///
/// Jobs due on the current decision tick are lowered as real `TrainAt`
/// intents. Only later appends reserve their exact producer without becoming
/// fictional queue inventory.
pub(crate) fn future_producer_lane_reservations(
    capacity: &AllocationCapacity,
    schedule: &[ScheduledProducerJob],
) -> ProducerLaneReservations {
    ProducerLaneReservations::from_jobs(
        &capacity.resources,
        schedule.iter().map(|job| ReservedProducerJob {
            producer: job.producer,
            kind: job.kind,
            enqueued_at: job.enqueued_at,
            starts_at: job.starts_at,
            ready_at: job.ready_at,
            ready_before: job.ready_before,
        }),
    )
}

/// Selected exact payloads plus the evidence needed to explain the allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AllocationResult<FoundryPayload, OffensePayload> {
    /// Accepted payloads in canonical proposal-key order.
    pub(crate) accepted: Vec<InvestmentProposal<FoundryPayload, OffensePayload>>,
    /// One disposition for every submitted proposal in canonical key order.
    pub(crate) decisions: Vec<ProposalDecision>,
    /// Final lane order including imported obligations and accepted proposals.
    pub(crate) producer_schedule: Vec<ScheduledProducerJob>,
    /// Final funding splits for allocator-owned non-production capital.
    pub(crate) capital_assignments: Vec<CapitalFundingAssignment>,
    selected_state: ClaimState,
    /// Unique joint producer states explored for the selected portfolio.
    #[cfg(test)]
    pub(crate) production_search_states: usize,
    /// Previously failed canonical producer states reused during selection.
    #[cfg(test)]
    pub(crate) production_search_memo_hits: usize,
}

impl<FoundryPayload, OffensePayload> AllocationResult<FoundryPayload, OffensePayload> {
    /// Atomically adds one exact marginal package to the selected offense.
    ///
    /// Failure leaves both the accepted claims and returned producer schedule
    /// unchanged. Success keeps request ordinals after every earlier job owned
    /// by the same connected operation.
    pub(crate) fn try_extend_connected_offense(
        &mut self,
        capacity: &AllocationCapacity,
        key: ConnectedOffenseKey,
        claims: &ClaimBundle,
    ) -> Result<&[ScheduledProducerJob], AllocationConflict> {
        let proposal_key = ProposalKey::ConnectedOffenseMinimum(key);
        if !self
            .accepted
            .iter()
            .any(|proposal| proposal.key() == proposal_key)
        {
            return Err(AllocationConflict::InactiveProposal(proposal_key));
        }

        let owner = ClaimOwner::Proposal(proposal_key);
        let mut extended = self.selected_state.clone();
        let resolved = extended.try_apply_with_priority(
            capacity,
            owner,
            claims,
            FundingPriority::marginal(owner),
        )?;
        self.selected_state = extended;
        self.producer_schedule = resolved.producer_schedule;
        self.capital_assignments = resolved.capital_assignments;
        bind_accepted_capital(&mut self.accepted, &self.capital_assignments);
        #[cfg(test)]
        {
            self.production_search_states = resolved.search_states;
            self.production_search_memo_hits = resolved.memo_hits;
        }
        #[cfg(not(test))]
        let _ = (resolved.search_states, resolved.memo_hits);
        Ok(&self.producer_schedule)
    }
}

/// Selects the exact best compatible subset of the two current proposal domains.
///
/// All four masks are evaluated when both domains submit a candidate. Named
/// semantic bands win first, then personality at a deliberate near-tie, lower
/// claimed capital, and finally the smaller structural-key vector.
pub(crate) fn allocate<FoundryPayload, OffensePayload>(
    capacity: &AllocationCapacity,
    mut obligations: Vec<ImportedObligation>,
    mut proposals: Vec<InvestmentProposal<FoundryPayload, OffensePayload>>,
    personality: AllocationPersonality,
) -> Result<AllocationResult<FoundryPayload, OffensePayload>, AllocationError> {
    if proposals.len() > 2 {
        return Err(AllocationError::TooManyProposals(proposals.len()));
    }
    proposals.sort_by_key(InvestmentProposal::key);
    for pair in proposals.windows(2) {
        if pair[0].key() == pair[1].key() {
            return Err(AllocationError::DuplicateProposalKey(pair[0].key()));
        }
        if pair[0].key().same_domain(pair[1].key()) {
            return Err(AllocationError::DuplicateProposalDomain(pair[1].key()));
        }
    }
    obligations.sort_by_key(ImportedObligation::owner);
    for pair in obligations.windows(2) {
        if pair[0].owner() == pair[1].owner() {
            return Err(AllocationError::DuplicateObligation(pair[0].owner()));
        }
    }

    let mut mandatory = ClaimState::default();
    for obligation in &obligations {
        let owner = obligation.owner();
        mandatory
            .try_apply_with_priority(
                capacity,
                owner,
                &obligation.claims,
                FundingPriority::obligation(owner),
            )
            .map_err(|conflict| AllocationError::ObligationConflict {
                obligation: owner,
                conflict,
            })?;
    }

    let individual_conflicts: Vec<_> = proposals
        .iter()
        .map(|proposal| {
            let mut state = mandatory.clone();
            state
                .try_apply_with_priority(
                    capacity,
                    ClaimOwner::Proposal(proposal.key()),
                    proposal.claims(),
                    proposal_funding_priority(proposal, 0, capacity.resources.observed_at()),
                )
                .err()
        })
        .collect();

    let mask_count = 1_u8 << proposals.len();
    let mut best: Option<(u8, PortfolioRank, ClaimState)> = None;
    for mask in 0..mask_count {
        let mut state = mandatory.clone();
        let mut feasible = true;
        for (index, funding_priority) in proposal_funding_order(
            mask,
            &proposals,
            personality,
            capacity.resources.observed_at(),
        ) {
            let proposal = &proposals[index];
            if state
                .try_apply_with_priority(
                    capacity,
                    ClaimOwner::Proposal(proposal.key()),
                    proposal.claims(),
                    funding_priority,
                )
                .is_err()
            {
                feasible = false;
                break;
            }
        }
        if !feasible {
            continue;
        }
        let rank = portfolio_rank(mask, &proposals, personality);
        if best
            .as_ref()
            .is_none_or(|(_, current, _)| rank.cmp(current).is_gt())
        {
            best = Some((mask, rank, state));
        }
    }

    let (selected_mask, _, selected_state) =
        best.expect("the empty portfolio always preserves valid obligations");
    let selected_keys: Vec<_> = proposals
        .iter()
        .enumerate()
        .filter(|(index, _)| selected_mask & (1 << index) != 0)
        .map(|(_, proposal)| proposal.key())
        .collect();

    let decisions = proposals
        .iter()
        .enumerate()
        .map(|(index, proposal)| {
            let disposition = if selected_mask & (1 << index) != 0 {
                ProposalDisposition::Accepted
            } else if let Some(conflict) = &individual_conflicts[index] {
                ProposalDisposition::Rejected(ProposalRejection::Infeasible(conflict.clone()))
            } else {
                let mut combined = selected_state.clone();
                match combined.try_apply_with_priority(
                    capacity,
                    ClaimOwner::Proposal(proposal.key()),
                    proposal.claims(),
                    proposal_funding_priority(proposal, u8::MAX, capacity.resources.observed_at()),
                ) {
                    Ok(_) => {
                        let selected_rank = portfolio_rank(selected_mask, &proposals, personality);
                        let rejected_rank = portfolio_rank(1 << index, &proposals, personality);
                        let basis = outranking_basis(&selected_rank, &rejected_rank)
                            .expect("an outranked proposal has a strictly weaker rank");
                        ProposalDisposition::Rejected(ProposalRejection::Outranked {
                            selected: selected_keys.clone(),
                            basis,
                        })
                    }
                    Err(conflict) => {
                        ProposalDisposition::Rejected(ProposalRejection::ConflictsWithSelected {
                            selected: selected_keys.clone(),
                            conflict,
                        })
                    }
                }
            };
            ProposalDecision {
                key: proposal.key(),
                case: proposal.case(),
                personality_weight: personality.weight(proposal.key()),
                disposition,
            }
        })
        .collect();

    let resolved = selected_state
        .resolve(capacity)
        .expect("the selected claim state was already proven feasible");
    #[cfg(not(test))]
    let _ = (resolved.search_states, resolved.memo_hits);
    let mut accepted: Vec<_> = proposals
        .into_iter()
        .enumerate()
        .filter(|(index, _)| selected_mask & (1 << index) != 0)
        .map(|(_, proposal)| proposal)
        .collect();
    bind_accepted_capital(&mut accepted, &resolved.capital_assignments);

    Ok(AllocationResult {
        accepted,
        decisions,
        producer_schedule: resolved.producer_schedule,
        capital_assignments: resolved.capital_assignments,
        selected_state,
        #[cfg(test)]
        production_search_states: resolved.search_states,
        #[cfg(test)]
        production_search_memo_hits: resolved.memo_hits,
    })
}

fn bind_accepted_capital<FoundryPayload, OffensePayload>(
    accepted: &mut [InvestmentProposal<FoundryPayload, OffensePayload>],
    assignments: &[CapitalFundingAssignment],
) {
    for proposal in accepted {
        let owner = ClaimOwner::Proposal(proposal.key());
        let claims = match proposal {
            InvestmentProposal::FoundryExpansion { claims, .. } => claims,
            InvestmentProposal::ConnectedOffenseMinimum { .. } => continue,
        };
        if let Some(assignment) = assignments
            .iter()
            .copied()
            .find(|assignment| assignment.owner == owner)
        {
            claims.bind_deferrable_capital(assignment);
        }
    }
}

fn proposal_funding_order<FoundryPayload, OffensePayload>(
    mask: u8,
    proposals: &[InvestmentProposal<FoundryPayload, OffensePayload>],
    personality: AllocationPersonality,
    observed_at: Tick,
) -> Vec<(usize, FundingPriority)> {
    let mut retained = Vec::new();
    let mut fresh = Vec::new();
    for (index, proposal) in proposals.iter().enumerate() {
        if mask & (1 << index) == 0 {
            continue;
        }
        if proposal.accepted_at(observed_at) < observed_at {
            retained.push(index);
        } else {
            fresh.push(index);
        }
    }
    retained.sort_unstable_by_key(|&index| {
        (
            proposals[index].accepted_at(observed_at),
            proposals[index].key(),
        )
    });
    fresh.sort_unstable_by(|&left, &right| {
        let left_rank = portfolio_rank(1 << left, proposals, personality);
        let right_rank = portfolio_rank(1 << right, proposals, personality);
        right_rank
            .cmp(&left_rank)
            .then_with(|| proposals[left].key().cmp(&proposals[right].key()))
    });
    retained
        .into_iter()
        .map(|index| {
            let owner = ClaimOwner::Proposal(proposals[index].key());
            (
                index,
                FundingPriority::retained_proposal(
                    owner,
                    proposals[index].accepted_at(observed_at),
                ),
            )
        })
        .chain(fresh.into_iter().enumerate().map(|(order, index)| {
            let owner = ClaimOwner::Proposal(proposals[index].key());
            (
                index,
                FundingPriority::fresh_proposal(owner, u8::try_from(order).unwrap_or(u8::MAX)),
            )
        }))
        .collect()
}

fn proposal_funding_priority<FoundryPayload, OffensePayload>(
    proposal: &InvestmentProposal<FoundryPayload, OffensePayload>,
    semantic_order: u8,
    observed_at: Tick,
) -> FundingPriority {
    let owner = ClaimOwner::Proposal(proposal.key());
    let accepted_at = proposal.accepted_at(observed_at);
    if accepted_at < observed_at {
        FundingPriority::retained_proposal(owner, accepted_at)
    } else {
        FundingPriority::fresh_proposal(owner, semantic_order)
    }
}

type BandHistogram = [u8; 3];
type PortfolioRank = (
    BandHistogram,
    BandHistogram,
    BandHistogram,
    BandHistogram,
    BandHistogram,
    u128,
    Reverse<u128>,
    Reverse<Vec<ProposalKey>>,
);

fn outranking_basis(winner: &PortfolioRank, loser: &PortfolioRank) -> Option<OutrankingBasis> {
    if winner.0 != loser.0 {
        Some(OutrankingBasis::Urgency)
    } else if winner.1 != loser.1 {
        Some(OutrankingBasis::Confidence)
    } else if winner.2 != loser.2 {
        Some(OutrankingBasis::StrategicValue)
    } else if winner.3 != loser.3 {
        Some(OutrankingBasis::TimeToImpact)
    } else if winner.4 != loser.4 {
        Some(OutrankingBasis::Safety)
    } else if winner.5 != loser.5 {
        Some(OutrankingBasis::Personality)
    } else if winner.6 != loser.6 {
        Some(OutrankingBasis::LowerCapital)
    } else if winner.7 != loser.7 {
        Some(OutrankingBasis::StructuralKey)
    } else {
        None
    }
}

fn portfolio_rank<FoundryPayload, OffensePayload>(
    mask: u8,
    proposals: &[InvestmentProposal<FoundryPayload, OffensePayload>],
    personality: AllocationPersonality,
) -> PortfolioRank {
    let mut urgency = [0_u8; 3];
    let mut confidence = [0_u8; 3];
    let mut value = [0_u8; 3];
    let mut time_to_impact = [0_u8; 3];
    let mut safety = [0_u8; 3];
    let mut personality_weight = 0_u128;
    let mut capital = 0_u128;
    let mut keys = Vec::new();
    for (index, proposal) in proposals.iter().enumerate() {
        if mask & (1 << index) != 0 {
            let case = proposal.case();
            add_band(&mut urgency, urgency_index(case.urgency));
            add_band(&mut confidence, confidence_index(case.confidence));
            add_band(&mut value, value_index(case.value));
            add_band(
                &mut time_to_impact,
                time_to_impact_index(case.time_to_impact),
            );
            add_band(&mut safety, safety_index(case.safety));
            personality_weight =
                personality_weight.saturating_add(personality.weight(proposal.key()));
            capital = capital.saturating_add(proposal.claims().claimed_capital());
            keys.push(proposal.key());
        }
    }
    (
        urgency,
        confidence,
        value,
        time_to_impact,
        safety,
        personality_weight,
        Reverse(capital),
        Reverse(keys),
    )
}

fn add_band(histogram: &mut BandHistogram, index: usize) {
    histogram[index] = histogram[index].saturating_add(1);
}

const fn urgency_index(urgency: Urgency) -> usize {
    match urgency {
        Urgency::Pressing => 0,
        Urgency::Timely => 1,
        Urgency::Developmental => 2,
    }
}

const fn confidence_index(confidence: Confidence) -> usize {
    match confidence {
        Confidence::Current => 0,
        Confidence::Supported => 1,
        Confidence::Prior => 2,
    }
}

const fn value_index(value: StrategicValue) -> usize {
    match value {
        StrategicValue::Decisive => 0,
        StrategicValue::Material => 1,
        StrategicValue::Incremental => 2,
    }
}

const fn time_to_impact_index(time_to_impact: TimeToImpact) -> usize {
    match time_to_impact {
        TimeToImpact::Immediate => 0,
        TimeToImpact::Near => 1,
        TimeToImpact::Patient => 2,
    }
}

const fn safety_index(safety: ExecutionSafety) -> usize {
    match safety {
        ExecutionSafety::Secure => 0,
        ExecutionSafety::Managed => 1,
        ExecutionSafety::Speculative => 2,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActorRole {
    Builder,
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnedActor {
    unit: UnitId,
    owner: ClaimOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnedSite {
    site: SiteFootprint,
    owner: ClaimOwner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedProducerJob {
    claim: ProducerJobClaim,
    owner: ClaimOwner,
    ordinal: usize,
    funding_priority: FundingPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FundingPriority {
    tier: u8,
    accepted_at: Tick,
    order: u8,
    owner: ClaimOwner,
}

impl FundingPriority {
    fn obligation(owner: ClaimOwner) -> Self {
        let ClaimOwner::Obligation {
            class,
            accepted_at,
            key,
        } = owner
        else {
            unreachable!("an obligation priority requires an obligation owner")
        };
        let tier = match class {
            ObligationClass::Survival | ObligationClass::PaidWork => 0,
            ObligationClass::PersistentPlan | ObligationClass::Legacy => 1,
        };
        Self {
            tier,
            accepted_at,
            order: match key {
                ObligationKey::SavedFoundry { .. } => 1,
                ObligationKey::OpeningCore { .. }
                | ObligationKey::PaidConstruction(_)
                | ObligationKey::ObservedBuilderWork { .. }
                | ObligationKey::DeferredFoundation { .. }
                | ObligationKey::ConnectedOffense { .. }
                | ObligationKey::Legacy { .. } => 0,
            },
            owner,
        }
    }

    fn retained_proposal(owner: ClaimOwner, accepted_at: Tick) -> Self {
        Self {
            tier: 1,
            accepted_at,
            // At equal ticks an operation was admitted before the utility
            // expansion pass that could create a saved Foundry.
            order: 0,
            owner,
        }
    }

    fn fresh_proposal(owner: ClaimOwner, semantic_order: u8) -> Self {
        Self {
            tier: 2,
            accepted_at: 0,
            order: semantic_order,
            owner,
        }
    }

    fn marginal(owner: ClaimOwner) -> Self {
        Self {
            tier: 3,
            accepted_at: 0,
            order: 0,
            owner,
        }
    }

    #[cfg(test)]
    fn fallback(owner: ClaimOwner) -> Self {
        match owner {
            ClaimOwner::Obligation { .. } => Self::obligation(owner),
            ClaimOwner::Proposal(_) => Self::fresh_proposal(owner, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnedDeferrableCapital {
    claim: DeferrableCapitalClaim,
    owner: ClaimOwner,
    funding_priority: FundingPriority,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ClaimState {
    current_scrap: u64,
    forecast_scrap: Vec<ForecastClaim>,
    actors: Vec<OwnedActor>,
    sites: Vec<OwnedSite>,
    producer_jobs: Vec<OwnedProducerJob>,
    deferrable_capital: Vec<OwnedDeferrableCapital>,
}

struct ResolvedClaimState {
    producer_schedule: Vec<ScheduledProducerJob>,
    capital_assignments: Vec<CapitalFundingAssignment>,
    search_states: usize,
    memo_hits: usize,
}

impl ClaimState {
    #[cfg(test)]
    fn try_apply(
        &mut self,
        capacity: &AllocationCapacity,
        owner: ClaimOwner,
        claims: &ClaimBundle,
    ) -> Result<(Vec<ScheduledProducerJob>, usize, usize), AllocationConflict> {
        self.try_apply_with_priority(capacity, owner, claims, FundingPriority::fallback(owner))
            .map(|resolved| {
                (
                    resolved.producer_schedule,
                    resolved.search_states,
                    resolved.memo_hits,
                )
            })
    }

    fn try_apply_with_priority(
        &mut self,
        capacity: &AllocationCapacity,
        owner: ClaimOwner,
        claims: &ClaimBundle,
        funding_priority: FundingPriority,
    ) -> Result<ResolvedClaimState, AllocationConflict> {
        let checkpoint = self.clone();
        match self.apply(capacity, owner, claims, funding_priority) {
            Ok(result) => Ok(result),
            Err(conflict) => {
                *self = checkpoint;
                Err(conflict)
            }
        }
    }

    fn apply(
        &mut self,
        capacity: &AllocationCapacity,
        owner: ClaimOwner,
        claims: &ClaimBundle,
        funding_priority: FundingPriority,
    ) -> Result<ResolvedClaimState, AllocationConflict> {
        if claims.deferrable_capital.is_none() {
            self.current_scrap = self
                .current_scrap
                .saturating_add(u64::from(claims.current_scrap));
        }
        let requested = self.current_scrap;
        if requested > u64::from(capacity.resources.current_scrap()) {
            return Err(AllocationConflict::CurrentScrap {
                requested,
                available: capacity.resources.current_scrap(),
            });
        }

        if claims.deferrable_capital.is_none() {
            self.forecast_scrap
                .extend(claims.forecast_scrap.iter().copied());
        }
        self.forecast_scrap
            .sort_unstable_by_key(|claim| claim.through);
        self.validate_forecast(capacity)?;

        if let Some(claim) = claims.deferrable_capital {
            if claim.through > capacity.resources.horizon() {
                return Err(AllocationConflict::ForecastHorizon {
                    through: claim.through,
                    horizon: capacity.resources.horizon(),
                });
            }
            self.deferrable_capital.push(OwnedDeferrableCapital {
                claim,
                owner,
                funding_priority,
            });
            self.deferrable_capital
                .sort_unstable_by_key(|capital| (capital.funding_priority, capital.owner));
        }

        for &unit in &claims.builders {
            self.claim_actor(capacity, owner, unit, ActorRole::Builder)?;
        }
        for &unit in &claims.units {
            self.claim_actor(capacity, owner, unit, ActorRole::Unit)?;
        }
        for &site in &claims.sites {
            if let Some(existing) = self
                .sites
                .iter()
                .copied()
                .find(|existing| existing.site.overlaps(site))
            {
                return Err(AllocationConflict::Site {
                    requested: site,
                    existing: existing.site,
                    owner: existing.owner,
                });
            }
            let index = self
                .sites
                .binary_search_by_key(&site, |existing| existing.site)
                .unwrap_err();
            self.sites.insert(index, OwnedSite { site, owner });
        }

        let first_ordinal = self
            .producer_jobs
            .iter()
            .filter(|job| job.owner == owner)
            .count();
        for (offset, claim) in claims.producer_jobs.iter().enumerate() {
            if matches!(owner, ClaimOwner::Obligation { .. })
                && matches!(claim.access, ProducerJobAccess::Flexible(_))
            {
                return Err(AllocationConflict::UnassignedObligationJob {
                    owner,
                    kind: claim.kind,
                });
            }
            if claim.access.producers().is_empty() {
                return Err(AllocationConflict::ProducerAccess {
                    kind: claim.kind,
                    eligible_producers: claim.access.producers().to_vec(),
                });
            }
            for &producer in claim.access.producers() {
                let Some(lane) = capacity.producer(producer) else {
                    return Err(AllocationConflict::UnknownProducer(producer));
                };
                if lane.earliest_enqueue_tick(claim.kind).is_none() {
                    return Err(AllocationConflict::ProducerAccess {
                        kind: claim.kind,
                        eligible_producers: claim.access.producers().to_vec(),
                    });
                }
            }
            self.producer_jobs.push(OwnedProducerJob {
                claim: claim.clone(),
                owner,
                ordinal: first_ordinal + offset,
                funding_priority,
            });
        }
        self.producer_jobs
            .sort_unstable_by_key(|job| (job.owner, job.ordinal));
        self.resolve(capacity)
    }

    fn validate_forecast(&self, capacity: &AllocationCapacity) -> Result<(), AllocationConflict> {
        let mut requested = 0_u64;
        for (index, claim) in self.forecast_scrap.iter().enumerate() {
            if claim.through > capacity.resources.horizon() {
                return Err(AllocationConflict::ForecastHorizon {
                    through: claim.through,
                    horizon: capacity.resources.horizon(),
                });
            }
            requested = requested.saturating_add(u64::from(claim.amount));
            let next_deadline_differs = self
                .forecast_scrap
                .get(index + 1)
                .is_none_or(|next| next.through != claim.through);
            if next_deadline_differs {
                let available = capacity.forecast_through(claim.through);
                if requested > available {
                    return Err(AllocationConflict::ForecastScrap {
                        through: claim.through,
                        requested,
                        available,
                    });
                }
            }
        }
        Ok(())
    }

    fn claim_actor(
        &mut self,
        capacity: &AllocationCapacity,
        owner: ClaimOwner,
        unit: UnitId,
        role: ActorRole,
    ) -> Result<(), AllocationConflict> {
        let exists = match role {
            ActorRole::Builder => capacity
                .resources
                .contains_builder(unit, matches!(owner, ClaimOwner::Proposal(_))),
            ActorRole::Unit => capacity.resources.contains_unit(unit),
        };
        if !exists {
            return Err(match role {
                ActorRole::Builder => AllocationConflict::UnknownBuilder(unit),
                ActorRole::Unit => AllocationConflict::UnknownUnit(unit),
            });
        }
        match self
            .actors
            .binary_search_by_key(&unit, |existing| existing.unit)
        {
            Ok(index) => Err(AllocationConflict::Actor {
                unit,
                existing: self.actors[index].owner,
            }),
            Err(index) => {
                self.actors.insert(index, OwnedActor { unit, owner });
                Ok(())
            }
        }
    }

    fn resolve(
        &self,
        capacity: &AllocationCapacity,
    ) -> Result<ResolvedClaimState, AllocationConflict> {
        self.validate_production_funding_bound(capacity)?;
        self.resolve_with_funding_mode(capacity, JointFundingMode::PreferPriority)
            .or_else(|| {
                self.resolve_with_funding_mode(
                    capacity,
                    JointFundingMode::PreserveCompatiblePortfolio,
                )
            })
            .ok_or_else(|| producer_schedule_conflict(&self.producer_jobs))
    }

    fn resolve_with_funding_mode(
        &self,
        capacity: &AllocationCapacity,
        funding_mode: JointFundingMode,
    ) -> Option<ResolvedClaimState> {
        let mut producers = capacity.resources.producers().to_vec();
        let mut schedule = Vec::with_capacity(self.producer_jobs.len());
        let mut capital_assignments = Vec::with_capacity(self.deferrable_capital.len());
        let mut search = ProductionPortfolioSearch {
            capacity,
            jobs: &self.producer_jobs,
            current_capital: self.current_scrap,
            forecast_capital: &self.forecast_scrap,
            deferrable_capital: &self.deferrable_capital,
            failed: BTreeSet::new(),
            explored_states: 0,
            memo_hits: 0,
            funding_mode,
        };
        if !search.find(
            &mut producers,
            &mut vec![true; self.producer_jobs.len()],
            &mut schedule,
            &mut capital_assignments,
        ) {
            return None;
        }
        schedule.sort_unstable_by_key(|job| {
            (
                job.enqueued_at,
                job.starts_at,
                job.owner,
                job.request_ordinal,
                job.producer,
            )
        });
        capital_assignments.sort_unstable();
        Some(ResolvedClaimState {
            producer_schedule: schedule,
            capital_assignments,
            search_states: search.explored_states,
            memo_hits: search.memo_hits,
        })
    }

    fn validate_production_funding_bound(
        &self,
        capacity: &AllocationCapacity,
    ) -> Result<(), AllocationConflict> {
        let mut deadlines: Vec<_> = self
            .forecast_scrap
            .iter()
            .map(|claim| claim.through)
            .chain(
                self.deferrable_capital
                    .iter()
                    .map(|capital| capital.claim.through),
            )
            .collect();
        for job in &self.producer_jobs {
            let duration = Tick::from(job.claim.kind.stats().train_ticks);
            let Some(latest_start) = job.claim.ready_before.checked_sub(duration) else {
                return Err(producer_schedule_conflict(&self.producer_jobs));
            };
            if job.claim.enqueue_not_before > latest_start {
                return Err(producer_schedule_conflict(&self.producer_jobs));
            }
            deadlines.push(
                job.claim
                    .fixed_assignment()
                    .map_or(latest_start, |fixed| fixed.enqueued_at),
            );
        }
        deadlines.sort_unstable();
        deadlines.dedup();
        for through in deadlines {
            let requested = u128::from(self.current_scrap)
                + self
                    .forecast_scrap
                    .iter()
                    .filter(|claim| claim.through <= through)
                    .map(|claim| u128::from(claim.amount))
                    .sum::<u128>()
                + self
                    .deferrable_capital
                    .iter()
                    .filter(|capital| capital.claim.through <= through)
                    .map(|capital| u128::from(capital.claim.amount))
                    .sum::<u128>()
                + self
                    .producer_jobs
                    .iter()
                    .filter(|job| {
                        let duration = Tick::from(job.claim.kind.stats().train_ticks);
                        job.claim
                            .ready_before
                            .checked_sub(duration)
                            .is_some_and(|latest_start| {
                                job.claim
                                    .fixed_assignment()
                                    .map_or(latest_start, |fixed| fixed.enqueued_at)
                                    <= through
                            })
                    })
                    .map(|job| u128::from(job.claim.kind.stats().cost))
                    .sum::<u128>();
            let available = u128::from(capacity.resources.current_scrap())
                + u128::from(capacity.forecast_through(through));
            if requested > available {
                return Err(AllocationConflict::ProductionFunding {
                    through,
                    requested,
                    available,
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProductionSearchState {
    remaining: Vec<bool>,
    producers: Vec<ProducerPlanningProjection>,
    cash_spend: Vec<(Tick, u128)>,
    owner_enqueue_floors: Vec<(ClaimOwner, Tick)>,
}

struct ProductionPortfolioSearch<'a> {
    capacity: &'a AllocationCapacity,
    jobs: &'a [OwnedProducerJob],
    current_capital: u64,
    forecast_capital: &'a [ForecastClaim],
    deferrable_capital: &'a [OwnedDeferrableCapital],
    failed: BTreeSet<ProductionSearchState>,
    explored_states: usize,
    memo_hits: usize,
    funding_mode: JointFundingMode,
}

impl ProductionPortfolioSearch<'_> {
    fn find(
        &mut self,
        producers: &mut [ProducerPlanningProjection],
        remaining: &mut [bool],
        schedule: &mut Vec<ScheduledProducerJob>,
        capital_assignments: &mut Vec<CapitalFundingAssignment>,
    ) -> bool {
        if remaining.iter().all(|remaining| !remaining) {
            let mut funded = schedule.clone();
            if let Some(assignments) = assign_joint_funding(
                self.capacity,
                self.current_capital,
                self.forecast_capital,
                self.deferrable_capital,
                self.jobs,
                &mut funded,
                self.funding_mode,
            ) {
                *schedule = funded;
                *capital_assignments = assignments;
                return true;
            }
            return false;
        }
        let state = ProductionSearchState {
            remaining: remaining.to_vec(),
            producers: producers.to_vec(),
            cash_spend: cash_spend_timeline(schedule),
            owner_enqueue_floors: owner_enqueue_floors(schedule),
        };
        if self.failed.contains(&state) {
            self.memo_hits = self.memo_hits.saturating_add(1);
            return false;
        }
        self.explored_states = self.explored_states.saturating_add(1);

        let mut placements = Vec::new();
        for (job_index, is_remaining) in remaining.iter().copied().enumerate() {
            if !is_remaining || !self.is_frontier(job_index, remaining) {
                continue;
            }
            let job = &self.jobs[job_index];
            let owner_enqueue = schedule
                .iter()
                .filter(|row| row.owner == job.owner && row.request_ordinal < job.ordinal)
                .map(|row| row.enqueued_at)
                .max()
                .unwrap_or(0);
            for &producer in job.claim.access.producers() {
                let lane_index = producers
                    .binary_search_by_key(&producer, ProducerPlanningProjection::producer)
                    .expect("every producer claim was validated against capacity");
                let lane = &producers[lane_index];
                let Some(slot_tick) = lane.earliest_enqueue_tick(job.claim.kind) else {
                    continue;
                };
                let earliest = slot_tick
                    .max(job.claim.enqueue_not_before)
                    .max(owner_enqueue);
                for enqueued_at in candidate_enqueue_ticks(self.capacity, earliest, job) {
                    let mut lane_after = lane.clone();
                    let Some(projected) = lane_after.append(job.claim.kind, enqueued_at) else {
                        continue;
                    };
                    if projected.ready_at >= job.claim.ready_before {
                        continue;
                    }
                    if job.claim.fixed_assignment().is_some_and(|fixed| {
                        fixed.enqueued_at != enqueued_at
                            || fixed.starts_at != projected.starts_at
                            || fixed.ready_at != projected.ready_at
                    }) {
                        continue;
                    }
                    placements.push(ProductionPlacement {
                        job_index,
                        lane_index,
                        lane_after,
                        row: ScheduledProducerJob {
                            owner: job.owner,
                            producer,
                            kind: job.claim.kind,
                            request_ordinal: job.ordinal,
                            enqueued_at,
                            starts_at: projected.starts_at,
                            ready_at: projected.ready_at,
                            ready_before: job.claim.ready_before,
                            current_scrap: 0,
                            forecast_scrap: 0,
                        },
                    });
                }
            }
        }
        placements.sort_unstable_by_key(|placement| {
            (
                placement.row.enqueued_at,
                self.jobs[placement.job_index].funding_priority,
                placement.row.owner,
                placement.row.request_ordinal,
                placement.row.starts_at,
                placement.row.producer,
                placement.job_index,
            )
        });

        for placement in placements {
            schedule.push(placement.row);
            if !combined_cash_timeline_fits(
                self.capacity,
                self.current_capital,
                self.forecast_capital,
                self.deferrable_capital,
                schedule,
            ) {
                schedule.pop();
                continue;
            }
            let prior_lane =
                core::mem::replace(&mut producers[placement.lane_index], placement.lane_after);
            remaining[placement.job_index] = false;
            if self.find(producers, remaining, schedule, capital_assignments) {
                return true;
            }
            remaining[placement.job_index] = true;
            producers[placement.lane_index] = prior_lane;
            schedule.pop();
        }
        self.failed.insert(state);
        false
    }

    fn is_frontier(&self, job_index: usize, remaining: &[bool]) -> bool {
        let job = &self.jobs[job_index];
        if self.jobs.iter().enumerate().any(|(index, other)| {
            remaining[index] && other.owner == job.owner && other.ordinal < job.ordinal
        }) {
            return false;
        }
        let Some(producer) = committed_producer(job) else {
            return true;
        };
        !self.jobs.iter().enumerate().any(|(index, other)| {
            remaining[index]
                && committed_producer(other) == Some(producer)
                && (other.funding_priority, other.owner, other.ordinal)
                    < (job.funding_priority, job.owner, job.ordinal)
        })
    }
}

fn cash_spend_timeline(schedule: &[ScheduledProducerJob]) -> Vec<(Tick, u128)> {
    let mut result = Vec::<(Tick, u128)>::new();
    let mut rows = schedule
        .iter()
        .map(|row| (row.enqueued_at, u128::from(row.kind.stats().cost)))
        .collect::<Vec<_>>();
    rows.sort_unstable();
    for (tick, amount) in rows {
        if let Some((_, prior)) = result
            .last_mut()
            .filter(|(prior_tick, _)| *prior_tick == tick)
        {
            *prior = prior.saturating_add(amount);
        } else {
            result.push((tick, amount));
        }
    }
    result
}

fn owner_enqueue_floors(schedule: &[ScheduledProducerJob]) -> Vec<(ClaimOwner, Tick)> {
    let mut result = Vec::<(ClaimOwner, Tick)>::new();
    let mut rows = schedule
        .iter()
        .map(|row| (row.owner, row.enqueued_at))
        .collect::<Vec<_>>();
    rows.sort_unstable();
    for (owner, enqueued_at) in rows {
        if let Some((_, prior)) = result
            .last_mut()
            .filter(|(prior_owner, _)| *prior_owner == owner)
        {
            *prior = (*prior).max(enqueued_at);
        } else {
            result.push((owner, enqueued_at));
        }
    }
    result
}

#[derive(Debug, Clone)]
struct ProductionPlacement {
    job_index: usize,
    lane_index: usize,
    lane_after: ProducerPlanningProjection,
    row: ScheduledProducerJob,
}

fn candidate_enqueue_ticks(
    capacity: &AllocationCapacity,
    earliest: Tick,
    job: &OwnedProducerJob,
) -> Vec<Tick> {
    if let Some(fixed) = job.claim.fixed_assignment() {
        return (fixed.enqueued_at >= earliest
            && fixed.enqueued_at <= capacity.resources.horizon()
            && fixed.enqueued_at < job.claim.ready_before)
            .then_some(fixed.enqueued_at)
            .into_iter()
            .collect();
    }
    let Some(earliest) = capacity.resources.decision_at_or_after(earliest) else {
        return Vec::new();
    };
    let latest = capacity
        .resources
        .horizon()
        .min(job.claim.ready_before.saturating_sub(1));
    if earliest > latest {
        return Vec::new();
    }
    let mut ticks = vec![earliest];
    ticks.extend(
        capacity
            .resources
            .forecast_income()
            .iter()
            .map(|income| income.available_at)
            .filter(|&tick| tick > earliest && tick <= latest),
    );
    ticks
}

fn committed_producer(job: &OwnedProducerJob) -> Option<BuildingId> {
    match &job.claim.access {
        ProducerJobAccess::Fixed(fixed) => Some(fixed.producer),
        ProducerJobAccess::Flexible(_) => None,
    }
}

fn combined_cash_timeline_fits(
    capacity: &AllocationCapacity,
    current_capital: u64,
    forecast_capital: &[ForecastClaim],
    deferrable_capital: &[OwnedDeferrableCapital],
    schedule: &[ScheduledProducerJob],
) -> bool {
    let mut events: Vec<_> = forecast_capital
        .iter()
        .map(|claim| claim.through)
        .chain(
            deferrable_capital
                .iter()
                .map(|capital| capital.claim.through),
        )
        .chain(schedule.iter().map(|row| row.enqueued_at))
        .collect();
    events.sort_unstable();
    events.dedup();
    events.into_iter().all(|through| {
        let requested = u128::from(current_capital)
            + forecast_capital
                .iter()
                .filter(|claim| claim.through <= through)
                .map(|claim| u128::from(claim.amount))
                .sum::<u128>()
            + deferrable_capital
                .iter()
                .filter(|capital| capital.claim.through <= through)
                .map(|capital| u128::from(capital.claim.amount))
                .sum::<u128>()
            + schedule
                .iter()
                .filter(|row| row.enqueued_at <= through)
                .map(|row| u128::from(row.kind.stats().cost))
                .sum::<u128>();
        let available = u128::from(capacity.resources.current_scrap())
            + u128::from(capacity.forecast_through(through));
        requested <= available
    })
}

fn assign_joint_funding(
    capacity: &AllocationCapacity,
    current_capital: u64,
    forecast_capital: &[ForecastClaim],
    deferrable_capital: &[OwnedDeferrableCapital],
    jobs: &[OwnedProducerJob],
    schedule: &mut [ScheduledProducerJob],
    mode: JointFundingMode,
) -> Option<Vec<CapitalFundingAssignment>> {
    // (priority, type, stable index, deadline, amount). Lower-priority work is
    // assigned forecast before higher-priority work only when the preferred
    // current-first split cannot preserve the entire compatible portfolio.
    let mut funding_order = Vec::with_capacity(schedule.len() + deferrable_capital.len());
    for (index, row) in schedule.iter().enumerate() {
        let claim = jobs
            .iter()
            .find(|job| job.owner == row.owner && job.ordinal == row.request_ordinal)?;
        funding_order.push((
            claim.funding_priority,
            FundingTarget::Production(index),
            row.enqueued_at,
            row.kind.stats().cost,
        ));
    }
    for (index, capital) in deferrable_capital.iter().enumerate() {
        funding_order.push((
            capital.funding_priority,
            FundingTarget::Capital(index),
            capital.claim.through,
            capital.claim.amount,
        ));
    }
    funding_order.sort_unstable();
    let mut funding = JointFundingState::new(
        capacity,
        current_capital,
        forecast_capital,
        deferrable_capital,
        jobs,
        schedule,
    )?;
    match mode {
        JointFundingMode::PreferPriority => funding.assign_preferred(&funding_order)?,
        JointFundingMode::PreserveCompatiblePortfolio => {
            let fresh_start = funding_order.partition_point(|request| request.0.tier < 2);
            let marginal_start = funding_order.partition_point(|request| request.0.tier <= 2);
            funding.assign_preferred(&funding_order[..fresh_start])?;

            let before_fresh = funding.clone();
            if funding
                .assign_preferred(&funding_order[fresh_start..marginal_start])
                .is_none()
            {
                funding = before_fresh;
                funding.assign_deadline_compatible(&funding_order[fresh_start..marginal_start])?;
            }

            // Marginals and future lower bands may consume only what the
            // selected minimum portfolio left behind. They cannot reopen a
            // stronger tier's funding split merely to make themselves fit.
            funding.assign_preferred(&funding_order[marginal_start..])?;
        }
    }
    Some(funding.finish(schedule))
}

type FundingRequest = (FundingPriority, FundingTarget, Tick, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FundingTarget {
    Capital(usize),
    Production(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FundingSplit {
    through: Tick,
    current: u128,
    forecast: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JointFundingMode {
    PreferPriority,
    PreserveCompatiblePortfolio,
}

#[derive(Debug, Clone)]
struct JointFundingState<'a> {
    capacity: &'a AllocationCapacity,
    forecast_capital: &'a [ForecastClaim],
    deferrable_capital: &'a [OwnedDeferrableCapital],
    jobs: &'a [OwnedProducerJob],
    schedule: Vec<ScheduledProducerJob>,
    current_remaining: u128,
    assigned_forecast: Vec<ForecastClaim>,
    capital_assignments: Vec<CapitalFundingAssignment>,
    deadlines: Vec<Tick>,
}

impl<'a> JointFundingState<'a> {
    fn new(
        capacity: &'a AllocationCapacity,
        current_capital: u64,
        forecast_capital: &'a [ForecastClaim],
        deferrable_capital: &'a [OwnedDeferrableCapital],
        jobs: &'a [OwnedProducerJob],
        schedule: &[ScheduledProducerJob],
    ) -> Option<Self> {
        let mut deadlines = forecast_capital
            .iter()
            .map(|claim| claim.through)
            .chain(schedule.iter().map(|row| row.enqueued_at))
            .chain(
                deferrable_capital
                    .iter()
                    .map(|capital| capital.claim.through),
            )
            .collect::<Vec<_>>();
        deadlines.sort_unstable();
        deadlines.dedup();
        Some(Self {
            capacity,
            forecast_capital,
            deferrable_capital,
            jobs,
            schedule: schedule.to_vec(),
            current_remaining: u128::from(capacity.resources.current_scrap())
                .checked_sub(current_capital.into())?,
            assigned_forecast: Vec::new(),
            capital_assignments: Vec::with_capacity(deferrable_capital.len()),
            deadlines,
        })
    }

    fn assign_preferred(&mut self, requests: &[FundingRequest]) -> Option<()> {
        for &(_, target, through, amount) in requests {
            let amount = u128::from(amount);
            let from_current = amount.min(self.current_remaining);
            let from_forecast = amount - from_current;
            if from_forecast > self.forecast_available_through(through)? {
                return None;
            }
            self.record(
                target,
                FundingSplit {
                    through,
                    current: from_current,
                    forecast: from_forecast,
                },
            )?;
        }
        Some(())
    }

    fn assign_deadline_compatible(&mut self, requests: &[FundingRequest]) -> Option<()> {
        for &(_, target, through, amount) in requests.iter().rev() {
            let amount = u128::from(amount);
            let forecast_slack = self
                .deadlines
                .iter()
                .copied()
                .filter(|&deadline| deadline >= through)
                .map(|deadline| self.forecast_available_through(deadline))
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .min()
                .unwrap_or(0);
            let from_forecast = amount.min(forecast_slack);
            let from_current = amount - from_forecast;
            self.record(
                target,
                FundingSplit {
                    through,
                    current: from_current,
                    forecast: from_forecast,
                },
            )?;
        }
        Some(())
    }

    fn forecast_available_through(&self, through: Tick) -> Option<u128> {
        let fixed_due = self
            .forecast_capital
            .iter()
            .filter(|claim| claim.through <= through)
            .map(|claim| u128::from(claim.amount))
            .sum::<u128>();
        let assigned_due = self
            .assigned_forecast
            .iter()
            .filter(|claim| claim.through <= through)
            .map(|claim| u128::from(claim.amount))
            .sum::<u128>();
        u128::from(self.capacity.forecast_through(through))
            .checked_sub(fixed_due)?
            .checked_sub(assigned_due)
    }

    fn record(&mut self, target: FundingTarget, split: FundingSplit) -> Option<()> {
        let from_current = u32::try_from(split.current).ok()?;
        let from_forecast = u32::try_from(split.forecast).ok()?;
        self.current_remaining = self.current_remaining.checked_sub(split.current)?;
        if from_forecast > 0 {
            self.assigned_forecast.push(ForecastClaim {
                through: split.through,
                amount: from_forecast,
            });
        }
        match target {
            FundingTarget::Capital(index) => {
                let capital = self.deferrable_capital[index];
                self.capital_assignments.push(CapitalFundingAssignment {
                    owner: capital.owner,
                    through: split.through,
                    current_scrap: from_current,
                    forecast_scrap: from_forecast,
                });
            }
            FundingTarget::Production(index) => {
                let row = &mut self.schedule[index];
                let claim = self
                    .jobs
                    .iter()
                    .find(|job| job.owner == row.owner && job.ordinal == row.request_ordinal)?;
                if let Some(fixed) = claim.claim.fixed_assignment()
                    && (fixed.producer != row.producer
                        || fixed.enqueued_at != row.enqueued_at
                        || fixed.starts_at != row.starts_at
                        || fixed.ready_at != row.ready_at)
                {
                    return None;
                }
                row.current_scrap = from_current;
                row.forecast_scrap = from_forecast;
            }
        }
        Some(())
    }

    fn finish(self, schedule: &mut [ScheduledProducerJob]) -> Vec<CapitalFundingAssignment> {
        schedule.copy_from_slice(&self.schedule);
        self.capital_assignments
    }
}

fn producer_schedule_conflict(jobs: &[OwnedProducerJob]) -> AllocationConflict {
    let mut owners: Vec<_> = jobs.iter().map(|job| job.owner).collect();
    owners.sort_unstable();
    owners.dedup();
    let mut producers: Vec<_> = jobs
        .iter()
        .flat_map(|job| job.claim.access.producers().iter().copied())
        .collect();
    producers.sort_unstable();
    producers.dedup();
    AllocationConflict::ProducerSchedule { producers, owners }
}

fn first_duplicate<T: Copy + PartialEq>(values: &[T]) -> Option<T> {
    values
        .windows(2)
        .find(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
}

fn tile_key(tile: TilePos) -> (i32, i32) {
    (tile.y, tile.x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::resources::{BuilderResource, ForecastAvailability, ResourcePlanningFixture};
    use crate::stats::QUEUE_CAP;

    fn site(x: i32, y: i32) -> SiteFootprint {
        SiteFootprint::new(TilePos::new(x, y), (2, 2)).expect("the fixture site is positive")
    }

    fn bundle(
        current_scrap: u32,
        forecast_scrap: Vec<ForecastClaim>,
        builders: Vec<u32>,
        units: Vec<u32>,
        sites: Vec<SiteFootprint>,
        producer_jobs: Vec<ProducerJobClaim>,
    ) -> ClaimBundle {
        ClaimBundle::new(
            current_scrap,
            forecast_scrap,
            builders.into_iter().map(UnitId).collect(),
            units.into_iter().map(UnitId).collect(),
            sites,
            producer_jobs,
        )
        .expect("the fixture claim bundle is canonicalizable")
    }

    fn capacity(
        current_scrap: u32,
        forecast_horizon: Tick,
        forecast_income: Vec<ForecastAvailability>,
        producers: Vec<ProducerPlanningProjection>,
    ) -> AllocationCapacity {
        capacity_with_rosters(
            current_scrap,
            forecast_horizon,
            forecast_income,
            (1..=8).map(UnitId).collect(),
            (1..=4).map(UnitId).collect(),
            producers,
        )
    }

    fn capacity_with_rosters(
        current_scrap: u32,
        forecast_horizon: Tick,
        forecast_income: Vec<ForecastAvailability>,
        units: Vec<UnitId>,
        builders: Vec<UnitId>,
        producers: Vec<ProducerPlanningProjection>,
    ) -> AllocationCapacity {
        let resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap,
            observed_at: 0,
            horizon: if forecast_horizon == 0 {
                10_000
            } else {
                forecast_horizon
            },
            cadence: 1,
            forecast_income,
            units,
            builders: builders
                .into_iter()
                .map(|id| BuilderResource {
                    id,
                    kind: UnitKind::Harvester,
                    obligation: None,
                })
                .collect(),
            producers,
        })
        .expect("the fixture projection is canonical");
        AllocationCapacity::fixture(resources)
    }

    fn producer_fixture(
        producer: BuildingId,
        production_available_at: Tick,
        trainable: Vec<UnitKind>,
    ) -> ProducerPlanningProjection {
        ProducerPlanningProjection::fixture(
            producer,
            0,
            1,
            production_available_at,
            vec![0; QUEUE_CAP],
            trainable,
        )
        .expect("the fixture producer is canonical")
    }

    fn timed_capacity(
        current_scrap: u32,
        observed_at: Tick,
        horizon: Tick,
        cadence: Tick,
        forecast_income: Vec<ForecastAvailability>,
        producers: Vec<ProducerPlanningProjection>,
    ) -> AllocationCapacity {
        let resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap,
            observed_at,
            horizon,
            cadence,
            forecast_income,
            units: (1..=8).map(UnitId).collect(),
            builders: (1..=4)
                .map(|id| BuilderResource {
                    id: UnitId(id),
                    kind: UnitKind::Harvester,
                    obligation: None,
                })
                .collect(),
            producers,
        })
        .expect("the timed fixture projection is canonical");
        AllocationCapacity::fixture(resources)
    }

    const fn ordinary_case() -> ProposalCase {
        ProposalCase {
            urgency: Urgency::Timely,
            confidence: Confidence::Supported,
            value: StrategicValue::Material,
            time_to_impact: TimeToImpact::Near,
            safety: ExecutionSafety::Managed,
        }
    }

    fn foundry(
        x: i32,
        current_scrap: u32,
        forecast_scrap: Vec<ForecastClaim>,
        case: ProposalCase,
    ) -> InvestmentProposal<&'static str, &'static str> {
        InvestmentProposal::FoundryExpansion {
            key: FoundryExpansionKey {
                anchor: TilePos::new(x, 10),
            },
            case,
            claims: bundle(
                current_scrap,
                forecast_scrap,
                vec![1],
                vec![],
                vec![site(x, 10)],
                vec![],
            ),
            payload: "exact foundry token",
        }
    }

    fn deferrable_foundry(
        x: i32,
        amount: u32,
        through: Tick,
        case: ProposalCase,
    ) -> InvestmentProposal<&'static str, &'static str> {
        let claims = bundle(0, vec![], vec![1], vec![], vec![site(x, 10)], vec![])
            .with_deferrable_capital(DeferrableCapitalClaim { through, amount })
            .expect("the fixture has one unassigned capital claim");
        InvestmentProposal::FoundryExpansion {
            key: FoundryExpansionKey {
                anchor: TilePos::new(x, 10),
            },
            case,
            claims,
            payload: "exact flexible foundry token",
        }
    }

    fn offense(
        current_scrap: u32,
        forecast_scrap: Vec<ForecastClaim>,
        case: ProposalCase,
    ) -> InvestmentProposal<&'static str, &'static str> {
        InvestmentProposal::ConnectedOffenseMinimum {
            key: ConnectedOffenseKey {
                objective: BuildingId(90),
                anchor: TilePos::new(40, 10),
            },
            case,
            accepted_at: 0,
            claims: bundle(
                current_scrap,
                forecast_scrap,
                vec![],
                vec![5],
                vec![],
                vec![],
            ),
            payload: "exact offense token",
        }
    }

    fn offense_accepted_at(
        current_scrap: u32,
        forecast_scrap: Vec<ForecastClaim>,
        case: ProposalCase,
        accepted_at: Tick,
    ) -> InvestmentProposal<&'static str, &'static str> {
        let mut proposal = offense(current_scrap, forecast_scrap, case);
        let InvestmentProposal::ConnectedOffenseMinimum {
            accepted_at: proposal_accepted_at,
            ..
        } = &mut proposal
        else {
            unreachable!()
        };
        *proposal_accepted_at = accepted_at;
        proposal
    }

    fn accepted_keys<FoundryPayload, OffensePayload>(
        result: &AllocationResult<FoundryPayload, OffensePayload>,
    ) -> Vec<ProposalKey> {
        result
            .accepted
            .iter()
            .map(InvestmentProposal::key)
            .collect()
    }

    fn with_jobs(
        mut proposal: InvestmentProposal<&'static str, &'static str>,
        jobs: Vec<ProducerJobClaim>,
    ) -> InvestmentProposal<&'static str, &'static str> {
        let claims = match &mut proposal {
            InvestmentProposal::FoundryExpansion { claims, .. }
            | InvestmentProposal::ConnectedOffenseMinimum { claims, .. } => claims,
        };
        claims.producer_jobs = jobs;
        proposal
    }

    #[test]
    fn two_proposals_examine_the_exact_four_mask_oracle() {
        let result = allocate(
            &capacity(100, 0, vec![], vec![]),
            vec![],
            vec![
                foundry(10, 70, vec![], ordinary_case()),
                offense(
                    70,
                    vec![],
                    ProposalCase {
                        safety: ExecutionSafety::Secure,
                        ..ordinary_case()
                    },
                ),
            ],
            AllocationPersonality::default(),
        )
        .expect("the allocator accepts both proposal domains");

        assert_eq!(
            accepted_keys(&result),
            vec![
                offense(
                    0,
                    vec![],
                    ProposalCase {
                        safety: ExecutionSafety::Secure,
                        ..ordinary_case()
                    }
                )
                .key()
            ]
        );
        assert!(matches!(
            result.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::ConflictsWithSelected {
                conflict: AllocationConflict::CurrentScrap { .. },
                ..
            })
        ));
    }

    #[test]
    fn allocation_matches_an_independent_four_mask_oracle() {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
        struct OracleRank {
            urgency: [u8; 3],
            confidence: [u8; 3],
            value: [u8; 3],
            time_to_impact: [u8; 3],
            safety: [u8; 3],
            personality: u128,
            capital: u128,
            keys: Vec<ProposalKey>,
        }

        let add_case = |rank: &mut OracleRank, case: ProposalCase| {
            let urgency = match case.urgency {
                Urgency::Pressing => 0,
                Urgency::Timely => 1,
                Urgency::Developmental => 2,
            };
            let confidence = match case.confidence {
                Confidence::Current => 0,
                Confidence::Supported => 1,
                Confidence::Prior => 2,
            };
            let value = match case.value {
                StrategicValue::Decisive => 0,
                StrategicValue::Material => 1,
                StrategicValue::Incremental => 2,
            };
            let time_to_impact = match case.time_to_impact {
                TimeToImpact::Immediate => 0,
                TimeToImpact::Near => 1,
                TimeToImpact::Patient => 2,
            };
            let safety = match case.safety {
                ExecutionSafety::Secure => 0,
                ExecutionSafety::Managed => 1,
                ExecutionSafety::Speculative => 2,
            };
            rank.urgency[urgency] += 1;
            rank.confidence[confidence] += 1;
            rank.value[value] += 1;
            rank.time_to_impact[time_to_impact] += 1;
            rank.safety[safety] += 1;
        };
        let better = |left: &OracleRank, right: &OracleRank| {
            (
                left.urgency,
                left.confidence,
                left.value,
                left.time_to_impact,
                left.safety,
                left.personality,
                Reverse(left.capital),
                Reverse(&left.keys),
            ) > (
                right.urgency,
                right.confidence,
                right.value,
                right.time_to_impact,
                right.safety,
                right.personality,
                Reverse(right.capital),
                Reverse(&right.keys),
            )
        };
        let cases = [
            ProposalCase {
                urgency: Urgency::Developmental,
                confidence: Confidence::Prior,
                value: StrategicValue::Incremental,
                time_to_impact: TimeToImpact::Patient,
                safety: ExecutionSafety::Speculative,
            },
            ordinary_case(),
            ProposalCase {
                urgency: Urgency::Pressing,
                confidence: Confidence::Current,
                value: StrategicValue::Decisive,
                time_to_impact: TimeToImpact::Immediate,
                safety: ExecutionSafety::Secure,
            },
        ];
        let personalities = [
            AllocationPersonality::default(),
            AllocationPersonality {
                economy: 40,
                offense: 0,
            },
            AllocationPersonality {
                economy: 0,
                offense: 40,
            },
        ];
        let economy_key = FoundryExpansionKey {
            anchor: TilePos::new(10, 10),
        };
        let offense_key = ConnectedOffenseKey {
            objective: BuildingId(90),
            anchor: TilePos::new(40, 10),
        };

        for bank in 0..=4 {
            for economy_cost in 0..=4 {
                for offense_cost in 0..=4 {
                    for &economy_case in &cases {
                        for &offense_case in &cases {
                            for &personality in &personalities {
                                let proposals = vec![
                                    foundry(10, economy_cost, vec![], economy_case),
                                    offense(offense_cost, vec![], offense_case),
                                ];
                                let result = allocate(
                                    &capacity(bank, 0, vec![], vec![]),
                                    vec![],
                                    proposals,
                                    personality,
                                )
                                .expect("the exhaustive fixture contains two valid domains");

                                let mut best: Option<(u8, OracleRank)> = None;
                                for mask in 0_u8..4 {
                                    let capital = u128::from(
                                        (mask & 1 != 0) as u32 * economy_cost
                                            + (mask & 2 != 0) as u32 * offense_cost,
                                    );
                                    if capital > u128::from(bank) {
                                        continue;
                                    }
                                    let mut rank = OracleRank {
                                        urgency: [0; 3],
                                        confidence: [0; 3],
                                        value: [0; 3],
                                        time_to_impact: [0; 3],
                                        safety: [0; 3],
                                        personality: 0,
                                        capital,
                                        keys: Vec::new(),
                                    };
                                    if mask & 1 != 0 {
                                        add_case(&mut rank, economy_case);
                                        rank.personality += BASE_PERSONALITY_WEIGHT
                                            + u128::from(personality.economy);
                                        rank.keys.push(ProposalKey::FoundryExpansion(economy_key));
                                    }
                                    if mask & 2 != 0 {
                                        add_case(&mut rank, offense_case);
                                        rank.personality += BASE_PERSONALITY_WEIGHT
                                            + u128::from(personality.offense);
                                        rank.keys.push(ProposalKey::ConnectedOffenseMinimum(
                                            offense_key,
                                        ));
                                    }
                                    if best
                                        .as_ref()
                                        .is_none_or(|(_, current)| better(&rank, current))
                                    {
                                        best = Some((mask, rank));
                                    }
                                }

                                let actual_mask =
                                    result.accepted.iter().fold(0_u8, |mask, item| {
                                        mask | match item {
                                            InvestmentProposal::FoundryExpansion { .. } => 1,
                                            InvestmentProposal::ConnectedOffenseMinimum {
                                                ..
                                            } => 2,
                                        }
                                    });
                                assert_eq!(actual_mask, best.expect("mask zero is feasible").0);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn forecast_is_shared_capacity_and_never_becomes_current_credit() {
        let deadline = 500;
        let result = allocate(
            &capacity(
                0,
                deadline,
                vec![ForecastAvailability {
                    available_at: 400,
                    amount: 50,
                }],
                vec![],
            ),
            vec![],
            vec![
                foundry(
                    10,
                    0,
                    vec![ForecastClaim {
                        through: deadline,
                        amount: 40,
                    }],
                    ordinary_case(),
                ),
                offense(
                    0,
                    vec![ForecastClaim {
                        through: deadline,
                        amount: 40,
                    }],
                    ordinary_case(),
                ),
            ],
            AllocationPersonality::default(),
        )
        .expect("forecast conflict is a proposal decision");

        assert!(result.decisions.iter().any(|decision| matches!(
            &decision.disposition,
            ProposalDisposition::Rejected(ProposalRejection::ConflictsWithSelected {
                conflict: AllocationConflict::ForecastScrap {
                    requested: 80,
                    available: 50,
                    ..
                },
                ..
            })
        )));
    }

    #[test]
    fn forecast_checks_every_deadline_prefix_not_only_the_final_total() {
        let result = allocate(
            &capacity(
                0,
                500,
                vec![
                    ForecastAvailability {
                        available_at: 100,
                        amount: 10,
                    },
                    ForecastAvailability {
                        available_at: 500,
                        amount: 90,
                    },
                ],
                vec![],
            ),
            vec![],
            vec![offense(
                0,
                vec![
                    ForecastClaim {
                        through: 200,
                        amount: 50,
                    },
                    ForecastClaim {
                        through: 500,
                        amount: 50,
                    },
                ],
                ordinary_case(),
            )],
            AllocationPersonality::default(),
        )
        .expect("prefix funding failure is proposal-local");

        assert!(matches!(
            result.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::ForecastScrap {
                    through: 200,
                    requested: 50,
                    available: 10,
                }
            ))
        ));
    }

    #[test]
    fn compatible_exact_claims_admit_both_payloads() {
        let mut expansion = foundry(10, 70, vec![], ordinary_case());
        let InvestmentProposal::FoundryExpansion { payload, .. } = &mut expansion else {
            unreachable!()
        };
        *payload = "keep this exact builder and site";
        let mut attack = offense(60, vec![], ordinary_case());
        let InvestmentProposal::ConnectedOffenseMinimum { payload, .. } = &mut attack else {
            unreachable!()
        };
        *payload = "keep this exact package";

        let result = allocate(
            &capacity(130, 0, vec![], vec![]),
            vec![],
            vec![attack, expansion],
            AllocationPersonality::default(),
        )
        .expect("compatible domains can run concurrently");

        assert_eq!(result.accepted.len(), 2);
        assert!(
            result
                .decisions
                .iter()
                .all(|decision| decision.disposition == ProposalDisposition::Accepted)
        );
        assert!(matches!(
            &result.accepted[0],
            InvestmentProposal::FoundryExpansion { payload, .. }
                if *payload == "keep this exact builder and site"
        ));
        assert!(matches!(
            &result.accepted[1],
            InvestmentProposal::ConnectedOffenseMinimum { payload, .. }
                if *payload == "keep this exact package"
        ));
    }

    #[test]
    fn canonicalization_makes_input_permutations_identical() {
        let first_capacity = capacity_with_rosters(
            160,
            500,
            vec![
                ForecastAvailability {
                    available_at: 300,
                    amount: 20,
                },
                ForecastAvailability {
                    available_at: 200,
                    amount: 10,
                },
            ],
            vec![UnitId(2), UnitId(1), UnitId(5)],
            vec![UnitId(2), UnitId(1)],
            vec![
                producer_fixture(BuildingId(8), 0, vec![UnitKind::Sentinel]),
                producer_fixture(BuildingId(7), 0, vec![UnitKind::Harvester]),
            ],
        );
        let second_capacity = capacity_with_rosters(
            160,
            500,
            vec![
                ForecastAvailability {
                    available_at: 200,
                    amount: 10,
                },
                ForecastAvailability {
                    available_at: 300,
                    amount: 20,
                },
            ],
            vec![UnitId(5), UnitId(1), UnitId(2)],
            vec![UnitId(1), UnitId(2)],
            vec![
                producer_fixture(BuildingId(7), 0, vec![UnitKind::Harvester]),
                producer_fixture(BuildingId(8), 0, vec![UnitKind::Sentinel]),
            ],
        );
        let obligations = vec![ImportedObligation {
            class: ObligationClass::PaidWork,
            accepted_at: 10,
            key: ObligationKey::PaidConstruction(BuildingId(20)),
            claims: ClaimBundle::default(),
        }];
        let proposals = vec![
            foundry(10, 70, vec![], ordinary_case()),
            offense(60, vec![], ordinary_case()),
        ];

        let first = allocate(
            &first_capacity,
            obligations.clone(),
            proposals.clone(),
            AllocationPersonality::default(),
        );
        let second = allocate(
            &second_capacity,
            obligations.into_iter().rev().collect(),
            proposals.into_iter().rev().collect(),
            AllocationPersonality::default(),
        );
        assert_eq!(first, second);
    }

    #[test]
    fn duplicate_structural_keys_are_rejected_independent_of_input_order() {
        let first = foundry(10, 10, vec![], ordinary_case());
        let second = foundry(10, 20, vec![], ordinary_case());
        let basis = capacity(100, 0, vec![], vec![]);

        let forward = allocate(
            &basis,
            vec![],
            vec![first.clone(), second.clone()],
            AllocationPersonality::default(),
        );
        let reverse = allocate(
            &basis,
            vec![],
            vec![second, first],
            AllocationPersonality::default(),
        );
        assert_eq!(forward, reverse);
        assert!(matches!(
            forward,
            Err(AllocationError::DuplicateProposalKey(
                ProposalKey::FoundryExpansion(_)
            ))
        ));
    }

    #[test]
    fn lowest_semantic_case_remains_a_positive_selectable_proposal() {
        let proposal = offense(
            0,
            vec![],
            ProposalCase {
                urgency: Urgency::Developmental,
                confidence: Confidence::Prior,
                value: StrategicValue::Incremental,
                time_to_impact: TimeToImpact::Patient,
                safety: ExecutionSafety::Speculative,
            },
        );
        let result = allocate(
            &capacity(0, 0, vec![], vec![]),
            vec![],
            vec![proposal],
            AllocationPersonality::default(),
        )
        .expect("every named case carries positive strategic value");

        assert_eq!(result.accepted.len(), 1);
        assert_eq!(
            result.decisions[0].disposition,
            ProposalDisposition::Accepted
        );
    }

    #[test]
    fn mandatory_work_creates_a_higher_order_lane_conflict() {
        let producer = BuildingId(7);
        let deadline = 350;
        let basis = capacity(
            230,
            0,
            vec![],
            vec![producer_fixture(
                producer,
                0,
                vec![UnitKind::Harvester, UnitKind::Sentinel],
            )],
        );
        let obligation = ImportedObligation {
            class: ObligationClass::Survival,
            accepted_at: 0,
            key: ObligationKey::OpeningCore { sequence: 0 },
            claims: bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    producer,
                    UnitKind::Harvester,
                    0,
                    0,
                    99,
                    deadline,
                )],
            ),
        };
        let mut expansion = foundry(10, 0, vec![], ordinary_case());
        let InvestmentProposal::FoundryExpansion { claims, .. } = &mut expansion else {
            unreachable!()
        };
        *claims = bundle(
            0,
            vec![],
            vec![1],
            vec![],
            vec![site(10, 10)],
            vec![ProducerJobClaim::flexible(
                UnitKind::Sentinel,
                0,
                deadline,
                vec![producer],
            )],
        );
        let mut attack = offense(0, vec![], ordinary_case());
        let InvestmentProposal::ConnectedOffenseMinimum { claims, .. } = &mut attack else {
            unreachable!()
        };
        *claims = bundle(
            0,
            vec![],
            vec![],
            vec![5],
            vec![],
            vec![ProducerJobClaim::flexible(
                UnitKind::Sentinel,
                0,
                deadline,
                vec![producer],
            )],
        );

        let result = allocate(
            &basis,
            vec![obligation],
            vec![expansion, attack],
            AllocationPersonality::default(),
        )
        .expect("the mandatory lane prefix remains valid");

        assert_eq!(accepted_keys(&result).len(), 1);
        assert!(result.decisions.iter().any(|decision| matches!(
            &decision.disposition,
            ProposalDisposition::Rejected(ProposalRejection::ConflictsWithSelected {
                conflict: AllocationConflict::ProducerSchedule { .. },
                ..
            })
        )));
    }

    #[test]
    fn fresh_lane_jobs_interleave_exactly_when_deadlines_require_it() {
        let producer = BuildingId(7);
        let basis = capacity(
            230,
            0,
            vec![],
            vec![producer_fixture(
                producer,
                0,
                vec![UnitKind::Harvester, UnitKind::Sentinel],
            )],
        );
        let mut expansion = foundry(10, 0, vec![], ordinary_case());
        let InvestmentProposal::FoundryExpansion { claims, .. } = &mut expansion else {
            unreachable!()
        };
        *claims = bundle(
            0,
            vec![],
            vec![1],
            vec![],
            vec![site(10, 10)],
            vec![
                ProducerJobClaim::flexible(UnitKind::Sentinel, 0, 500, vec![producer]),
                ProducerJobClaim::flexible(UnitKind::Sentinel, 0, 650, vec![producer]),
            ],
        );
        let mut attack = offense(0, vec![], ordinary_case());
        let InvestmentProposal::ConnectedOffenseMinimum { claims, .. } = &mut attack else {
            unreachable!()
        };
        *claims = bundle(
            0,
            vec![],
            vec![],
            vec![5],
            vec![],
            vec![ProducerJobClaim::flexible(
                UnitKind::Harvester,
                0,
                200,
                vec![producer],
            )],
        );

        let result = allocate(
            &basis,
            vec![],
            vec![expansion, attack],
            AllocationPersonality::default(),
        )
        .expect("the exact lane interleaving is feasible");

        assert_eq!(result.accepted.len(), 2);
        assert_eq!(
            result
                .producer_schedule
                .iter()
                .map(|job| job.kind)
                .collect::<Vec<_>>(),
            vec![UnitKind::Harvester, UnitKind::Sentinel, UnitKind::Sentinel]
        );
        assert_eq!(result.producer_schedule[0].ready_at, 99);
        assert_eq!(result.producer_schedule[2].ready_at, 399);
        assert!(
            result
                .producer_schedule
                .iter()
                .all(|job| job.enqueued_at == 0)
        );
        assert_eq!(
            result
                .producer_schedule
                .iter()
                .map(|job| job.current_scrap)
                .sum::<u32>(),
            230
        );
    }

    #[test]
    fn joint_assignment_backtracks_to_preserve_the_specialist_lane() {
        let flexible = BuildingId(7);
        let specialist = BuildingId(8);
        let basis = capacity(
            140,
            0,
            vec![],
            vec![
                producer_fixture(flexible, 0, vec![UnitKind::Harvester, UnitKind::Sentinel]),
                producer_fixture(specialist, 0, vec![UnitKind::Harvester]),
            ],
        );
        let mut expansion = foundry(10, 0, vec![], ordinary_case());
        let InvestmentProposal::FoundryExpansion { claims, .. } = &mut expansion else {
            unreachable!()
        };
        *claims = bundle(
            0,
            vec![],
            vec![1],
            vec![],
            vec![site(10, 10)],
            vec![ProducerJobClaim::flexible(
                UnitKind::Harvester,
                0,
                101,
                vec![flexible, specialist],
            )],
        );
        let mut attack = offense(0, vec![], ordinary_case());
        let InvestmentProposal::ConnectedOffenseMinimum { claims, .. } = &mut attack else {
            unreachable!()
        };
        *claims = bundle(
            0,
            vec![],
            vec![],
            vec![5],
            vec![],
            vec![ProducerJobClaim::flexible(
                UnitKind::Sentinel,
                0,
                151,
                vec![flexible],
            )],
        );

        let result = allocate(
            &basis,
            vec![],
            vec![expansion, attack],
            AllocationPersonality::default(),
        )
        .expect("joint assignment can move the flexible request");

        assert_eq!(result.accepted.len(), 2);
        assert!(result.producer_schedule.iter().any(|job| {
            job.kind == UnitKind::Harvester && job.producer == specialist && job.ready_at == 99
        }));
        assert!(result.producer_schedule.iter().any(|job| {
            job.kind == UnitKind::Sentinel && job.producer == flexible && job.ready_at == 149
        }));
    }

    #[test]
    fn staged_income_cannot_fund_parallel_jobs_before_it_is_spendable() {
        let first = BuildingId(7);
        let second = BuildingId(8);
        let result = allocate(
            &capacity(
                0,
                20,
                vec![
                    ForecastAvailability {
                        available_at: 10,
                        amount: 50,
                    },
                    ForecastAvailability {
                        available_at: 20,
                        amount: 50,
                    },
                ],
                vec![
                    producer_fixture(first, 0, vec![UnitKind::Harvester]),
                    producer_fixture(second, 0, vec![UnitKind::Harvester]),
                ],
            ),
            vec![],
            vec![
                with_jobs(
                    foundry(10, 0, vec![], ordinary_case()),
                    vec![ProducerJobClaim::flexible(
                        UnitKind::Harvester,
                        11,
                        112,
                        vec![first],
                    )],
                ),
                with_jobs(
                    offense(0, vec![], ordinary_case()),
                    vec![ProducerJobClaim::flexible(
                        UnitKind::Harvester,
                        11,
                        112,
                        vec![second],
                    )],
                ),
            ],
            AllocationPersonality::default(),
        )
        .expect("a shared funding conflict rejects only one fresh proposal");

        assert_eq!(result.accepted.len(), 1);
        assert!(result.decisions.iter().any(|decision| matches!(
            &decision.disposition,
            ProposalDisposition::Rejected(ProposalRejection::ConflictsWithSelected {
                conflict: AllocationConflict::ProductionFunding {
                    through: 12,
                    requested: 100,
                    available: 50,
                },
                ..
            })
        )));
    }

    #[test]
    fn allocator_assigns_current_then_forecast_funding_across_proposals() {
        let first = BuildingId(7);
        let second = BuildingId(8);
        let result = allocate(
            &capacity(
                50,
                20,
                vec![ForecastAvailability {
                    available_at: 20,
                    amount: 50,
                }],
                vec![
                    producer_fixture(first, 0, vec![UnitKind::Harvester]),
                    producer_fixture(second, 0, vec![UnitKind::Harvester]),
                ],
            ),
            vec![],
            vec![
                with_jobs(
                    foundry(10, 0, vec![], ordinary_case()),
                    vec![ProducerJobClaim::flexible(
                        UnitKind::Harvester,
                        0,
                        121,
                        vec![first],
                    )],
                ),
                with_jobs(
                    offense(0, vec![], ordinary_case()),
                    vec![ProducerJobClaim::flexible(
                        UnitKind::Harvester,
                        0,
                        121,
                        vec![second],
                    )],
                ),
            ],
            AllocationPersonality::default(),
        )
        .expect("funding is assigned only after the combined portfolio is known");

        assert_eq!(result.accepted.len(), 2);
        assert_eq!(result.producer_schedule[0].starts_at, 0);
        assert_eq!(result.producer_schedule[0].current_scrap, 50);
        assert_eq!(result.producer_schedule[0].forecast_scrap, 0);
        assert_eq!(result.producer_schedule[1].starts_at, 20);
        assert_eq!(result.producer_schedule[1].current_scrap, 0);
        assert_eq!(result.producer_schedule[1].forecast_scrap, 50);
    }

    #[test]
    fn semantic_order_assigns_urgent_production_before_patient_flexible_capital() {
        let producer = BuildingId(7);
        let basis = timed_capacity(
            50,
            120,
            132,
            12,
            vec![ForecastAvailability {
                available_at: 132,
                amount: 100,
            }],
            vec![producer_fixture(producer, 120, vec![UnitKind::Harvester])],
        );
        let patient_foundry = deferrable_foundry(
            10,
            100,
            132,
            ProposalCase {
                urgency: Urgency::Developmental,
                confidence: Confidence::Supported,
                value: StrategicValue::Material,
                time_to_impact: TimeToImpact::Patient,
                safety: ExecutionSafety::Managed,
            },
        );
        let urgent_connected = with_jobs(
            offense_accepted_at(
                0,
                vec![],
                ProposalCase {
                    urgency: Urgency::Pressing,
                    confidence: Confidence::Current,
                    value: StrategicValue::Decisive,
                    time_to_impact: TimeToImpact::Immediate,
                    safety: ExecutionSafety::Secure,
                },
                120,
            ),
            vec![ProducerJobClaim::flexible(
                UnitKind::Harvester,
                120,
                400,
                vec![producer],
            )],
        );

        let first = allocate(
            &basis,
            vec![],
            vec![patient_foundry.clone(), urgent_connected.clone()],
            AllocationPersonality::default(),
        )
        .expect("aggregate bank and forecast fund both compatible domains");
        let reversed = allocate(
            &basis,
            vec![],
            vec![urgent_connected, patient_foundry],
            AllocationPersonality::default(),
        )
        .expect("input order cannot change the funding decision");

        assert_eq!(first, reversed);
        assert_eq!(first.accepted.len(), 2);
        let connected = first
            .producer_schedule
            .iter()
            .find(|job| {
                matches!(
                    job.owner,
                    ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(_))
                )
            })
            .expect("the urgent connected request was scheduled");
        assert_eq!(connected.enqueued_at, 120);
        assert_eq!((connected.current_scrap, connected.forecast_scrap), (50, 0));
        let foundry = first
            .capital_assignments
            .iter()
            .find(|assignment| {
                matches!(
                    assignment.owner,
                    ClaimOwner::Proposal(ProposalKey::FoundryExpansion(_))
                )
            })
            .expect("the Foundry capital received an exact split");
        assert_eq!((foundry.current_scrap, foundry.forecast_scrap), (0, 100));
        assert_eq!(
            u128::from(connected.current_scrap)
                + u128::from(connected.forecast_scrap)
                + u128::from(foundry.current_scrap)
                + u128::from(foundry.forecast_scrap),
            150,
            "producer and construction capital are each counted exactly once"
        );
    }

    #[test]
    fn stronger_foundry_uses_current_bank_and_defers_compatible_production() {
        let producer = BuildingId(7);
        let basis = timed_capacity(
            100,
            120,
            132,
            12,
            vec![ForecastAvailability {
                available_at: 132,
                amount: 50,
            }],
            vec![producer_fixture(producer, 120, vec![UnitKind::Harvester])],
        );
        let foundry = deferrable_foundry(
            10,
            100,
            132,
            ProposalCase {
                urgency: Urgency::Pressing,
                confidence: Confidence::Current,
                value: StrategicValue::Decisive,
                time_to_impact: TimeToImpact::Near,
                safety: ExecutionSafety::Secure,
            },
        );
        let connected = with_jobs(
            offense_accepted_at(
                0,
                vec![],
                ProposalCase {
                    urgency: Urgency::Developmental,
                    confidence: Confidence::Prior,
                    value: StrategicValue::Incremental,
                    time_to_impact: TimeToImpact::Patient,
                    safety: ExecutionSafety::Speculative,
                },
                120,
            ),
            vec![ProducerJobClaim::flexible(
                UnitKind::Harvester,
                120,
                400,
                vec![producer],
            )],
        );

        let result = allocate(
            &basis,
            vec![],
            vec![connected, foundry],
            AllocationPersonality::default(),
        )
        .expect("the lower-priority job can wait for the first safe income tick");

        assert_eq!(result.accepted.len(), 2);
        assert_eq!(result.producer_schedule[0].enqueued_at, 132);
        assert_eq!(result.producer_schedule[0].current_scrap, 0);
        assert_eq!(result.producer_schedule[0].forecast_scrap, 50);
        let foundry = result
            .capital_assignments
            .iter()
            .find(|assignment| {
                matches!(
                    assignment.owner,
                    ClaimOwner::Proposal(ProposalKey::FoundryExpansion(_))
                )
            })
            .expect("the stronger Foundry was funded");
        assert_eq!((foundry.current_scrap, foundry.forecast_scrap), (100, 0));
    }

    #[test]
    fn stronger_deferrable_capital_does_not_make_immediate_work_artificially_infeasible() {
        let producer = BuildingId(7);
        let kind = UnitKind::Harvester;
        let cost = kind.stats().cost;
        let observed_at = 120;
        let income_at = 132;
        let must_enqueue_before_income = observed_at + Tick::from(kind.stats().train_ticks);
        let basis = timed_capacity(
            cost,
            observed_at,
            must_enqueue_before_income,
            12,
            vec![ForecastAvailability {
                available_at: income_at,
                amount: 100,
            }],
            vec![producer_fixture(producer, observed_at, vec![kind])],
        );
        let foundry = deferrable_foundry(
            10,
            100,
            income_at,
            ProposalCase {
                urgency: Urgency::Pressing,
                confidence: Confidence::Current,
                value: StrategicValue::Decisive,
                time_to_impact: TimeToImpact::Near,
                safety: ExecutionSafety::Secure,
            },
        );
        let connected = with_jobs(
            offense_accepted_at(
                0,
                vec![],
                ProposalCase {
                    urgency: Urgency::Developmental,
                    confidence: Confidence::Prior,
                    value: StrategicValue::Incremental,
                    time_to_impact: TimeToImpact::Immediate,
                    safety: ExecutionSafety::Speculative,
                },
                observed_at,
            ),
            vec![ProducerJobClaim::flexible(
                kind,
                observed_at,
                must_enqueue_before_income,
                vec![producer],
            )],
        );

        let result = allocate(
            &basis,
            vec![],
            vec![connected, foundry],
            AllocationPersonality::default(),
        )
        .expect("both proposals are jointly fundable by their exact deadlines");

        assert_eq!(result.accepted.len(), 2);
        assert_eq!(result.producer_schedule[0].enqueued_at, observed_at);
        assert_eq!(result.producer_schedule[0].current_scrap, cost);
        assert_eq!(result.producer_schedule[0].forecast_scrap, 0);
        let foundry = result
            .capital_assignments
            .iter()
            .find(|assignment| {
                matches!(
                    assignment.owner,
                    ClaimOwner::Proposal(ProposalKey::FoundryExpansion(_))
                )
            })
            .expect("the higher-ranked Foundry remains fully funded");
        assert_eq!((foundry.current_scrap, foundry.forecast_scrap), (0, 100));
    }

    #[test]
    fn compatibility_fallback_does_not_spend_forecast_promised_to_later_work() {
        let producer = BuildingId(7);
        let kind = UnitKind::Harvester;
        let cost = kind.stats().cost;
        let observed_at = 120;
        let foundry_deadline = 132;
        let fixed_deadline = 240;
        let must_enqueue_before_income = observed_at + Tick::from(kind.stats().train_ticks);
        let basis = timed_capacity(
            cost,
            observed_at,
            fixed_deadline,
            12,
            vec![
                ForecastAvailability {
                    available_at: foundry_deadline,
                    amount: 100,
                },
                ForecastAvailability {
                    available_at: fixed_deadline,
                    amount: 100,
                },
            ],
            vec![producer_fixture(producer, observed_at, vec![kind])],
        );
        let prior = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: 10,
            key: ObligationKey::SavedFoundry {
                anchor: TilePos::new(20, 20),
            },
            claims: bundle(
                0,
                vec![ForecastClaim {
                    through: fixed_deadline,
                    amount: 100,
                }],
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        };
        let foundry = deferrable_foundry(
            10,
            100,
            foundry_deadline,
            ProposalCase {
                urgency: Urgency::Pressing,
                confidence: Confidence::Current,
                value: StrategicValue::Decisive,
                time_to_impact: TimeToImpact::Near,
                safety: ExecutionSafety::Secure,
            },
        );
        let connected = with_jobs(
            offense_accepted_at(
                0,
                vec![],
                ProposalCase {
                    urgency: Urgency::Developmental,
                    confidence: Confidence::Prior,
                    value: StrategicValue::Incremental,
                    time_to_impact: TimeToImpact::Immediate,
                    safety: ExecutionSafety::Speculative,
                },
                observed_at,
            ),
            vec![ProducerJobClaim::flexible(
                kind,
                observed_at,
                must_enqueue_before_income,
                vec![producer],
            )],
        );

        let result = allocate(
            &basis,
            vec![prior],
            vec![connected, foundry],
            AllocationPersonality::default(),
        )
        .expect("current cash plus both income events fund all three commitments");

        assert_eq!(result.accepted.len(), 2);
        assert_eq!(
            (
                result.producer_schedule[0].current_scrap,
                result.producer_schedule[0].forecast_scrap,
            ),
            (cost, 0)
        );
        let foundry = result
            .capital_assignments
            .iter()
            .find(|assignment| {
                matches!(
                    assignment.owner,
                    ClaimOwner::Proposal(ProposalKey::FoundryExpansion(_))
                )
            })
            .expect("the fresh Foundry has one exact split");
        assert_eq!((foundry.current_scrap, foundry.forecast_scrap), (0, 100));
    }

    #[test]
    fn remembered_connected_transition_keeps_priority_over_later_saved_foundry() {
        let producer = BuildingId(7);
        let basis = timed_capacity(
            50,
            120,
            132,
            12,
            vec![ForecastAvailability {
                available_at: 132,
                amount: 100,
            }],
            vec![producer_fixture(producer, 120, vec![UnitKind::Harvester])],
        );
        for connected_accepted_at in [90, 100] {
            let saved_owner = ClaimOwner::Obligation {
                class: ObligationClass::PersistentPlan,
                accepted_at: 100,
                key: ObligationKey::SavedFoundry {
                    anchor: TilePos::new(10, 10),
                },
            };
            let saved = ImportedObligation {
                class: ObligationClass::PersistentPlan,
                accepted_at: 100,
                key: ObligationKey::SavedFoundry {
                    anchor: TilePos::new(10, 10),
                },
                claims: bundle(0, vec![], vec![1], vec![], vec![site(10, 10)], vec![])
                    .with_deferrable_capital(DeferrableCapitalClaim {
                        through: 132,
                        amount: 100,
                    })
                    .unwrap(),
            };
            let connected = with_jobs(
                offense_accepted_at(0, vec![], ordinary_case(), connected_accepted_at),
                vec![ProducerJobClaim::flexible(
                    UnitKind::Harvester,
                    120,
                    400,
                    vec![producer],
                )],
            );

            let result = allocate(
                &basis,
                vec![saved],
                vec![connected],
                AllocationPersonality::default(),
            )
            .expect("an earlier remembered transition and saved Foundry both fit");

            assert_eq!(result.accepted.len(), 1);
            assert_eq!(result.producer_schedule[0].enqueued_at, 120);
            assert_eq!(result.producer_schedule[0].current_scrap, 50);
            assert_eq!(result.producer_schedule[0].forecast_scrap, 0);
            let saved_assignment = result
                .capital_assignments
                .iter()
                .find(|assignment| assignment.owner == saved_owner)
                .expect("the saved Foundry retains its full later capital");
            assert_eq!(
                (
                    saved_assignment.current_scrap,
                    saved_assignment.forecast_scrap
                ),
                (0, 100)
            );
        }
    }

    #[test]
    fn marginal_connected_scale_uses_only_residual_funding() {
        let producer = BuildingId(7);
        let basis = timed_capacity(
            150,
            120,
            132,
            12,
            vec![ForecastAvailability {
                available_at: 132,
                amount: 50,
            }],
            vec![producer_fixture(producer, 120, vec![UnitKind::Harvester])],
        );
        let foundry = deferrable_foundry(
            10,
            100,
            132,
            ProposalCase {
                urgency: Urgency::Pressing,
                ..ordinary_case()
            },
        );
        let connected_key = ConnectedOffenseKey {
            objective: BuildingId(90),
            anchor: TilePos::new(40, 10),
        };
        let connected = with_jobs(
            offense_accepted_at(
                0,
                vec![],
                ProposalCase {
                    urgency: Urgency::Developmental,
                    ..ordinary_case()
                },
                120,
            ),
            vec![ProducerJobClaim::flexible(
                UnitKind::Harvester,
                120,
                400,
                vec![producer],
            )],
        );
        let mut result = allocate(
            &basis,
            vec![],
            vec![foundry, connected],
            AllocationPersonality::default(),
        )
        .expect("the minimum and Foundry fit");
        let foundry_before = result
            .capital_assignments
            .iter()
            .copied()
            .find(|assignment| {
                matches!(
                    assignment.owner,
                    ClaimOwner::Proposal(ProposalKey::FoundryExpansion(_))
                )
            })
            .unwrap();
        let minimum_before = result.producer_schedule[0];
        let marginal = bundle(
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![ProducerJobClaim::flexible(
                UnitKind::Harvester,
                120,
                400,
                vec![producer],
            )],
        );

        result
            .try_extend_connected_offense(&basis, connected_key, &marginal)
            .expect("the marginal can use the one residual income payment");

        assert_eq!(
            result
                .capital_assignments
                .iter()
                .copied()
                .find(|assignment| assignment.owner == foundry_before.owner),
            Some(foundry_before)
        );
        assert_eq!(result.producer_schedule[0], minimum_before);
        assert_eq!(result.producer_schedule[1].enqueued_at, 132);
        assert_eq!(
            (
                result.producer_schedule[1].current_scrap,
                result.producer_schedule[1].forecast_scrap,
            ),
            (0, 50)
        );
    }

    #[test]
    fn production_cost_and_nonproduction_capital_are_each_charged_once() {
        let producer = BuildingId(7);
        let proposal = |current_scrap| {
            let mut proposal = offense(0, vec![], ordinary_case());
            let InvestmentProposal::ConnectedOffenseMinimum { claims, .. } = &mut proposal else {
                unreachable!()
            };
            *claims = bundle(
                40,
                vec![ForecastClaim {
                    through: 20,
                    amount: 30,
                }],
                vec![],
                vec![5],
                vec![],
                vec![ProducerJobClaim::flexible(
                    UnitKind::Sentinel,
                    0,
                    200,
                    vec![producer],
                )],
            );
            allocate(
                &capacity(
                    current_scrap,
                    200,
                    vec![ForecastAvailability {
                        available_at: 20,
                        amount: 30,
                    }],
                    vec![producer_fixture(producer, 0, vec![UnitKind::Sentinel])],
                ),
                vec![],
                vec![proposal],
                AllocationPersonality::default(),
            )
        };

        let exact = proposal(130).expect("40 current + 30 forecast + 90 training fits exactly");
        assert_eq!(exact.accepted.len(), 1);
        assert_eq!(exact.accepted[0].claims().claimed_capital(), 160);
        assert_eq!(exact.producer_schedule[0].current_scrap, 90);
        assert_eq!(exact.producer_schedule[0].forecast_scrap, 0);

        let short = proposal(129).expect("insufficient funding is a traced proposal rejection");
        assert!(matches!(
            short.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::ProductionFunding {
                    requested: 160,
                    available: 159,
                    ..
                }
            ))
        ));
    }

    #[test]
    fn fixed_obligation_keeps_accepted_timing_when_an_earlier_slot_is_legal() {
        let producer = BuildingId(7);
        let kind = UnitKind::Harvester;
        let enqueued_at = 20;
        let starts_at = 20;
        let ready_at = starts_at + Tick::from(kind.stats().train_ticks) - 1;
        let owner = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: 5,
            key: ObligationKey::ConnectedOffense {
                objective: BuildingId(90),
                anchor: TilePos::new(40, 10),
            },
            claims: bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    producer,
                    kind,
                    enqueued_at,
                    starts_at,
                    ready_at,
                    ready_at + 2,
                )],
            ),
        };

        let result: AllocationResult<(), ()> = allocate(
            &capacity(
                kind.stats().cost,
                ready_at + 2,
                vec![],
                vec![producer_fixture(producer, 0, vec![kind])],
            ),
            vec![owner],
            vec![],
            AllocationPersonality::default(),
        )
        .expect("the accepted later slot remains legal");

        let [scheduled] = result.producer_schedule.as_slice() else {
            panic!("one fixed job must remain in the schedule")
        };
        assert_eq!(scheduled.producer, producer);
        assert_eq!(scheduled.enqueued_at, enqueued_at);
        assert_eq!(scheduled.starts_at, starts_at);
        assert_eq!(scheduled.ready_at, ready_at);
    }

    #[test]
    fn fixed_obligation_rebases_funding_without_changing_its_schedule() {
        let producer = BuildingId(7);
        let kind = UnitKind::Harvester;
        let cost = kind.stats().cost;
        let enqueued_at = 20;
        let ready_at = enqueued_at + Tick::from(kind.stats().train_ticks) - 1;
        let obligation = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: 5,
            key: ObligationKey::ConnectedOffense {
                objective: BuildingId(90),
                anchor: TilePos::new(40, 10),
            },
            claims: bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    producer,
                    kind,
                    enqueued_at,
                    enqueued_at,
                    ready_at,
                    ready_at + 2,
                )],
            ),
        };
        let lane = || {
            ProducerPlanningProjection::fixture(producer, 0, 1, 0, vec![0; QUEUE_CAP], vec![kind])
                .expect("the producer fixture is valid")
        };
        let later_lane = || {
            ProducerPlanningProjection::fixture(
                producer,
                12,
                1,
                12,
                vec![12; QUEUE_CAP],
                vec![kind],
            )
            .expect("the later producer fixture is valid")
        };

        let forecast_funded: AllocationResult<(), ()> = allocate(
            &capacity(
                0,
                ready_at + 2,
                vec![ForecastAvailability {
                    available_at: enqueued_at,
                    amount: cost,
                }],
                vec![lane()],
            ),
            vec![obligation.clone()],
            vec![],
            AllocationPersonality::default(),
        )
        .expect("the accepted job is initially forecast funded");
        assert_eq!(forecast_funded.producer_schedule[0].current_scrap, 0);
        assert_eq!(forecast_funded.producer_schedule[0].forecast_scrap, cost);

        let current_funded: AllocationResult<(), ()> = allocate(
            &timed_capacity(cost, 12, ready_at + 2, 1, vec![], vec![later_lane()]),
            vec![obligation.clone()],
            vec![],
            AllocationPersonality::default(),
        )
        .expect("matured income becomes current funding without changing the job");
        let scheduled = &current_funded.producer_schedule[0];
        assert_eq!(scheduled.enqueued_at, enqueued_at);
        assert_eq!(scheduled.starts_at, enqueued_at);
        assert_eq!(scheduled.ready_at, ready_at);
        assert_eq!(scheduled.current_scrap, cost);
        assert_eq!(scheduled.forecast_scrap, 0);

        assert!(matches!(
            allocate::<(), ()>(
                &timed_capacity(
                    cost - 1,
                    12,
                    ready_at + 2,
                    1,
                    vec![],
                    vec![later_lane()],
                ),
                vec![obligation],
                vec![],
                AllocationPersonality::default(),
            ),
            Err(AllocationError::ObligationConflict {
                conflict: AllocationConflict::ProductionFunding {
                    through,
                    requested,
                    available,
                },
                ..
            }) if through == enqueued_at
                && requested == u128::from(cost)
                && available == u128::from(cost - 1)
        ));
    }

    #[test]
    fn joint_scheduler_assigns_staged_hundred_scrap_payments_globally() {
        let first = BuildingId(7);
        let second = BuildingId(8);
        let result = allocate(
            &capacity(
                0,
                230,
                vec![
                    ForecastAvailability {
                        available_at: 10,
                        amount: 100,
                    },
                    ForecastAvailability {
                        available_at: 20,
                        amount: 100,
                    },
                ],
                vec![
                    producer_fixture(first, 0, vec![UnitKind::Sentinel]),
                    producer_fixture(second, 0, vec![UnitKind::Lancer]),
                ],
            ),
            vec![],
            vec![
                with_jobs(
                    foundry(10, 0, vec![], ordinary_case()),
                    vec![ProducerJobClaim::flexible(
                        UnitKind::Sentinel,
                        10,
                        200,
                        vec![first],
                    )],
                ),
                with_jobs(
                    offense(0, vec![], ordinary_case()),
                    vec![ProducerJobClaim::flexible(
                        UnitKind::Lancer,
                        10,
                        230,
                        vec![second],
                    )],
                ),
            ],
            AllocationPersonality::default(),
        )
        .expect("the second portfolio job may wait for the second payment");

        assert_eq!(result.accepted.len(), 2);
        assert_eq!(result.producer_schedule[0].enqueued_at, 10);
        assert_eq!(result.producer_schedule[0].forecast_scrap, 90);
        assert_eq!(result.producer_schedule[1].enqueued_at, 20);
        assert_eq!(result.producer_schedule[1].forecast_scrap, 110);
    }

    #[test]
    fn enqueue_lower_bounds_snap_to_the_global_bot_cadence() {
        let producer = BuildingId(7);
        let lane = ProducerPlanningProjection::fixture(
            producer,
            12,
            12,
            12,
            vec![12; QUEUE_CAP],
            vec![UnitKind::Sentinel],
        )
        .expect("the producer is observed on a global decision boundary");
        let result = allocate(
            &timed_capacity(90, 12, 200, 12, Vec::new(), vec![lane]),
            vec![],
            vec![with_jobs(
                offense(0, vec![], ordinary_case()),
                vec![ProducerJobClaim::flexible(
                    UnitKind::Sentinel,
                    13,
                    174,
                    vec![producer],
                )],
            )],
            AllocationPersonality::default(),
        )
        .expect("a between-cadence lower bound snaps forward");

        assert_eq!(result.producer_schedule[0].enqueued_at, 24);
        assert_eq!(result.producer_schedule[0].starts_at, 24);
        assert_eq!(result.producer_schedule[0].ready_at, 173);

        let lane = ProducerPlanningProjection::fixture(
            producer,
            12,
            12,
            12,
            vec![12; QUEUE_CAP],
            vec![UnitKind::Sentinel],
        )
        .expect("the producer is observed on a global decision boundary");
        let bounded = allocate(
            &timed_capacity(90, 12, 23, 12, Vec::new(), vec![lane]),
            vec![],
            vec![with_jobs(
                offense(0, vec![], ordinary_case()),
                vec![ProducerJobClaim::flexible(
                    UnitKind::Sentinel,
                    13,
                    174,
                    vec![producer],
                )],
            )],
            AllocationPersonality::default(),
        )
        .expect("the bounded candidate is rejected traceably");
        assert!(matches!(
            bounded.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::ProducerSchedule { .. }
            ))
        ));
    }

    #[test]
    fn joint_production_matches_an_independent_two_job_cash_oracle() {
        fn oracle(
            current: u32,
            income: &[ForecastAvailability],
            releases: [Tick; 2],
            deadlines: [Tick; 2],
        ) -> Option<Vec<(Tick, BuildingId, u8)>> {
            let duration = Tick::from(UnitKind::Harvester.stats().train_ticks);
            let cost = u128::from(UnitKind::Harvester.stats().cost);
            let mut best: Option<Vec<(Tick, BuildingId, u8)>> = None;
            for first_lane in [BuildingId(7), BuildingId(8)] {
                for second_lane in [BuildingId(7), BuildingId(8)] {
                    let Some(first_latest) = deadlines[0].checked_sub(duration) else {
                        continue;
                    };
                    let Some(second_latest) = deadlines[1].checked_sub(duration) else {
                        continue;
                    };
                    for first_start in releases[0]..=first_latest {
                        for second_start in releases[1]..=second_latest {
                            if first_lane == second_lane
                                && first_start < second_start.saturating_add(duration)
                                && second_start < first_start.saturating_add(duration)
                            {
                                continue;
                            }
                            let cash_fits = [first_start, second_start].into_iter().all(|tick| {
                                let jobs_due = u128::from(first_start <= tick)
                                    + u128::from(second_start <= tick);
                                let available = u128::from(current)
                                    + income
                                        .iter()
                                        .filter(|row| row.available_at <= tick)
                                        .map(|row| u128::from(row.amount))
                                        .sum::<u128>();
                                jobs_due * cost <= available
                            });
                            if !cash_fits {
                                continue;
                            }
                            let mut rows =
                                vec![(first_start, first_lane, 0), (second_start, second_lane, 1)];
                            rows.sort_unstable();
                            if best.as_ref().is_none_or(|current| rows < *current) {
                                best = Some(rows);
                            }
                        }
                    }
                }
            }
            best
        }

        let income_cases = [
            vec![],
            vec![ForecastAvailability {
                available_at: 10,
                amount: 50,
            }],
            vec![ForecastAvailability {
                available_at: 20,
                amount: 100,
            }],
            vec![
                ForecastAvailability {
                    available_at: 10,
                    amount: 50,
                },
                ForecastAvailability {
                    available_at: 20,
                    amount: 50,
                },
            ],
        ];
        for current in [0, 50, 100] {
            for income in &income_cases {
                for deadlines in [[101, 101], [111, 121], [121, 121]] {
                    for releases in [[0, 0], [0, 11], [11, 11]] {
                        let expected = oracle(current, income, releases, deadlines);
                        let producers = vec![
                            producer_fixture(BuildingId(7), 0, vec![UnitKind::Harvester]),
                            producer_fixture(BuildingId(8), 0, vec![UnitKind::Harvester]),
                        ];
                        let proposals = vec![
                            with_jobs(
                                foundry(10, 0, vec![], ordinary_case()),
                                vec![ProducerJobClaim::flexible(
                                    UnitKind::Harvester,
                                    releases[0],
                                    deadlines[0],
                                    vec![BuildingId(7), BuildingId(8)],
                                )],
                            ),
                            with_jobs(
                                offense(0, vec![], ordinary_case()),
                                vec![ProducerJobClaim::flexible(
                                    UnitKind::Harvester,
                                    releases[1],
                                    deadlines[1],
                                    vec![BuildingId(7), BuildingId(8)],
                                )],
                            ),
                        ];
                        let horizon = deadlines.into_iter().max().unwrap_or(1);
                        let result = allocate(
                            &capacity(current, horizon, income.clone(), producers),
                            vec![],
                            proposals,
                            AllocationPersonality::default(),
                        )
                        .expect("the oracle fixture has valid proposal structure");

                        assert_eq!(
                            result.accepted.len() == 2,
                            expected.is_some(),
                            "current={current}, income={income:?}, releases={releases:?}, deadlines={deadlines:?}"
                        );
                        if let Some(expected) = expected {
                            let actual: Vec<_> = result
                                .producer_schedule
                                .iter()
                                .map(|row| {
                                    let domain = match row.owner {
                                        ClaimOwner::Proposal(ProposalKey::FoundryExpansion(_)) => 0,
                                        ClaimOwner::Proposal(
                                            ProposalKey::ConnectedOffenseMinimum(_),
                                        ) => 1,
                                        ClaimOwner::Obligation { .. } => {
                                            unreachable!("the oracle submits no obligations")
                                        }
                                    };
                                    (row.starts_at, row.producer, domain)
                                })
                                .collect();
                            assert_eq!(actual, expected);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn wealthy_eighteen_job_package_has_a_bounded_exact_search() {
        let producers = [BuildingId(7), BuildingId(8), BuildingId(9)];
        let jobs = (0..18)
            .map(|_| ProducerJobClaim::flexible(UnitKind::Sentinel, 0, 2_000, producers.to_vec()))
            .collect();
        let result = allocate(
            &capacity(
                2_000,
                0,
                vec![],
                producers
                    .into_iter()
                    .map(|producer| producer_fixture(producer, 0, vec![UnitKind::Sentinel]))
                    .collect(),
            ),
            vec![],
            vec![with_jobs(offense(0, vec![], ordinary_case()), jobs)],
            AllocationPersonality::default(),
        )
        .expect("the wealthy package fits three ordinary producer lanes");

        assert_eq!(result.accepted.len(), 1);
        assert_eq!(result.producer_schedule.len(), 18);
        assert!(
            result.production_search_states <= 19,
            "the canonical successful path should visit at most one state per request: {}",
            result.production_search_states
        );
    }

    #[test]
    fn failed_state_memo_merges_equivalent_cross_owner_interleavings() {
        let producer = BuildingId(7);
        let economy = ClaimOwner::Proposal(ProposalKey::FoundryExpansion(FoundryExpansionKey {
            anchor: TilePos::new(10, 10),
        }));
        let offense =
            ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(ConnectedOffenseKey {
                objective: BuildingId(90),
                anchor: TilePos::new(40, 10),
            }));
        let claim = || ProducerJobClaim::flexible(UnitKind::Sentinel, 0, 301, vec![producer]);
        let jobs = vec![
            OwnedProducerJob {
                claim: claim(),
                owner: economy,
                ordinal: 0,
                funding_priority: FundingPriority::fresh_proposal(economy, 0),
            },
            OwnedProducerJob {
                claim: claim(),
                owner: economy,
                ordinal: 1,
                funding_priority: FundingPriority::fresh_proposal(economy, 0),
            },
            OwnedProducerJob {
                claim: claim(),
                owner: offense,
                ordinal: 0,
                funding_priority: FundingPriority::fresh_proposal(offense, 1),
            },
        ];
        let basis = capacity(
            270,
            0,
            vec![],
            vec![producer_fixture(producer, 0, vec![UnitKind::Sentinel])],
        );
        let mut producer_state = basis.resources.producers().to_vec();
        let mut remaining = vec![true; jobs.len()];
        let mut schedule = Vec::new();
        let mut search = ProductionPortfolioSearch {
            capacity: &basis,
            jobs: &jobs,
            current_capital: 0,
            forecast_capital: &[],
            deferrable_capital: &[],
            failed: BTreeSet::new(),
            explored_states: 0,
            memo_hits: 0,
            funding_mode: JointFundingMode::PreferPriority,
        };

        assert!(!search.find(
            &mut producer_state,
            &mut remaining,
            &mut schedule,
            &mut Vec::new(),
        ));
        assert!(search.memo_hits > 0);
        assert!(search.explored_states < 12);
    }

    #[test]
    fn accepted_unpaid_future_job_joins_the_shared_cash_and_lane_schedule() {
        let producer = BuildingId(7);
        let basis = capacity(
            140,
            0,
            vec![],
            vec![producer_fixture(
                producer,
                0,
                vec![UnitKind::Harvester, UnitKind::Sentinel],
            )],
        );
        let accepted_future_job = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: 10,
            key: ObligationKey::PaidConstruction(BuildingId(20)),
            claims: bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    producer,
                    UnitKind::Harvester,
                    0,
                    0,
                    99,
                    500,
                )],
            ),
        };
        let mut attack = offense(0, vec![], ordinary_case());
        let InvestmentProposal::ConnectedOffenseMinimum { claims, .. } = &mut attack else {
            unreachable!()
        };
        *claims = bundle(
            0,
            vec![],
            vec![],
            vec![5],
            vec![],
            vec![ProducerJobClaim::flexible(
                UnitKind::Sentinel,
                0,
                251,
                vec![producer],
            )],
        );

        let result = allocate(
            &basis,
            vec![accepted_future_job],
            vec![attack],
            AllocationPersonality::default(),
        )
        .expect("accepted future work retains its exact lane prefix");

        assert_eq!(result.accepted.len(), 1);
        assert_eq!(
            result
                .producer_schedule
                .iter()
                .map(|row| row.current_scrap)
                .sum::<u32>(),
            140
        );
        assert_eq!(result.producer_schedule[0].kind, UnitKind::Harvester);
        assert_eq!(result.producer_schedule[1].kind, UnitKind::Sentinel);
        assert_eq!(result.producer_schedule[1].ready_at, 249);
    }

    #[test]
    fn connected_offense_extensions_are_atomic_and_continue_job_ordinals() {
        let producer = BuildingId(7);
        let key = ConnectedOffenseKey {
            objective: BuildingId(90),
            anchor: TilePos::new(40, 10),
        };
        let basis = capacity(
            180,
            0,
            vec![],
            vec![producer_fixture(producer, 0, vec![UnitKind::Sentinel])],
        );
        let one_job = || {
            bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::flexible(
                    UnitKind::Sentinel,
                    0,
                    500,
                    vec![producer],
                )],
            )
        };
        let mut result = allocate(
            &basis,
            vec![],
            vec![with_jobs(
                offense(0, vec![], ordinary_case()),
                one_job().producer_jobs,
            )],
            AllocationPersonality::default(),
        )
        .expect("the minimum connected package is selected");

        let schedule = result
            .try_extend_connected_offense(&basis, key, &one_job())
            .expect("one marginal package fits the residual capital and lane");
        assert_eq!(
            schedule
                .iter()
                .map(|job| job.request_ordinal)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(schedule[1].ready_at, 299);

        let before_failure = result.clone();
        assert!(matches!(
            result.try_extend_connected_offense(&basis, key, &one_job()),
            Err(AllocationConflict::ProductionFunding { .. })
        ));
        assert_eq!(result, before_failure);

        let inactive = ConnectedOffenseKey {
            objective: BuildingId(91),
            anchor: TilePos::new(41, 10),
        };
        assert_eq!(
            result.try_extend_connected_offense(&basis, inactive, &ClaimBundle::default()),
            Err(AllocationConflict::InactiveProposal(
                ProposalKey::ConnectedOffenseMinimum(inactive)
            ))
        );
        assert_eq!(result, before_failure);
    }

    #[test]
    fn future_obligation_does_not_monopolize_an_idle_lane_prefix() {
        let producer = BuildingId(7);
        let obligation = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: 10,
            key: ObligationKey::ConnectedOffense {
                objective: BuildingId(90),
                anchor: TilePos::new(40, 10),
            },
            claims: bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    producer,
                    UnitKind::Harvester,
                    500,
                    500,
                    599,
                    601,
                )],
            ),
        };
        let proposal = with_jobs(
            offense(0, vec![], ordinary_case()),
            vec![ProducerJobClaim::flexible(
                UnitKind::Sentinel,
                0,
                151,
                vec![producer],
            )],
        );

        let result = allocate(
            &capacity(
                140,
                0,
                vec![],
                vec![producer_fixture(
                    producer,
                    0,
                    vec![UnitKind::Harvester, UnitKind::Sentinel],
                )],
            ),
            vec![obligation],
            vec![proposal],
            AllocationPersonality::default(),
        )
        .expect("idle capacity before a retained future job remains usable");

        assert_eq!(result.accepted.len(), 1);
        assert_eq!(result.producer_schedule[0].kind, UnitKind::Sentinel);
        assert_eq!(result.producer_schedule[0].starts_at, 0);
        assert_eq!(result.producer_schedule[1].kind, UnitKind::Harvester);
        assert_eq!(result.producer_schedule[1].starts_at, 500);
    }

    #[test]
    fn assigned_obligations_retain_chronological_lane_order_before_class() {
        let producer = BuildingId(7);
        let older = ImportedObligation {
            class: ObligationClass::PaidWork,
            accepted_at: 0,
            key: ObligationKey::PaidConstruction(BuildingId(20)),
            claims: bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    producer,
                    UnitKind::Harvester,
                    0,
                    0,
                    99,
                    101,
                )],
            ),
        };
        let newer = ImportedObligation {
            class: ObligationClass::Survival,
            accepted_at: 10,
            key: ObligationKey::OpeningCore { sequence: 0 },
            claims: bundle(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    producer,
                    UnitKind::Sentinel,
                    0,
                    100,
                    249,
                    400,
                )],
            ),
        };
        let basis = capacity(
            140,
            0,
            vec![],
            vec![producer_fixture(
                producer,
                0,
                vec![UnitKind::Harvester, UnitKind::Sentinel],
            )],
        );

        let forward = allocate::<(), ()>(
            &basis,
            vec![older.clone(), newer.clone()],
            vec![],
            AllocationPersonality::default(),
        )
        .expect("chronological obligations fit the retained lane");
        let reverse = allocate::<(), ()>(
            &basis,
            vec![newer, older],
            vec![],
            AllocationPersonality::default(),
        )
        .expect("input order does not change retained lane order");

        assert_eq!(forward, reverse);
        assert_eq!(forward.producer_schedule[0].kind, UnitKind::Harvester);
        assert_eq!(forward.producer_schedule[0].ready_at, 99);
        assert_eq!(forward.producer_schedule[1].kind, UnitKind::Sentinel);
        assert_eq!(forward.producer_schedule[1].ready_at, 249);
    }

    #[test]
    fn one_owners_request_ordinals_preserve_funding_order_across_lanes() {
        let first = BuildingId(7);
        let second = BuildingId(8);
        let proposal = with_jobs(
            offense(0, vec![], ordinary_case()),
            vec![
                ProducerJobClaim::flexible(UnitKind::Harvester, 100, 201, vec![first]),
                ProducerJobClaim::flexible(UnitKind::Sentinel, 0, 251, vec![second]),
            ],
        );
        let result = allocate(
            &capacity(
                140,
                0,
                vec![],
                vec![
                    producer_fixture(first, 0, vec![UnitKind::Harvester]),
                    producer_fixture(second, 0, vec![UnitKind::Sentinel]),
                ],
            ),
            vec![],
            vec![proposal],
            AllocationPersonality::default(),
        )
        .expect("ordered requests can begin concurrently but not in reverse order");

        assert_eq!(result.producer_schedule[0].request_ordinal, 0);
        assert_eq!(result.producer_schedule[0].starts_at, 100);
        assert_eq!(result.producer_schedule[1].request_ordinal, 1);
        assert_eq!(result.producer_schedule[1].starts_at, 100);
    }

    #[test]
    fn observed_queue_cursor_delays_fresh_work_without_reclaiming_its_scrap() {
        let producer = BuildingId(7);
        let result = allocate(
            &capacity(
                90,
                0,
                vec![],
                vec![producer_fixture(producer, 400, vec![UnitKind::Sentinel])],
            ),
            vec![],
            vec![{
                let mut attack = offense(0, vec![], ordinary_case());
                let InvestmentProposal::ConnectedOffenseMinimum { claims, .. } = &mut attack else {
                    unreachable!()
                };
                *claims = bundle(
                    0,
                    vec![],
                    vec![],
                    vec![5],
                    vec![],
                    vec![ProducerJobClaim::flexible(
                        UnitKind::Sentinel,
                        0,
                        550,
                        vec![producer],
                    )],
                );
                attack
            }],
            AllocationPersonality::default(),
        )
        .expect("the observed queue cursor is already-paid capacity evidence");

        assert_eq!(result.producer_schedule[0].current_scrap, 90);
        assert_eq!(result.producer_schedule[0].starts_at, 400);
        assert_eq!(result.producer_schedule[0].ready_at, 549);
    }

    #[test]
    fn income_phase_credit_is_usable_only_at_its_encoded_spendable_tick() {
        let producer = BuildingId(7);
        let basis = capacity(
            0,
            11,
            vec![ForecastAvailability {
                available_at: 11,
                amount: UnitKind::Harvester.stats().cost,
            }],
            vec![producer_fixture(producer, 0, vec![UnitKind::Harvester])],
        );
        let proposal = |deadline| {
            with_jobs(
                offense(0, vec![], ordinary_case()),
                vec![ProducerJobClaim::flexible(
                    UnitKind::Harvester,
                    10,
                    deadline,
                    vec![producer],
                )],
            )
        };

        let too_early = allocate(
            &basis,
            vec![],
            vec![proposal(110)],
            AllocationPersonality::default(),
        )
        .expect("funding lateness is a proposal-local rejection");
        assert!(matches!(
            too_early.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::ProductionFunding {
                    through: 10,
                    requested: 50,
                    available: 0,
                }
            ))
        ));

        let exact = allocate(
            &basis,
            vec![],
            vec![proposal(111)],
            AllocationPersonality::default(),
        )
        .expect("credit is spendable on its encoded command boundary");
        assert_eq!(exact.producer_schedule[0].starts_at, 11);
        assert_eq!(exact.producer_schedule[0].ready_at, 110);
        assert_eq!(exact.producer_schedule[0].forecast_scrap, 50);
    }

    #[test]
    fn producer_access_and_strict_deadline_edges_are_enforced() {
        let producer = BuildingId(7);
        let basis = capacity(
            u32::MAX,
            0,
            vec![],
            vec![producer_fixture(producer, 100, vec![UnitKind::Sentinel])],
        );
        let job = |kind, ready_before| {
            bundle(
                0,
                vec![],
                vec![],
                vec![5],
                vec![],
                vec![ProducerJobClaim::flexible(
                    kind,
                    0,
                    ready_before,
                    vec![producer],
                )],
            )
        };
        let proposal = |claims| -> InvestmentProposal<&'static str, &'static str> {
            InvestmentProposal::ConnectedOffenseMinimum {
                key: ConnectedOffenseKey {
                    objective: BuildingId(90),
                    anchor: TilePos::new(40, 10),
                },
                case: ordinary_case(),
                accepted_at: 0,
                claims,
                payload: "attack",
            }
        };

        let exact = allocate(
            &basis,
            vec![],
            vec![proposal(job(UnitKind::Sentinel, 250))],
            AllocationPersonality::default(),
        )
        .expect("completion on the tick before the deadline fits");
        assert_eq!(exact.accepted.len(), 1);
        assert_eq!(exact.producer_schedule[0].ready_at, 249);

        let late = allocate(
            &basis,
            vec![],
            vec![proposal(job(UnitKind::Sentinel, 249))],
            AllocationPersonality::default(),
        )
        .expect("a late proposal is traceably rejected");
        assert!(matches!(
            late.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::ProducerSchedule { .. }
            ))
        ));

        let inaccessible = allocate(
            &basis,
            vec![],
            vec![proposal(job(UnitKind::Moth, 2_000))],
            AllocationPersonality::default(),
        )
        .expect("partial access is a proposal rejection");
        assert!(matches!(
            inaccessible.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::ProducerAccess {
                    kind: UnitKind::Moth,
                    ref eligible_producers,
                }
            )) if eligible_producers == &[BuildingId(7)]
        ));

        let mixed = allocate(
            &basis,
            vec![],
            vec![proposal(bundle(
                0,
                vec![],
                vec![],
                vec![5],
                vec![],
                vec![ProducerJobClaim::flexible(
                    UnitKind::Sentinel,
                    0,
                    250,
                    vec![producer, BuildingId(99)],
                )],
            ))],
            AllocationPersonality::default(),
        )
        .expect("malformed access evidence remains a proposal-local rejection");
        assert!(matches!(
            mixed.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::UnknownProducer(BuildingId(99))
            ))
        ));
    }

    #[test]
    fn personality_flips_marginal_choice_but_not_domain_access() {
        let proposals = || {
            vec![
                foundry(10, 100, vec![], ordinary_case()),
                offense(100, vec![], ordinary_case()),
            ]
        };
        let basis = capacity(100, 0, vec![], vec![]);
        let economic = allocate(
            &basis,
            vec![],
            proposals(),
            AllocationPersonality {
                economy: 50,
                offense: 0,
            },
        )
        .expect("economic personality allocation");
        let aggressive = allocate(
            &basis,
            vec![],
            proposals(),
            AllocationPersonality {
                economy: 0,
                offense: 50,
            },
        )
        .expect("offensive personality allocation");

        assert!(matches!(
            economic.accepted[0],
            InvestmentProposal::FoundryExpansion { .. }
        ));
        assert!(matches!(
            aggressive.accepted[0],
            InvestmentProposal::ConnectedOffenseMinimum { .. }
        ));
        for result in [&economic, &aggressive] {
            assert_eq!(result.decisions.len(), 2);
            assert!(
                result
                    .decisions
                    .iter()
                    .all(|decision| decision.personality_weight >= BASE_PERSONALITY_WEIGHT)
            );
        }
    }

    #[test]
    fn one_pressing_case_outranks_two_developmental_cases() {
        let developmental = ProposalCase {
            urgency: Urgency::Developmental,
            confidence: Confidence::Current,
            value: StrategicValue::Decisive,
            time_to_impact: TimeToImpact::Immediate,
            safety: ExecutionSafety::Secure,
        };
        let pressing = ProposalCase {
            urgency: Urgency::Pressing,
            confidence: Confidence::Prior,
            value: StrategicValue::Incremental,
            time_to_impact: TimeToImpact::Patient,
            safety: ExecutionSafety::Speculative,
        };
        let pressing_rank = portfolio_rank(
            1,
            &[foundry(10, 0, vec![], pressing)],
            AllocationPersonality {
                economy: 0,
                offense: u16::MAX,
            },
        );
        let developmental_rank = portfolio_rank(
            3,
            &[
                foundry(10, 0, vec![], developmental),
                offense(0, vec![], developmental),
            ],
            AllocationPersonality {
                economy: u16::MAX,
                offense: u16::MAX,
            },
        );

        assert!(pressing_rank > developmental_rank);
        assert_eq!(
            outranking_basis(&pressing_rank, &developmental_rank),
            Some(OutrankingBasis::Urgency)
        );
    }

    #[test]
    fn personality_is_named_as_the_basis_only_after_semantic_bands_tie() {
        let proposals = [
            foundry(10, 100, vec![], ordinary_case()),
            offense(100, vec![], ordinary_case()),
        ];
        let personality = AllocationPersonality {
            economy: 40,
            offense: 10,
        };
        let expansion = portfolio_rank(1, &proposals, personality);
        let offense = portfolio_rank(2, &proposals, personality);

        assert!(expansion > offense);
        assert_eq!(
            outranking_basis(&expansion, &offense),
            Some(OutrankingBasis::Personality)
        );
    }

    #[test]
    fn lower_capital_then_structural_key_break_equal_semantic_ties() {
        let basis = capacity(100, 0, vec![], vec![]);
        let cheaper_offense = allocate(
            &basis,
            vec![],
            vec![
                foundry(10, 100, vec![], ordinary_case()),
                offense(90, vec![], ordinary_case()),
            ],
            AllocationPersonality::default(),
        )
        .expect("capital tie-break");
        assert!(matches!(
            cheaper_offense.accepted[0],
            InvestmentProposal::ConnectedOffenseMinimum { .. }
        ));

        let structural = allocate(
            &basis,
            vec![],
            vec![
                foundry(10, 100, vec![], ordinary_case()),
                offense(100, vec![], ordinary_case()),
            ],
            AllocationPersonality::default(),
        )
        .expect("structural tie-break");
        assert!(matches!(
            structural.accepted[0],
            InvestmentProposal::FoundryExpansion { .. }
        ));

        let capital_proposals = [
            foundry(10, 100, vec![], ordinary_case()),
            offense(90, vec![], ordinary_case()),
        ];
        let expensive = portfolio_rank(1, &capital_proposals, AllocationPersonality::default());
        let cheap = portfolio_rank(2, &capital_proposals, AllocationPersonality::default());
        assert_eq!(
            outranking_basis(&cheap, &expensive),
            Some(OutrankingBasis::LowerCapital)
        );

        let structural_proposals = [
            foundry(10, 100, vec![], ordinary_case()),
            offense(100, vec![], ordinary_case()),
        ];
        let canonical = portfolio_rank(1, &structural_proposals, AllocationPersonality::default());
        let later = portfolio_rank(2, &structural_proposals, AllocationPersonality::default());
        assert_eq!(
            outranking_basis(&canonical, &later),
            Some(OutrankingBasis::StructuralKey)
        );
    }

    #[test]
    fn imported_obligations_are_mandatory_and_trace_exact_conflicts() {
        let obligation = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: 12,
            key: ObligationKey::SavedFoundry {
                anchor: TilePos::new(10, 10),
            },
            claims: bundle(40, vec![], vec![1], vec![], vec![site(10, 10)], vec![]),
        };
        let result = allocate(
            &capacity(140, 0, vec![], vec![]),
            vec![obligation],
            vec![foundry(10, 100, vec![], ordinary_case())],
            AllocationPersonality::default(),
        )
        .expect("a fresh conflict does not displace prior accepted work");

        assert!(result.accepted.is_empty());
        assert!(matches!(
            result.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::Actor {
                    unit: UnitId(1),
                    existing: ClaimOwner::Obligation { .. },
                }
            ))
        ));
    }

    #[test]
    fn late_schedule_failure_restores_a_nonempty_claim_state_atomically() {
        let producer = BuildingId(7);
        let basis = capacity(
            200,
            100,
            vec![ForecastAvailability {
                available_at: 100,
                amount: 100,
            }],
            vec![producer_fixture(producer, 0, vec![UnitKind::Sentinel])],
        );
        let mut state = ClaimState::default();
        state
            .try_apply(
                &basis,
                ClaimOwner::Obligation {
                    class: ObligationClass::PaidWork,
                    accepted_at: 0,
                    key: ObligationKey::PaidConstruction(BuildingId(20)),
                },
                &bundle(10, vec![], vec![2], vec![], vec![site(20, 20)], vec![]),
            )
            .expect("the preexisting obligation is valid");
        let before = state.clone();
        let owner = ClaimOwner::Proposal(ProposalKey::FoundryExpansion(FoundryExpansionKey {
            anchor: TilePos::new(10, 10),
        }));
        let claims = bundle(
            20,
            vec![ForecastClaim {
                through: 100,
                amount: 20,
            }],
            vec![1],
            vec![5],
            vec![site(10, 10)],
            vec![ProducerJobClaim::flexible(
                UnitKind::Sentinel,
                0,
                100,
                vec![producer],
            )],
        );

        assert!(matches!(
            state.try_apply(&basis, owner, &claims),
            Err(AllocationConflict::ProducerSchedule { .. })
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn bounded_forecast_rejects_claims_beyond_its_horizon() {
        let result = allocate(
            &capacity(
                0,
                500,
                vec![ForecastAvailability {
                    available_at: 500,
                    amount: 100,
                }],
                vec![],
            ),
            vec![],
            vec![offense(
                0,
                vec![ForecastClaim {
                    through: 501,
                    amount: 1,
                }],
                ordinary_case(),
            )],
            AllocationPersonality::default(),
        )
        .expect("an out-of-horizon proposal is rejected, not imported");

        assert!(matches!(
            result.decisions[0].disposition,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(
                AllocationConflict::ForecastHorizon {
                    through: 501,
                    horizon: 500,
                }
            ))
        ));
    }

    #[test]
    fn claim_bundle_rejects_internal_actor_and_site_aliasing() {
        assert_eq!(
            ClaimBundle::new(0, vec![], vec![UnitId(1)], vec![UnitId(1)], vec![], vec![]),
            Err(ClaimBundleError::ActorRoleOverlap(UnitId(1)))
        );
        let first = site(10, 10);
        let second = site(11, 11);
        assert_eq!(
            ClaimBundle::new(0, vec![], vec![], vec![], vec![second, first], vec![]),
            Err(ClaimBundleError::OverlappingSites { first, second })
        );
    }

    #[test]
    fn flexible_capital_cannot_alias_a_fixed_split_or_second_deadline() {
        let claim = DeferrableCapitalClaim {
            through: 100,
            amount: 50,
        };
        assert_eq!(
            ClaimBundle::new(1, vec![], vec![], vec![], vec![], vec![])
                .unwrap()
                .with_deferrable_capital(claim),
            Err(ClaimBundleError::MixedCapitalFunding)
        );
        assert_eq!(
            ClaimBundle::new(
                0,
                vec![ForecastClaim {
                    through: 100,
                    amount: 1,
                }],
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .unwrap()
            .with_deferrable_capital(claim),
            Err(ClaimBundleError::MixedCapitalFunding)
        );
        assert_eq!(
            ClaimBundle::default()
                .with_deferrable_capital(claim)
                .unwrap()
                .with_deferrable_capital(claim),
            Err(ClaimBundleError::DuplicateDeferrableCapital)
        );
    }

    #[test]
    fn equal_deadline_forecast_rows_merge_independent_of_input_order() {
        let first = ClaimBundle::new(
            0,
            vec![
                ForecastClaim {
                    through: 100,
                    amount: 20,
                },
                ForecastClaim {
                    through: 100,
                    amount: 30,
                },
            ],
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .expect("equal-deadline claims fit u32");
        let second = ClaimBundle::new(
            0,
            vec![
                ForecastClaim {
                    through: 100,
                    amount: 30,
                },
                ForecastClaim {
                    through: 100,
                    amount: 20,
                },
            ],
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .expect("reversed claims fit u32");

        assert_eq!(first, second);
        assert_eq!(
            first.forecast_scrap,
            &[ForecastClaim {
                through: 100,
                amount: 50,
            }]
        );
        assert_eq!(
            ClaimBundle::new(
                0,
                vec![
                    ForecastClaim {
                        through: 100,
                        amount: u32::MAX,
                    },
                    ForecastClaim {
                        through: 100,
                        amount: 1,
                    },
                ],
                vec![],
                vec![],
                vec![],
                vec![],
            ),
            Err(ClaimBundleError::ForecastScrapOverflow(100))
        );
    }

    #[test]
    fn row_major_keys_do_not_inherit_tile_positions_column_major_order() {
        let top_right = FoundryExpansionKey {
            anchor: TilePos::new(20, 1),
        };
        let bottom_left = FoundryExpansionKey {
            anchor: TilePos::new(1, 20),
        };
        assert!(top_right < bottom_left);
        assert!(top_right.anchor > bottom_left.anchor);
    }
}
