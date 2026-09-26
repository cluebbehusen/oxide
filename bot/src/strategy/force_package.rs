//! Pure opportunity sizing for connected reconnaissance, suppression, and strike operations.
//!
//! This module describes what an operation can usefully bring and what the
//! current fog-honest economy can conservatively finish by one fixed deadline.
//! It does not reserve units, spend scrap, emit commands, or freeze tactical
//! membership. The owning planner performs those mutations and freezes exact
//! members only when the operation commits to suppression.

use super::super::executive::weapon_burst_dps100;
use super::super::intelligence::{
    AirDefenseContact, AirDefenseSource, BuildingContact, ContactEvidence, StrategicIntelligence,
};
use super::super::observation::Observation;
use super::super::profile::ResolvedProfile;
use super::super::resources::{
    ProducerEgress, ProductionAccess, ResourceForecast, ResourceSnapshot,
    count_paid_queued_ready_with_access,
};
use crate::allocation::{AllocationCapacity, ConnectedOffenseKey, ProducerJobClaim};
#[cfg(test)]
use crate::observation::ObservationData;
use crate::planning::{PlanningWork, Progress};
use chassis::Tick;
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::TilePos;
use oxide_sim::ids::{PlayerId, UnitId};
use oxide_sim::stats::{BOMB_SALVO_SPACING, BuildingKind, Domain, Role, UnitKind, WeaponStats};
use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

/// One lowerable production family and the total number the operation wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ProviderDemand {
    pub(super) kind: UnitKind,
    pub(super) count: usize,
}

/// Strategic priority carried across package derivation and production
/// lowering.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub(super) enum ProviderPriority {
    Minimum,
    Marginal,
}

/// One ordered tranche of exact providers with the same strategic priority,
/// tactical family, and concrete kind.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub(super) struct ProviderDemandTranche {
    pub(super) priority: ProviderPriority,
    pub(super) family: ForceFamily,
    pub(super) kind: UnitKind,
    pub(super) count: usize,
}

/// Capability measured in thousandths of one basic provider in each family.
///
/// Suppression and strike use full-salvo damage per 100 ticks, so a multi-bomb
/// airframe is valued by the complete attack run rather than its per-bomb hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct NormalizedCapability {
    pub(super) recon: u64,
    pub(super) suppression: u64,
    pub(super) strike: u64,
}

/// Fixed timing and prior-forecast constraints for one package derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PreparationConstraints {
    /// Observation boundary at which an incomplete package expires.
    pub(super) deadline: Tick,
    /// Ticks between opportunities for this controller to issue commands.
    pub(super) decision_cadence: Tick,
    /// Earlier commitments that own forecast income not covered by the current
    /// bank passed in the resource snapshot.
    pub(super) protected_forecast_scrap: u32,
}

/// Queue, income, and producer-route evidence used by one package derivation.
#[derive(Debug, Clone, Copy)]
pub(super) struct ProductionEvidence<'a> {
    resources: &'a ResourceSnapshot,
    access: &'a ProductionAccess,
    planning: Option<&'a PlanningWork>,
}

impl<'a> ProductionEvidence<'a> {
    pub(super) const fn with_planning(
        resources: &'a ResourceSnapshot,
        access: &'a ProductionAccess,
        planning: Option<&'a PlanningWork>,
    ) -> Self {
        Self {
            resources,
            access,
            planning,
        }
    }
}

/// Primary objective and the canonical current cluster retained by connected
/// tactical admission.
#[derive(Debug, Clone, Copy)]
pub(super) struct ConnectedTargetEvidence<'a> {
    pub(super) primary: &'a BuildingContact,
    pub(super) cluster: &'a [&'a BuildingContact],
    /// Frozen identity of an admitted operation being revised. Its remembered
    /// members of positive confidence still count toward target durability
    /// and value; anti-air evidence remains current-only.
    pub(super) committed: Option<ConnectedOffenseKey>,
}

/// Why a current connected opportunity cannot admit its minimum package.
///
/// These reasons describe only admission of the common minimum repertoire.
/// Once that minimum is feasible, a later inability to fund or finish another
/// marginal provider simply ends opportunity scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForcePackageRejection {
    /// The shared work allowance has not yet produced a current production witness.
    Deferred,
    /// Current evidence does not identify a live, completed, valuable target.
    TargetNotActionable,
    /// Net bank and completed-source income cannot pay for the next provider
    /// by the fixed deadline.
    InsufficientResources {
        family: ForceFamily,
        required_scrap: u32,
        available_scrap: u32,
        deadline_shortfall: u32,
    },
    /// No completed, currently legal production lane can make this family.
    MissingCompletedProviderCapability { family: ForceFamily },
    /// A legal completed lane exists, but its queue, egress, funding cadence,
    /// or train time cannot expose the provider before the fixed deadline.
    PreparationWindowTooShort {
        family: ForceFamily,
        observed_at: Tick,
        deadline: Tick,
    },
    /// A currently observed air-domain anti-air source covers the target
    /// cluster. The connected package's ground-targeting suppression force
    /// cannot remove it.
    UntargetableCurrentAirDefense { firepower: u64, hit_points: u64 },
}

/// A connected operation sized against one current, targetable opportunity.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ConnectedForcePackage {
    /// Observation tick that supplied this package's evidence.
    pub(super) derived_at: Tick,
    /// Absolute tick by which every newly requested provider must be ready.
    pub(super) preparation_deadline: Tick,
    /// Canonical targets whose value and defenses sized this package. A
    /// revision sizes only members of the operation's committed cluster.
    pub(super) target_anchors: Vec<TilePos>,
    pub(super) recon: Vec<ProviderDemand>,
    pub(super) suppression: Vec<ProviderDemand>,
    pub(super) strike: Vec<ProviderDemand>,
    /// Every desired provider in derivation priority order. Existing and paid
    /// providers remain represented so lowering can consume them before
    /// scheduling the missing remainder.
    pub(super) provider_priority: Vec<ProviderDemandTranche>,
    /// New providers whose payment and production timing made this exact
    /// package feasible. Existing live and already-paid providers are omitted.
    pub(super) funded_providers: Vec<FundedProvider>,
    /// Personality-independent complete repertoire required for admission.
    pub(super) minimum_capability: NormalizedCapability,
    /// Opportunity-scaled ceiling. Production stops here even if scrap remains.
    pub(super) useful_capability: NormalizedCapability,
    /// Current-visible non-suppression collateral work that makes attack-run
    /// bombers useful in addition to their direct ground-strike contribution.
    pub(super) useful_bombing: u64,
    /// Current cluster durability weighted by strategic building value.
    pub(super) target_value: u64,
    /// Spendable bank observed when this package was derived.
    pub(super) current_scrap: u32,
    /// Deduplicated, currently observed anti-air damage per 100 ticks,
    /// including sources this package cannot suppress directly.
    pub(super) observed_aa_firepower: u64,
    /// The currently observed share of anti-air firepower attached to live
    /// ground-domain units or completed static targets that artillery can hit.
    pub(super) suppressible_aa_firepower: u64,
    /// Income from sources completed at the observation, through the deadline.
    pub(super) forecast_scrap: u32,
    /// Capability supplied by the chosen, possibly indivisible provider mix.
    pub(super) chosen_capability: NormalizedCapability,
    /// Target-specific bombing value supplied by the chosen strike providers.
    pub(super) chosen_bombing: u64,
}

const NORMALIZED_PROVIDER: u64 = 1_000;
/// Buildings within this local Manhattan radius form one tactical objective.
/// More distant value remains available to a later operation rather than
/// silently enlarging the current force package.
const TARGET_CLUSTER_RADIUS: u32 = 4;
/// Converts strategic target value into additional strike work alongside the
/// cluster's literal hit points. This is a scale divisor, not a value ceiling.
const TARGET_VALUE_STRIKE_DIVISOR: u64 = 10;
/// The existing connected strike phase's bounded window, expressed as 100-tick
/// damage periods. This converts observed durability into useful firepower;
/// it is not a roster ceiling.
const TACTICAL_EFFECT_WINDOW: Tick = 1_200;

type PackageCandidateScore = (u64, u128, Reverse<u32>, u64);

/// Capability family used by package-demand diagnostics.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForceFamily {
    /// Vision required to establish current target evidence.
    Recon,
    /// Ground firepower required to remove targetable anti-air defenses.
    Suppression,
    /// Air firepower required to destroy the target cluster.
    Strike,
}

impl ForceFamily {
    const ALL: [Self; 3] = [Self::Recon, Self::Suppression, Self::Strike];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddProviderFailure {
    InsufficientResources {
        required_scrap: u32,
        available_scrap: u32,
    },
    PreparationWindowTooShort,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub(super) struct FundedProvider {
    pub(super) kind: UnitKind,
    pub(super) command_tick: Tick,
}

#[derive(Debug, Clone, Copy)]
struct ProviderFundingEvidence<'a> {
    observed_at: Tick,
    current_scrap: u32,
    forecast: &'a ResourceForecast,
    constraints: PreparationConstraints,
}

impl ProviderFundingEvidence<'_> {
    fn available_scrap_at(self, tick: Tick) -> u32 {
        self.current_scrap.saturating_add(
            self.forecast
                .income_through(tick)
                .amount()
                .saturating_sub(self.constraints.protected_forecast_scrap),
        )
    }

    fn earliest_command_tick(self, committed_scrap: u32, cost: u32) -> Option<Tick> {
        let required = committed_scrap.checked_add(cost)?;
        if self.current_scrap >= required {
            return Some(self.observed_at);
        }
        if self.available_scrap_at(self.constraints.deadline) < required {
            return None;
        }

        let mut earliest = self.observed_at;
        let mut latest = self.constraints.deadline;
        while earliest < latest {
            let middle = earliest.saturating_add(latest.saturating_sub(earliest) / 2);
            if self.available_scrap_at(middle) >= required {
                latest = middle;
            } else {
                earliest = middle.saturating_add(1);
            }
        }
        next_decision_tick_after(earliest, self.constraints.decision_cadence)
    }
}

#[derive(Debug, Clone)]
#[cfg(test)]
struct FundedLane {
    eligible_kinds: Vec<UnitKind>,
    available_tick: Tick,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg(test)]
struct FundedLaneClass {
    eligible_kinds: Vec<UnitKind>,
    available_ticks: Vec<Tick>,
}

#[derive(Debug, Clone, Copy)]
struct PreservedProvider {
    family: ForceFamily,
    kind: UnitKind,
    remaining: usize,
}

#[derive(Debug)]
struct PackageRefinement<'a> {
    planning: &'a PlanningWork,
    capacity: &'a AllocationCapacity,
    key: ConnectedOffenseKey,
}

impl PackageRefinement<'_> {
    fn refine(
        &self,
        resources: &ResourceSnapshot,
        access: &ProductionAccess,
        providers: &[FundedProvider],
        deadline: Tick,
    ) -> Progress<()> {
        let mut eligible_by_kind = BTreeMap::new();
        let jobs = providers
            .iter()
            .map(|provider| {
                let eligible = eligible_by_kind.entry(provider.kind).or_insert_with(|| {
                    resources
                        .producers()
                        .iter()
                        .filter(|lane| {
                            access.allows(lane.producer, provider.kind)
                                && lane.horizon_timing(&[provider.kind]).is_some_and(|timing| {
                                    matches!(
                                        timing.current_egress,
                                        ProducerEgress::NotRequired | ProducerEgress::Open
                                    )
                                })
                        })
                        .map(|lane| lane.producer)
                        .collect::<Vec<_>>()
                });
                ProducerJobClaim::flexible(
                    provider.kind,
                    provider.command_tick,
                    deadline,
                    eligible.clone(),
                )
            })
            .collect();
        crate::allocation::forecast::refine(self.capacity, self.key, jobs, self.planning)
    }
}

#[derive(Debug, Clone)]
struct PackageBuilder<'a> {
    faction: oxide_sim::state::Faction,
    observed_at: Tick,
    deadline: Tick,
    decision_cadence: Tick,
    current_scrap: u32,
    protected_forecast_scrap: u32,
    forecast: &'a ResourceForecast,
    resources: &'a ResourceSnapshot,
    committed_scrap: u32,
    production_access: &'a ProductionAccess,
    refinement: Option<&'a PackageRefinement<'a>>,
    deferred: &'a Cell<bool>,
    pub(super) funded_providers: Vec<FundedProvider>,
    preserved: Vec<PreservedProvider>,
    provider_priority: Vec<ProviderDemandTranche>,
    recon: Vec<ProviderDemand>,
    suppression: Vec<ProviderDemand>,
    strike: Vec<ProviderDemand>,
    capability: NormalizedCapability,
    bombing_capability_per_provider: u64,
    bombing: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PackageSearchKey {
    capability: (u64, u64, u64, u64),
    committed_scrap: u32,
    preserved: Vec<(ForceFamily, UnitKind, usize)>,
    provider_priority: Vec<ProviderDemandTranche>,
    funded_providers: Vec<FundedProvider>,
}

#[cfg(test)]
fn funded_providers_fit(
    resources: &ResourceSnapshot,
    providers: &[FundedProvider],
    deadline: Tick,
    access: &ProductionAccess,
) -> bool {
    let mut kinds: Vec<_> = providers.iter().map(|provider| provider.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let lanes = funded_lane_evidence(resources, access, &kinds);
    funded_lane_schedule_fits(lanes, providers, deadline)
}

pub(super) fn refine_provider_demands(
    production: ProductionEvidence<'_>,
    demands: &[ProviderDemandTranche],
    observed_at: Tick,
    constraints: PreparationConstraints,
    key: ConnectedOffenseKey,
) -> Progress<()> {
    let Some(providers) =
        funded_demand_releases(production.resources, demands, observed_at, constraints)
    else {
        return Progress::ProvenInfeasible;
    };
    if let Some(planning) = production.planning {
        let Ok(capacity) = AllocationCapacity::from_snapshot(
            production.resources,
            constraints.deadline,
            constraints.decision_cadence,
        ) else {
            return Progress::ProvenInfeasible;
        };
        PackageRefinement {
            planning,
            capacity: &capacity,
            key,
        }
        .refine(
            production.resources,
            production.access,
            &providers,
            constraints.deadline,
        )
    } else {
        Progress::Deferred
    }
}

#[cfg(test)]
fn provider_demands_fit_funded_horizon(
    resources: &ResourceSnapshot,
    demands: &[ProviderDemandTranche],
    observed_at: Tick,
    constraints: PreparationConstraints,
    access: &ProductionAccess,
) -> bool {
    funded_demand_releases(resources, demands, observed_at, constraints).is_some_and(|providers| {
        funded_providers_fit(resources, &providers, constraints.deadline, access)
    })
}

fn funded_demand_releases(
    resources: &ResourceSnapshot,
    demands: &[ProviderDemandTranche],
    observed_at: Tick,
    constraints: PreparationConstraints,
) -> Option<Vec<FundedProvider>> {
    if constraints.decision_cadence == 0 || constraints.deadline < observed_at {
        return None;
    }
    let funding = ProviderFundingEvidence {
        observed_at,
        current_scrap: resources.current_scrap().amount(),
        forecast: resources.forecast(),
        constraints,
    };
    let mut committed_scrap = 0_u32;
    let mut providers = Vec::new();
    for demand in demands {
        for _ in 0..demand.count {
            let command_tick =
                funding.earliest_command_tick(committed_scrap, demand.kind.stats().cost)?;
            committed_scrap = committed_scrap.checked_add(demand.kind.stats().cost)?;
            providers.push(FundedProvider {
                kind: demand.kind,
                command_tick,
            });
        }
    }
    Some(providers)
}

#[cfg(test)]
fn funded_lane_schedule_fits(
    lanes: Vec<FundedLane>,
    providers: &[FundedProvider],
    deadline: Tick,
) -> bool {
    if providers.is_empty() {
        return true;
    }
    debug_assert!(
        providers
            .windows(2)
            .all(|pair| pair[0].command_tick <= pair[1].command_tick)
    );
    let classes = canonical_funded_lane_classes(lanes);
    FundedHorizonSearch {
        providers,
        deadline,
        failed: BTreeMap::new(),
    }
    .fits(0, classes)
}

#[cfg(test)]
fn funded_lane_evidence(
    resources: &ResourceSnapshot,
    access: &ProductionAccess,
    kinds: &[UnitKind],
) -> Vec<FundedLane> {
    resources
        .producers()
        .iter()
        .filter_map(|lane| {
            let mut available_tick = None;
            let eligible_kinds: Vec<_> = kinds
                .iter()
                .copied()
                .filter(|&kind| {
                    if !access.allows(lane.producer, kind) {
                        return false;
                    }
                    let Some(timing) = lane.horizon_timing(&[kind]) else {
                        return false;
                    };
                    if !matches!(
                        timing.current_egress,
                        ProducerEgress::NotRequired | ProducerEgress::Open
                    ) {
                        return false;
                    }
                    let Some(kind_available_tick) = timing
                        .no_block_latest_ready_tick
                        .checked_add(1)
                        .and_then(|tick| tick.checked_sub(Tick::from(kind.stats().train_ticks)))
                    else {
                        return false;
                    };
                    debug_assert!(
                        available_tick.is_none_or(|existing| existing == kind_available_tick)
                    );
                    available_tick = Some(kind_available_tick);
                    true
                })
                .collect();
            Some(FundedLane {
                eligible_kinds,
                available_tick: available_tick?,
            })
        })
        .collect()
}

#[cfg(test)]
fn canonical_funded_lane_classes(lanes: Vec<FundedLane>) -> Vec<FundedLaneClass> {
    let mut classes = BTreeMap::<Vec<UnitKind>, Vec<Tick>>::new();
    for lane in lanes {
        classes
            .entry(lane.eligible_kinds)
            .or_default()
            .push(lane.available_tick);
    }
    classes
        .into_iter()
        .map(|(eligible_kinds, mut available_ticks)| {
            available_ticks.sort_unstable();
            FundedLaneClass {
                eligible_kinds,
                available_ticks,
            }
        })
        .collect()
}

/// Exact fixed-order scheduling over canonical producer classes.
///
/// A provider's funding command is a release time. Within one lane, provider
/// order is the canonical funding order, so a state needs only each lane's
/// next available tick. Equal-eligibility lanes are interchangeable and a
/// state with every lane available no later dominates a later state.
#[cfg(test)]
struct FundedHorizonSearch<'a> {
    providers: &'a [FundedProvider],
    deadline: Tick,
    failed: BTreeMap<usize, Vec<Vec<FundedLaneClass>>>,
}

#[cfg(test)]
impl FundedHorizonSearch<'_> {
    fn fits(&mut self, provider_index: usize, lanes: Vec<FundedLaneClass>) -> bool {
        if provider_index == self.providers.len() {
            return true;
        }
        if self.is_dominated_failure(provider_index, &lanes) {
            return false;
        }
        if !remaining_funded_work_can_fit(&self.providers[provider_index..], &lanes, self.deadline)
        {
            self.record_failure(provider_index, lanes);
            return false;
        }

        let provider = self.providers[provider_index];
        let duration = Tick::from(provider.kind.stats().train_ticks);
        let mut candidates = Vec::new();
        for (class_index, class) in lanes.iter().enumerate() {
            if class.eligible_kinds.binary_search(&provider.kind).is_err() {
                continue;
            }
            let flexibility = class
                .eligible_kinds
                .iter()
                .filter(|kind| {
                    self.providers[provider_index + 1..]
                        .iter()
                        .any(|future| future.kind == **kind)
                })
                .count();
            for (lane_index, &available_tick) in class.available_ticks.iter().enumerate() {
                if lane_index > 0 && class.available_ticks[lane_index - 1] == available_tick {
                    continue;
                }
                let Some(next_available_tick) = available_tick
                    .max(provider.command_tick)
                    .checked_add(duration)
                else {
                    continue;
                };
                if next_available_tick > self.deadline {
                    continue;
                }
                candidates.push((
                    flexibility,
                    Reverse(next_available_tick),
                    class_index,
                    lane_index,
                    next_available_tick,
                ));
            }
        }
        candidates.sort_unstable();

        for (_, _, class_index, lane_index, next_available_tick) in candidates {
            let mut next_lanes = lanes.clone();
            next_lanes[class_index].available_ticks[lane_index] = next_available_tick;
            next_lanes[class_index].available_ticks.sort_unstable();
            if self.fits(provider_index + 1, next_lanes) {
                return true;
            }
        }

        self.record_failure(provider_index, lanes);
        false
    }

    fn is_dominated_failure(&self, provider_index: usize, lanes: &[FundedLaneClass]) -> bool {
        self.failed.get(&provider_index).is_some_and(|failed| {
            failed
                .iter()
                .any(|known| funded_lanes_dominate(known, lanes))
        })
    }

    fn record_failure(&mut self, provider_index: usize, lanes: Vec<FundedLaneClass>) {
        let failed = self.failed.entry(provider_index).or_default();
        failed.retain(|known| !funded_lanes_dominate(&lanes, known));
        failed.push(lanes);
    }
}

#[cfg(test)]
fn funded_lanes_dominate(left: &[FundedLaneClass], right: &[FundedLaneClass]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.eligible_kinds == right.eligible_kinds
                && left.available_ticks.len() == right.available_ticks.len()
                && left
                    .available_ticks
                    .iter()
                    .zip(&right.available_ticks)
                    .all(|(&left, &right)| left <= right)
        })
}

#[cfg(test)]
fn remaining_funded_work_can_fit(
    providers: &[FundedProvider],
    lanes: &[FundedLaneClass],
    deadline: Tick,
) -> bool {
    let Some(first) = providers.first() else {
        return true;
    };
    let mut counts = BTreeMap::<UnitKind, usize>::new();
    let requested_ticks = providers.iter().fold(0_u128, |total, provider| {
        *counts.entry(provider.kind).or_default() += 1;
        total.saturating_add(u128::from(provider.kind.stats().train_ticks))
    });
    let capacity_after = |release_tick: Tick| {
        lanes
            .iter()
            .flat_map(|class| &class.available_ticks)
            .map(|&available_tick| deadline.saturating_sub(available_tick.max(release_tick)))
            .map(u128::from)
            .sum::<u128>()
    };
    if requested_ticks > capacity_after(first.command_tick) {
        return false;
    }

    let modular_capacity = lanes
        .iter()
        .map(|class| {
            let divisor = class
                .eligible_kinds
                .iter()
                .filter(|kind| counts.contains_key(kind))
                .map(|kind| Tick::from(kind.stats().train_ticks))
                .reduce(greatest_common_divisor);
            class
                .available_ticks
                .iter()
                .map(|&available_tick| {
                    deadline.saturating_sub(available_tick.max(first.command_tick))
                })
                .map(|capacity| divisor.map_or(0, |divisor| capacity - capacity % divisor))
                .map(u128::from)
                .sum::<u128>()
        })
        .sum::<u128>();
    if requested_ticks > modular_capacity {
        return false;
    }

    if counts.iter().any(|(&kind, &count)| {
        let duration = Tick::from(kind.stats().train_ticks);
        let slots = lanes
            .iter()
            .filter(|class| class.eligible_kinds.binary_search(&kind).is_ok())
            .flat_map(|class| &class.available_ticks)
            .map(|&available_tick| {
                deadline.saturating_sub(available_tick.max(first.command_tick)) / duration
            })
            .map(u128::from)
            .sum::<u128>();
        count as u128 > slots
    }) {
        return false;
    }

    let mut suffix_ticks = requested_ticks;
    for (index, provider) in providers.iter().enumerate() {
        if (index == 0 || providers[index - 1].command_tick != provider.command_tick)
            && suffix_ticks > capacity_after(provider.command_tick)
        {
            return false;
        }
        suffix_ticks = suffix_ticks.saturating_sub(u128::from(provider.kind.stats().train_ticks));
    }
    true
}

#[cfg(test)]
fn greatest_common_divisor(mut left: Tick, mut right: Tick) -> Tick {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// Derives a side-effect-free package for a currently observed target.
///
/// Rejection reports why the fixed deadline, current completed production
/// base, current scrap plus conservative completed-source forecast after older
/// forecast promises, and available existing forces cannot field the common
/// minimum repertoire. Forecast-funded work starts only on a later real bot
/// decision cadence. The returned counts are revisable kind totals; the owning
/// planner schedules only currently open queue positions and freezes exact unit
/// ids when it commits to suppression.
#[cfg(test)]
pub(super) fn derive_connected_force_package(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    target: &BuildingContact,
    production: ProductionEvidence<'_>,
    unavailable: &[UnitId],
    constraints: PreparationConstraints,
) -> Result<ConnectedForcePackage, ForcePackageRejection> {
    let cluster = current_target_cluster(intelligence, target.player, target.anchor);
    derive_connected_force_package_for_cluster(
        profile,
        observation,
        intelligence,
        ConnectedTargetEvidence {
            primary: target,
            cluster: &cluster,
            committed: None,
        },
        production,
        unavailable,
        constraints,
    )
}

/// [`derive_connected_force_package`] against a caller-vetted current cluster.
///
/// Connected admission uses this boundary after proving that every retained
/// member is reachable by the operation's actual air and suppression tactics.
#[cfg(test)]
pub(super) fn derive_connected_force_package_for_cluster(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    targets: ConnectedTargetEvidence<'_>,
    production: ProductionEvidence<'_>,
    unavailable: &[UnitId],
    constraints: PreparationConstraints,
) -> Result<ConnectedForcePackage, ForcePackageRejection> {
    derive_connected_force_package_options_for_cluster(
        profile,
        observation,
        intelligence,
        targets,
        production,
        unavailable,
        constraints,
    )
    .map(ConnectedForcePackageOptions::into_largest)
}

/// Exact common minimum followed by deterministic, additions-only marginal
/// variants on the same evidence, target cluster, and preparation deadline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedForcePackageOptions {
    pub(super) refinement_pending: bool,
    pub(super) minimum: ConnectedForcePackage,
    pub(super) marginal: Vec<ConnectedForcePackage>,
}

impl ConnectedForcePackageOptions {
    #[cfg(test)]
    pub(super) fn into_largest(self) -> ConnectedForcePackage {
        self.marginal.into_iter().last().unwrap_or(self.minimum)
    }
}

/// Derives the independently admissible minimum and every useful marginal
/// extension without changing the minimum's target or production basis.
pub(super) fn derive_connected_force_package_options_for_cluster(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    targets: ConnectedTargetEvidence<'_>,
    production: ProductionEvidence<'_>,
    unavailable: &[UnitId],
    constraints: PreparationConstraints,
) -> Result<ConnectedForcePackageOptions, ForcePackageRejection> {
    derive_package_options::<false>(
        profile,
        observation,
        intelligence,
        targets,
        production,
        unavailable,
        constraints,
    )
}

pub(super) fn derive_connected_minimum_for_cluster(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    targets: ConnectedTargetEvidence<'_>,
    production: ProductionEvidence<'_>,
    unavailable: &[UnitId],
    constraints: PreparationConstraints,
) -> Result<ConnectedForcePackageOptions, ForcePackageRejection> {
    derive_package_options::<true>(
        profile,
        observation,
        intelligence,
        targets,
        production,
        unavailable,
        constraints,
    )
}

fn derive_package_options<const MINIMUM_ONLY: bool>(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    targets: ConnectedTargetEvidence<'_>,
    production: ProductionEvidence<'_>,
    unavailable: &[UnitId],
    constraints: PreparationConstraints,
) -> Result<ConnectedForcePackageOptions, ForcePackageRejection> {
    let deferred = Cell::new(false);
    let result = derive_package_options_inner::<MINIMUM_ONLY>(
        profile,
        observation,
        intelligence,
        targets,
        production,
        unavailable,
        constraints,
        &deferred,
    );
    if result.is_err() && deferred.get() {
        Err(ForcePackageRejection::Deferred)
    } else {
        result
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one derivation shares a deferred verdict across composition alternatives"
)]
fn derive_package_options_inner<const MINIMUM_ONLY: bool>(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    targets: ConnectedTargetEvidence<'_>,
    production: ProductionEvidence<'_>,
    unavailable: &[UnitId],
    constraints: PreparationConstraints,
    deferred: &Cell<bool>,
) -> Result<ConnectedForcePackageOptions, ForcePackageRejection> {
    let ConnectedTargetEvidence {
        primary: target,
        cluster,
        committed,
    } = targets;
    let ProductionEvidence {
        resources,
        access: production_access,
        planning,
    } = production;
    let PreparationConstraints {
        deadline: preparation_deadline,
        decision_cadence,
        protected_forecast_scrap,
    } = constraints;
    if target.id.is_none()
        || !target.built
        || target.hp == 0
        || building_value(target.kind) == 0
        || !intelligence
            .buildings()
            .iter()
            .any(|contact| contact == target)
    {
        return Err(ForcePackageRejection::TargetNotActionable);
    }

    let mut cluster: Vec<_> = cluster
        .iter()
        .copied()
        .filter(|contact| {
            contact.player == target.player
                && (contact.evidence == ContactEvidence::Current
                    || (committed.is_some() && contact.confidence_at(observation.tick) > 0))
                && contact.built
                && contact.hp > 0
                && building_value(contact.kind) > 0
                && intelligence
                    .buildings()
                    .iter()
                    .any(|known| known == *contact)
        })
        .collect();
    cluster.sort_unstable_by_key(|contact| (contact.anchor.y, contact.anchor.x, contact.id));
    cluster.dedup_by_key(|contact| (contact.anchor, contact.id));
    if cluster.is_empty()
        || !cluster
            .iter()
            .any(|contact| contact.id == target.id && contact.anchor == target.anchor)
    {
        return Err(ForcePackageRejection::TargetNotActionable);
    }
    let cluster_hp = cluster.iter().fold(0_u64, |total, contact| {
        total.saturating_add(u64::from(contact.hp))
    });
    let target_value = cluster.iter().fold(0_u64, |total, contact| {
        total.saturating_add(
            u64::from(contact.hp).saturating_mul(u64::from(building_value(contact.kind))),
        )
    });
    let air_defense = current_air_defense(intelligence, &cluster);
    if air_defense.untargetable_air_firepower > 0 {
        return Err(ForcePackageRejection::UntargetableCurrentAirDefense {
            firepower: air_defense.untargetable_air_firepower,
            hit_points: air_defense.untargetable_air_hp,
        });
    }

    let air_ground = Role::AirGround.unit_for(observation.faction);
    let bomber = Role::Bomber.unit_for(observation.faction);
    let minimum_capability = NormalizedCapability {
        recon: NORMALIZED_PROVIDER,
        suppression: NORMALIZED_PROVIDER,
        strike: NORMALIZED_PROVIDER,
    };
    let effect_periods = TACTICAL_EFFECT_WINDOW / 100;
    let strike_work = cluster_hp.saturating_add(target_value / TARGET_VALUE_STRIKE_DIVISOR);
    let suppression_work = air_defense.suppressible_hp.saturating_add(
        air_defense
            .suppressible_firepower
            .saturating_mul(effect_periods),
    );
    let useful_capability = NormalizedCapability {
        recon: minimum_capability.recon,
        suppression: minimum_capability.suppression.max(normalized_work(
            suppression_work,
            ground_firepower(UnitKind::Bombard),
            effect_periods,
        )),
        strike: minimum_capability.strike.max(normalized_work(
            strike_work,
            ground_firepower(air_ground),
            effect_periods,
        )),
    };
    let bombing = current_bombing_opportunity(observation, intelligence, &cluster, bomber);

    let forecast_scrap = resources
        .forecast()
        .income_through(preparation_deadline)
        .amount();
    let capacity = planning.and_then(|_| {
        AllocationCapacity::from_snapshot(resources, preparation_deadline, decision_cadence).ok()
    });
    let refinement =
        planning
            .zip(capacity.as_ref())
            .map(|(planning, capacity)| PackageRefinement {
                planning,
                capacity,
                key: committed.unwrap_or(ConnectedOffenseKey {
                    objective: target.id.expect("current target checked"),
                    anchor: target.anchor,
                }),
            });
    let mut builder = PackageBuilder {
        faction: observation.faction,
        observed_at: observation.tick,
        deadline: preparation_deadline,
        decision_cadence,
        current_scrap: resources.current_scrap().amount(),
        protected_forecast_scrap,
        forecast: resources.forecast(),
        resources,
        committed_scrap: 0,
        production_access,
        refinement: refinement.as_ref(),
        deferred,
        funded_providers: Vec::new(),
        preserved: Vec::new(),
        provider_priority: Vec::new(),
        recon: Vec::new(),
        suppression: Vec::new(),
        strike: Vec::new(),
        capability: NormalizedCapability {
            recon: 0,
            suppression: 0,
            strike: 0,
        },
        bombing_capability_per_provider: bombing.per_provider,
        bombing: 0,
    };

    let mut unavailable = unavailable.to_vec();
    unavailable.sort_unstable();
    unavailable.dedup();
    for family in ForceFamily::ALL {
        for kind in preservation_order(family, observation.faction) {
            let live = observation
                .my_units
                .iter()
                .filter(|unit| {
                    unit.player == observation.me
                        && unit.hp > 0
                        && unit.kind == kind
                        && unavailable.binary_search(&unit.id).is_err()
                })
                .count();
            let paid = count_paid_queued_ready_with_access(
                resources,
                kind,
                preparation_deadline,
                production_access,
            );
            builder.retain_preserved(family, kind, live.saturating_add(paid));
        }
    }

    let template = builder;
    let mut minimum_candidates =
        minimum_package_candidates(profile, template.clone(), minimum_capability);
    if minimum_candidates.is_empty() {
        minimum_candidates.push(construct_minimum(
            template,
            observation,
            resources,
            minimum_capability,
        )?);
    }
    minimum_candidates.sort_by_key(|candidate| {
        Reverse(package_candidate_score(
            profile,
            minimum_capability,
            0,
            candidate,
        ))
    });
    minimum_candidates.retain(|candidate| candidate.refine_providers(&candidate.funded_providers));
    if minimum_candidates.is_empty() {
        return Err(ForcePackageRejection::PreparationWindowTooShort {
            family: ForceFamily::Strike,
            observed_at: observation.tick,
            deadline: preparation_deadline,
        });
    }
    let builders = if MINIMUM_ONLY {
        let mut candidates = minimum_candidates;
        candidates.sort_by_key(|candidate| {
            Reverse(package_candidate_score(
                profile,
                minimum_capability,
                0,
                candidate,
            ))
        });
        vec![candidates.remove(0)]
    } else {
        best_complete_portfolio_path(
            profile,
            useful_capability,
            bombing.useful,
            minimum_candidates,
        )
    };
    let mut packages = builders.into_iter().map(|mut builder| {
        builder.canonicalize_provider_priority(profile);
        builder.sort_demands();
        let chosen_capability = builder.capability;
        ConnectedForcePackage {
            derived_at: observation.tick,
            preparation_deadline,
            target_anchors: cluster.iter().map(|contact| contact.anchor).collect(),
            recon: builder.recon,
            suppression: builder.suppression,
            strike: builder.strike,
            provider_priority: builder.provider_priority,
            funded_providers: builder.funded_providers,
            minimum_capability,
            useful_capability,
            useful_bombing: bombing.useful,
            target_value,
            current_scrap: resources.current_scrap().amount(),
            observed_aa_firepower: air_defense.total_firepower,
            suppressible_aa_firepower: air_defense.suppressible_firepower,
            forecast_scrap,
            chosen_capability,
            chosen_bombing: builder.bombing,
        }
    });
    let minimum = packages
        .next()
        .expect("the common minimum produced one complete package");
    Ok(ConnectedForcePackageOptions {
        refinement_pending: deferred.get(),
        minimum,
        marginal: packages.collect(),
    })
}

fn minimum_package_candidates<'a>(
    profile: &ResolvedProfile,
    template: PackageBuilder<'a>,
    minimum: NormalizedCapability,
) -> Vec<PackageBuilder<'a>> {
    let mut candidates = vec![template];
    for family in ForceFamily::ALL {
        while candidates
            .iter()
            .any(|candidate| candidate.capability_for(family) < minimum.for_family(family))
        {
            let mut next = Vec::new();
            for candidate in candidates {
                if candidate.capability_for(family) >= minimum.for_family(family) {
                    next.push(candidate);
                } else {
                    next.extend(
                        candidate
                            .provider_successors(family, ProviderPriority::Minimum)
                            .into_iter()
                            .filter(|successor| successor.priority_is_canonical(profile)),
                    );
                }
            }
            candidates = unique_search_states(next);
            candidates.sort_by_key(|candidate| {
                Reverse(package_candidate_score(profile, minimum, 0, candidate))
            });
            if candidates.len() > COMPOSITION_BEAM_WIDTH {
                let economical = candidates
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, candidate)| candidate.committed_scrap)
                    .map(|(index, _)| index)
                    .unwrap();
                let economical = candidates.remove(economical);
                candidates.truncate(COMPOSITION_BEAM_WIDTH - 1);
                candidates.push(economical);
            }
            if candidates.is_empty() {
                return candidates;
            }
        }
    }
    candidates
}

fn construct_minimum<'a>(
    mut builder: PackageBuilder<'a>,
    observation: &Observation,
    resources: &ResourceSnapshot,
    minimum: NormalizedCapability,
) -> Result<PackageBuilder<'a>, ForcePackageRejection> {
    for family in ForceFamily::ALL {
        while builder.capability_for(family) < minimum.for_family(family) {
            if builder.add_preserved(family, ProviderPriority::Minimum) {
                continue;
            }
            if !has_completed_provider_capability(
                observation,
                resources,
                builder.production_access,
                family,
            ) {
                return Err(ForcePackageRejection::MissingCompletedProviderCapability { family });
            }
            if let Err(failure) = builder.add_first_new_provider(
                family,
                ProviderPriority::Minimum,
                &preservation_order(family, builder.faction),
            ) {
                return Err(match failure {
                    AddProviderFailure::InsufficientResources {
                        required_scrap,
                        available_scrap,
                    } => ForcePackageRejection::InsufficientResources {
                        family,
                        required_scrap,
                        available_scrap,
                        deadline_shortfall: required_scrap.saturating_sub(available_scrap),
                    },
                    AddProviderFailure::PreparationWindowTooShort => {
                        ForcePackageRejection::PreparationWindowTooShort {
                            family,
                            observed_at: builder.observed_at,
                            deadline: builder.deadline,
                        }
                    }
                });
            }
        }
    }
    Ok(builder)
}

const COMPOSITION_BEAM_WIDTH: usize = 8;

fn best_complete_portfolio_path<'a>(
    profile: &ResolvedProfile,
    useful: NormalizedCapability,
    useful_bombing: u64,
    minimum_candidates: Vec<PackageBuilder<'a>>,
) -> Vec<PackageBuilder<'a>> {
    let score = |candidate: &PackageBuilder<'_>| {
        package_candidate_score(profile, useful, useful_bombing, candidate)
    };
    let minimums = unique_search_states(minimum_candidates);
    let mut frontier: Vec<_> = minimums.iter().cloned().enumerate().collect();
    let rank = |paths: &mut Vec<(usize, PackageBuilder<'a>)>| {
        // Stable ties preserve the canonical provider discovery order.
        paths.sort_by_key(|path| Reverse(score(&path.1)));
        if let Some(index) = paths
            .iter()
            .enumerate()
            .min_by_key(|(_, path)| {
                let candidate = &path.1;
                (candidate.committed_scrap, Reverse(score(candidate)))
            })
            .map(|(index, _)| index)
        {
            let economical = paths.remove(index);
            paths.truncate(COMPOSITION_BEAM_WIDTH - 1);
            paths.push(economical);
            paths.sort_by_key(|path| Reverse(score(&path.1)));
        }
    };
    rank(&mut frontier);
    let mut candidates = Vec::new();
    while !frontier.is_empty() {
        let mut next = Vec::new();
        let mut seen = BTreeSet::new();
        for (minimum_index, candidate) in frontier {
            for family in [ForceFamily::Suppression, ForceFamily::Strike] {
                if candidate.capability_for(family) >= useful.for_family(family)
                    && (family != ForceFamily::Strike || candidate.bombing >= useful_bombing)
                {
                    continue;
                }
                for mut successor in
                    candidate.provider_successors(family, ProviderPriority::Marginal)
                {
                    if !successor.canonicalize_funding(profile) {
                        continue;
                    }
                    let advances_family = successor
                        .capability_for(family)
                        .min(useful.for_family(family))
                        > candidate
                            .capability_for(family)
                            .min(useful.for_family(family));
                    let advances_bombing = family == ForceFamily::Strike
                        && successor.bombing.min(useful_bombing)
                            > candidate.bombing.min(useful_bombing);
                    if (advances_family || advances_bombing) && seen.insert(successor.search_key())
                    {
                        next.push((minimum_index, successor));
                    }
                }
            }
            candidates.push((minimum_index, candidate));
        }
        rank(&mut next);
        frontier = next;
    }
    candidates.sort_by_key(|(_, candidate)| Reverse(score(candidate)));
    let mut best = (0, minimums[0].clone());
    if let Some(refinement) = minimums[0].refinement {
        match refinement.planning.portfolio_candidate(
            minimums[0].observed_at,
            refinement.key,
            candidates.len(),
            |index| {
                let (minimum, candidate) = &candidates[index];
                match candidate.provider_refinement(&candidate.funded_providers) {
                    Progress::Ready(()) => Progress::Ready((*minimum, candidate.clone())),
                    Progress::Deferred => Progress::Deferred,
                    Progress::Exhausted => Progress::Exhausted,
                    Progress::ProvenInfeasible => Progress::ProvenInfeasible,
                }
            },
        ) {
            Progress::Ready(candidate) => best = candidate,
            Progress::Deferred | Progress::Exhausted => minimums[0].deferred.set(true),
            Progress::ProvenInfeasible => {}
        }
    } else {
        for (minimum, candidate) in candidates {
            match candidate.provider_refinement(&candidate.funded_providers) {
                Progress::Ready(()) => {
                    best = (minimum, candidate);
                    break;
                }
                Progress::ProvenInfeasible => {}
                Progress::Deferred | Progress::Exhausted => {
                    candidate.deferred.set(true);
                    break;
                }
            }
        }
    }
    canonical_growth_path(profile, minimums[best.0].clone(), best.1)
}

fn canonical_growth_path<'a>(
    profile: &ResolvedProfile,
    mut current: PackageBuilder<'a>,
    target: PackageBuilder<'a>,
) -> Vec<PackageBuilder<'a>> {
    // Discovery order may differ from the final funding order. Every offered
    // prefix must retain job ordinals and payment times when a variant grows.
    debug_assert!(
        target
            .funded_providers
            .starts_with(&current.funded_providers)
    );
    let mut result = vec![current.clone()];
    for tranche in &target.provider_priority {
        let minimum = current
            .provider_priority
            .iter()
            .find(|prior| {
                (prior.priority, prior.family, prior.kind)
                    == (tranche.priority, tranche.family, tranche.kind)
            })
            .map_or(0, |prior| prior.count);
        for _ in minimum..tranche.count {
            let preserved = current.preserved.iter().zip(&target.preserved).position(
                |(prior, final_provider)| {
                    prior.family == tranche.family
                        && prior.kind == tranche.kind
                        && prior.remaining > final_provider.remaining
                },
            );
            if let Some(index) = preserved {
                current.preserved[index].remaining -= 1;
            } else {
                let funded = target.funded_providers[current.funded_providers.len()];
                assert_eq!(funded.kind, tranche.kind);
                current.funded_providers.push(funded);
                current.committed_scrap += tranche.kind.stats().cost;
            }
            current.accept_provider(tranche.family, tranche.kind, tranche.priority);
            current.canonicalize_provider_priority(profile);
            result.push(current.clone());
        }
    }
    debug_assert_eq!(current.search_key(), target.search_key());
    result
}

fn unique_search_states<'a>(candidates: Vec<PackageBuilder<'a>>) -> Vec<PackageBuilder<'a>> {
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.search_key()))
        .collect()
}

fn package_candidate_score(
    profile: &ResolvedProfile,
    useful: NormalizedCapability,
    useful_bombing: u64,
    candidate: &PackageBuilder<'_>,
) -> PackageCandidateScore {
    let (total_useful, personality_value) = capped_useful_objective(
        profile,
        useful,
        useful_bombing,
        candidate.capability,
        candidate.bombing,
    );
    let total_capability = candidate
        .capability
        .recon
        .saturating_add(candidate.capability.suppression)
        .saturating_add(candidate.capability.strike)
        .saturating_add(candidate.bombing);
    (
        total_useful,
        personality_value,
        Reverse(candidate.committed_scrap),
        total_capability,
    )
}

fn capped_useful_objective(
    profile: &ResolvedProfile,
    useful: NormalizedCapability,
    useful_bombing: u64,
    chosen: NormalizedCapability,
    chosen_bombing: u64,
) -> (u64, u128) {
    let capped_recon = chosen.recon.min(useful.recon);
    let capped_suppression = chosen.suppression.min(useful.suppression);
    let capped_strike = chosen.strike.min(useful.strike);
    let capped_bombing = chosen_bombing.min(useful_bombing);
    let personality_value = u128::from(capped_suppression)
        .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.siege)))
        .saturating_add(
            u128::from(capped_strike)
                .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.air))),
        )
        .saturating_add(u128::from(capped_bombing).saturating_mul(u128::from(
            50_u16 + u16::from(profile.traits.air).saturating_mul(2),
        )));
    let total_useful = capped_recon
        .saturating_add(capped_suppression)
        .saturating_add(capped_strike)
        .saturating_add(capped_bombing);
    (total_useful, personality_value)
}

impl NormalizedCapability {
    fn for_family(self, family: ForceFamily) -> u64 {
        match family {
            ForceFamily::Recon => self.recon,
            ForceFamily::Suppression => self.suppression,
            ForceFamily::Strike => self.strike,
        }
    }
}

impl PackageBuilder<'_> {
    fn priority_is_canonical(&self, profile: &ResolvedProfile) -> bool {
        self.provider_priority.windows(2).all(|pair| {
            provider_priority_rank(profile, self.faction, pair[0])
                <= provider_priority_rank(profile, self.faction, pair[1])
        })
    }

    fn capability_for(&self, family: ForceFamily) -> u64 {
        self.capability.for_family(family)
    }

    fn add_preserved(&mut self, family: ForceFamily, priority: ProviderPriority) -> bool {
        let Some(provider) = self
            .preserved
            .iter_mut()
            .find(|provider| provider.family == family && provider.remaining > 0)
        else {
            return false;
        };
        let kind = provider.kind;
        provider.remaining -= 1;
        self.accept_provider(family, kind, priority);
        true
    }

    fn provider_successors(&self, family: ForceFamily, priority: ProviderPriority) -> Vec<Self> {
        let mut successors = Vec::new();
        for (index, provider) in self.preserved.iter().enumerate() {
            if provider.family != family || provider.remaining == 0 {
                continue;
            }
            let mut successor = self.clone();
            successor.preserved[index].remaining -= 1;
            successor.accept_provider(family, provider.kind, priority);
            successors.push(successor);
        }
        if priority == ProviderPriority::Minimum && !successors.is_empty() {
            return unique_search_states(successors);
        }
        let new_kinds = match priority {
            ProviderPriority::Minimum => preservation_order(family, self.faction),
            ProviderPriority::Marginal => new_provider_order(family, self.faction),
        };
        for kind in new_kinds {
            if let Ok(mut additions) = self.add_new_kind_variants(family, priority, kind) {
                successors.append(&mut additions);
            }
        }
        unique_search_states(successors)
    }

    fn add_first_new_provider(
        &mut self,
        family: ForceFamily,
        priority: ProviderPriority,
        kinds: &[UnitKind],
    ) -> Result<(), AddProviderFailure> {
        let mut resource_limit: Option<(u32, u32)> = None;
        for &kind in kinds {
            match self.add_new_kind_variants(family, priority, kind) {
                Ok(mut candidates) => {
                    *self = candidates.remove(0);
                    return Ok(());
                }
                Err(AddProviderFailure::InsufficientResources {
                    required_scrap,
                    available_scrap,
                }) => {
                    if resource_limit.is_none_or(|(current_required, current_available)| {
                        required_scrap.saturating_sub(available_scrap)
                            < current_required.saturating_sub(current_available)
                    }) {
                        resource_limit = Some((required_scrap, available_scrap));
                    }
                }
                Err(AddProviderFailure::PreparationWindowTooShort) => {}
            }
        }
        if let Some((required_scrap, available_scrap)) = resource_limit {
            Err(AddProviderFailure::InsufficientResources {
                required_scrap,
                available_scrap,
            })
        } else {
            Err(AddProviderFailure::PreparationWindowTooShort)
        }
    }

    fn providers_fit(&self, providers: &[FundedProvider]) -> bool {
        if providers.iter().any(|provider| {
            provider
                .command_tick
                .checked_add(Tick::from(provider.kind.stats().train_ticks))
                .is_none_or(|ready_at| ready_at > self.deadline)
        }) {
            return false;
        }
        if self.refinement.is_some() {
            crate::resources::production_may_fit_horizon(
                self.resources,
                &providers
                    .iter()
                    .map(|provider| provider.kind)
                    .collect::<Vec<_>>(),
                self.deadline,
                self.production_access,
            )
        } else {
            self.refine_providers(providers)
        }
    }

    fn refine_providers(&self, providers: &[FundedProvider]) -> bool {
        match self.provider_refinement(providers) {
            Progress::Ready(()) => true,
            Progress::ProvenInfeasible => false,
            Progress::Deferred | Progress::Exhausted => {
                self.deferred.set(true);
                false
            }
        }
    }

    fn provider_refinement(&self, providers: &[FundedProvider]) -> Progress<()> {
        if providers.is_empty() {
            return Progress::Ready(());
        }
        if let Some(refinement) = self.refinement {
            refinement.refine(
                self.resources,
                self.production_access,
                providers,
                self.deadline,
            )
        } else {
            Progress::Deferred
        }
    }

    fn add_new_kind_variants(
        &self,
        family: ForceFamily,
        priority: ProviderPriority,
        kind: UnitKind,
    ) -> Result<Vec<Self>, AddProviderFailure> {
        let cost = kind.stats().cost;
        let kinds: Vec<_> = self
            .funded_providers
            .iter()
            .map(|provider| provider.kind)
            .chain(std::iter::once(kind))
            .collect();
        if !crate::resources::production_may_fit_horizon(
            self.resources,
            &kinds,
            self.deadline,
            self.production_access,
        ) {
            return Err(AddProviderFailure::PreparationWindowTooShort);
        }
        let required_scrap = self.committed_scrap.saturating_add(cost);
        let available_scrap = self.available_scrap_at(self.deadline);
        if available_scrap < required_scrap {
            return Err(AddProviderFailure::InsufficientResources {
                required_scrap,
                available_scrap,
            });
        }
        let Some(command_tick) = self.earliest_funded_command_tick(cost) else {
            return Err(AddProviderFailure::PreparationWindowTooShort);
        };
        let mut successor = self.clone();
        successor
            .funded_providers
            .push(FundedProvider { kind, command_tick });
        successor.committed_scrap = successor.committed_scrap.saturating_add(cost);
        successor.accept_provider(family, kind, priority);
        // Marginal funding is reordered before its payment deadlines are checked.
        if priority == ProviderPriority::Minimum
            && !successor.providers_fit(&successor.funded_providers)
        {
            return Err(AddProviderFailure::PreparationWindowTooShort);
        }
        Ok(vec![successor])
    }

    fn search_key(&self) -> PackageSearchKey {
        PackageSearchKey {
            capability: (
                self.capability.recon,
                self.capability.suppression,
                self.capability.strike,
                self.bombing,
            ),
            committed_scrap: self.committed_scrap,
            preserved: self
                .preserved
                .iter()
                .map(|provider| (provider.family, provider.kind, provider.remaining))
                .collect(),
            provider_priority: self.provider_priority.clone(),
            funded_providers: self.funded_providers.clone(),
        }
    }

    fn canonicalize_provider_priority(&mut self, profile: &ResolvedProfile) {
        let mut canonical = Vec::<ProviderDemandTranche>::new();
        for tranche in self.provider_priority.drain(..) {
            if let Some(existing) = canonical.iter_mut().find(|existing| {
                existing.priority == tranche.priority
                    && existing.family == tranche.family
                    && existing.kind == tranche.kind
            }) {
                existing.count = existing.count.saturating_add(tranche.count);
            } else {
                canonical.push(tranche);
            }
        }
        canonical.sort_unstable_by_key(|tranche| {
            provider_priority_rank(profile, self.faction, *tranche)
        });
        self.provider_priority = canonical;
    }

    fn canonicalize_funding(&mut self, profile: &ResolvedProfile) -> bool {
        self.canonicalize_provider_priority(profile);
        let mut free = BTreeMap::<UnitKind, usize>::new();
        for tranche in &self.provider_priority {
            *free.entry(tranche.kind).or_default() += tranche.count;
        }
        for funded in &self.funded_providers {
            *free
                .get_mut(&funded.kind)
                .expect("a funded provider is demanded") -= 1;
        }
        let funding = self.funding_evidence();
        let mut committed = 0_u32;
        let mut providers = Vec::with_capacity(self.funded_providers.len());
        for tranche in &self.provider_priority {
            for _ in 0..tranche.count {
                let remaining = free
                    .get_mut(&tranche.kind)
                    .expect("a demanded kind was counted");
                if *remaining > 0 {
                    *remaining -= 1;
                    continue;
                }
                let cost = tranche.kind.stats().cost;
                let Some(command_tick) = funding.earliest_command_tick(committed, cost) else {
                    return false;
                };
                providers.push(FundedProvider {
                    kind: tranche.kind,
                    command_tick,
                });
                committed = committed.saturating_add(cost);
            }
        }
        if !self.providers_fit(&providers) {
            return false;
        }
        self.funded_providers = providers;
        true
    }

    fn retain_preserved(&mut self, family: ForceFamily, kind: UnitKind, count: usize) {
        if count > 0 {
            self.preserved.push(PreservedProvider {
                family,
                kind,
                remaining: count,
            });
        }
    }

    fn accept_provider(&mut self, family: ForceFamily, kind: UnitKind, priority: ProviderPriority) {
        self.add_demand(family, kind);
        self.add_capability(family, provider_capability(family, kind, self.faction));
        if family == ForceFamily::Strike && kind == Role::Bomber.unit_for(self.faction) {
            self.bombing = self
                .bombing
                .saturating_add(self.bombing_capability_per_provider);
        }
        if let Some(tranche) = self.provider_priority.last_mut()
            && tranche.priority == priority
            && tranche.family == family
            && tranche.kind == kind
        {
            tranche.count = tranche.count.saturating_add(1);
        } else {
            self.provider_priority.push(ProviderDemandTranche {
                priority,
                family,
                kind,
                count: 1,
            });
        }
    }

    fn earliest_funded_command_tick(&self, cost: u32) -> Option<Tick> {
        self.funding_evidence()
            .earliest_command_tick(self.committed_scrap, cost)
    }

    fn available_scrap_at(&self, tick: Tick) -> u32 {
        self.funding_evidence().available_scrap_at(tick)
    }

    fn funding_evidence(&self) -> ProviderFundingEvidence<'_> {
        ProviderFundingEvidence {
            observed_at: self.observed_at,
            current_scrap: self.current_scrap,
            forecast: self.forecast,
            constraints: PreparationConstraints {
                deadline: self.deadline,
                decision_cadence: self.decision_cadence,
                protected_forecast_scrap: self.protected_forecast_scrap,
            },
        }
    }

    fn add_demand(&mut self, family: ForceFamily, kind: UnitKind) {
        let demands = match family {
            ForceFamily::Recon => &mut self.recon,
            ForceFamily::Suppression => &mut self.suppression,
            ForceFamily::Strike => &mut self.strike,
        };
        if let Some(demand) = demands.iter_mut().find(|demand| demand.kind == kind) {
            demand.count = demand.count.saturating_add(1);
        } else {
            demands.push(ProviderDemand { kind, count: 1 });
        }
    }

    fn add_capability(&mut self, family: ForceFamily, contribution: u64) {
        let capability = match family {
            ForceFamily::Recon => &mut self.capability.recon,
            ForceFamily::Suppression => &mut self.capability.suppression,
            ForceFamily::Strike => &mut self.capability.strike,
        };
        *capability = capability.saturating_add(contribution);
    }

    fn sort_demands(&mut self) {
        self.recon.sort_unstable_by_key(|demand| {
            provider_rank(ForceFamily::Recon, self.faction, demand.kind)
        });
        self.suppression.sort_unstable_by_key(|demand| {
            provider_rank(ForceFamily::Suppression, self.faction, demand.kind)
        });
        self.strike.sort_unstable_by_key(|demand| {
            provider_rank(ForceFamily::Strike, self.faction, demand.kind)
        });
    }
}

fn has_completed_provider_capability(
    observation: &Observation,
    resources: &ResourceSnapshot,
    production_access: &ProductionAccess,
    family: ForceFamily,
) -> bool {
    let completed = |kind: BuildingKind| {
        observation.my_buildings.iter().any(|building| {
            building.player == observation.me
                && building.kind == kind
                && building.built
                && building.hp > 0
        })
    };
    new_provider_order(family, observation.faction)
        .into_iter()
        .any(|unit_kind| {
            unit_kind
                .faction()
                .is_none_or(|faction| faction == observation.faction)
                && unit_kind.stats().requires.iter().copied().all(completed)
                && resources.producers().iter().any(|lane| {
                    production_access.allows(lane.producer, unit_kind)
                        && observation.my_buildings.iter().any(|building| {
                            building.id == lane.producer
                                && building
                                    .kind
                                    .tier_stats(building.tier)
                                    .produces
                                    .contains(&unit_kind)
                        })
                })
        })
}

fn next_decision_tick_after(tick: Tick, cadence: Tick) -> Option<Tick> {
    let remainder = tick.checked_rem(cadence)?;
    tick.checked_add(cadence.checked_sub(remainder)?)
}

pub(super) fn current_target_cluster(
    intelligence: &StrategicIntelligence,
    target_player: PlayerId,
    original_anchor: TilePos,
) -> Vec<&BuildingContact> {
    intelligence
        .buildings()
        .iter()
        .filter(|contact| {
            contact.player == target_player
                && contact.evidence == ContactEvidence::Current
                && contact.built
                && contact.hp > 0
                && building_value(contact.kind) > 0
                && within_target_cluster(original_anchor, contact.anchor)
        })
        .collect()
}

/// Whether `anchor` can belong to the tactical cluster around `original`.
pub(super) fn within_target_cluster(original: TilePos, anchor: TilePos) -> bool {
    manhattan(anchor, original) <= TARGET_CLUSTER_RADIUS
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BombingOpportunity {
    useful: u64,
    per_provider: u64,
}

#[derive(Debug, Clone, Copy)]
struct BombingVictim {
    position: Vec2Fx,
    hit_points: u32,
}

/// Values only collateral that the authoritative bomb impacts can currently
/// reach. The selected building's direct damage remains part of Strike, while
/// this optional dimension captures why a committed attack run can be better
/// than another light strafing aircraft against a dense target. Operational
/// mobile anti-air already included in the package's mandatory suppression work
/// is excluded here. Other air-defense exposure is not charged here: connected
/// packages reject admission when current air-only defense cannot be removed.
fn current_bombing_opportunity(
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    cluster: &[&BuildingContact],
    bomber: UnitKind,
) -> BombingOpportunity {
    let Some(weapon) = bomber
        .stats()
        .weapons
        .iter()
        .find(|weapon| weapon.targets.covers(Domain::Ground) && weapon.splash.is_some())
    else {
        return BombingOpportunity::default();
    };
    let suppressed_mobile_units =
        current_air_defense(intelligence, cluster).suppressible_mobile_units;
    let victims = current_bombing_victims(intelligence, &suppressed_mobile_units);
    if victims.is_empty() {
        return BombingOpportunity::default();
    }

    let mut splashable = vec![false; victims.len()];
    let mut best_run_work = 0_u64;
    for target in cluster {
        let aim = building_contact_center(target);
        // Opposite headings lay the same impact line, so these are the four
        // unique axes in the eight-way tactical approximation.
        for heading in [0_u8, 32, 64, 96] {
            let impacts = bomb_salvo_impacts(observation, aim, heading, weapon);
            let mut run_work = 0_u64;
            for (index, victim) in victims.iter().enumerate() {
                let hits = impacts
                    .iter()
                    .filter(|impact| bombing_splash_reaches(**impact, victim.position, weapon))
                    .count();
                if hits == 0 {
                    continue;
                }
                splashable[index] = true;
                let damage = u64::from(weapon.damage)
                    .saturating_mul(u64::try_from(hits).unwrap_or(u64::MAX));
                run_work = run_work.saturating_add(damage.min(u64::from(victim.hit_points)));
            }
            best_run_work = best_run_work.max(run_work);
        }
    }
    if best_run_work == 0 {
        return BombingOpportunity::default();
    }

    let useful_work = victims
        .iter()
        .zip(splashable)
        .filter(|(_, splashable)| *splashable)
        .fold(0_u64, |total, (victim, _)| {
            total.saturating_add(u64::from(victim.hit_points))
        });
    let salvo_damage = u64::from(weapon.damage).saturating_mul(u64::from(weapon.salvo.max(1)));
    BombingOpportunity {
        useful: normalize_bombing_work(useful_work, salvo_damage),
        per_provider: normalize_bombing_work(best_run_work, salvo_damage),
    }
}

fn current_bombing_victims(
    intelligence: &StrategicIntelligence,
    suppressed_mobile_units: &BTreeSet<UnitId>,
) -> Vec<BombingVictim> {
    let units = intelligence.units().iter().filter_map(|contact| {
        (contact.evidence == ContactEvidence::Current
            && contact.hp > 0
            && contact.body_domain() == Domain::Ground
            && !suppressed_mobile_units.contains(&contact.id))
        .then_some(BombingVictim {
            position: contact.tile.center(),
            hit_points: contact.hp,
        })
    });
    let charges = intelligence.buildings().iter().filter_map(|contact| {
        (contact.evidence == ContactEvidence::Current
            && contact.built
            && contact.hp > 0
            && contact.kind.is_stealthy())
        .then_some(BombingVictim {
            position: building_contact_center(contact),
            hit_points: contact.hp,
        })
    });
    units.chain(charges).collect()
}

fn bomb_salvo_impacts(
    observation: &Observation,
    aim: Vec2Fx,
    heading: u8,
    weapon: &WeaponStats,
) -> Vec<Vec2Fx> {
    let direction = chassis::compass::dir(heading);
    let salvo = i32::from(weapon.salvo.max(1));
    (0..salvo)
        .map(|index| {
            let along = Fx::from_num(2 * index - (salvo - 1)) * HALF;
            clamp_to_observed_envelope(observation, aim + direction * (along * BOMB_SALVO_SPACING))
        })
        .collect()
}

fn bombing_splash_reaches(impact: Vec2Fx, victim: Vec2Fx, weapon: &WeaponStats) -> bool {
    weapon
        .splash
        .is_some_and(|radius| victim.dist_sq(impact) <= radius * radius)
}

fn clamp_to_observed_envelope(observation: &Observation, position: Vec2Fx) -> Vec2Fx {
    let max_x = Fx::from_num(observation.map_width) - HALF;
    let max_y = Fx::from_num(observation.map_height) - HALF;
    Vec2Fx::new(position.x.clamp(HALF, max_x), position.y.clamp(HALF, max_y))
}

fn building_contact_center(contact: &BuildingContact) -> Vec2Fx {
    let size = contact.kind.tier_stats(contact.tier).size;
    let far = contact.anchor.offset(size.0 - 1, size.1 - 1);
    (contact.anchor.center() + far.center()) * HALF
}

fn normalize_bombing_work(work: u64, salvo_damage: u64) -> u64 {
    if work == 0 {
        0
    } else {
        work.saturating_mul(NORMALIZED_PROVIDER)
            .div_ceil(salvo_damage.max(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentAirDefense {
    total_firepower: u64,
    suppressible_firepower: u64,
    suppressible_hp: u64,
    suppressible_mobile_units: BTreeSet<UnitId>,
    untargetable_air_firepower: u64,
    untargetable_air_hp: u64,
}

/// Fog-honest anti-air coverage over every tile occupied by the selected
/// target buildings. Sources are deduplicated in canonical identity order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TargetClusterAirDefense {
    pub(super) all_target_tiles_visible: bool,
    pub(super) sources: Vec<AirDefenseContact>,
}

pub(super) fn target_cluster_air_defense(
    intelligence: &StrategicIntelligence,
    cluster: &[&BuildingContact],
) -> TargetClusterAirDefense {
    let mut all_target_tiles_visible = !cluster.is_empty();
    let mut sources = BTreeMap::<AirDefenseSource, AirDefenseContact>::new();
    for contact in cluster {
        let (width, height) = contact.kind.tier_stats(contact.tier).size;
        for dy in 0..height {
            for dx in 0..width {
                let assessment = intelligence.air_defense_at(contact.anchor.offset(dx, dy));
                all_target_tiles_visible &= assessment.target_visible;
                for source in assessment.sources {
                    sources
                        .entry(source.source)
                        .and_modify(|known| {
                            known.firepower_per_100_ticks = known
                                .firepower_per_100_ticks
                                .max(source.firepower_per_100_ticks);
                        })
                        .or_insert(source);
                }
            }
        }
    }
    TargetClusterAirDefense {
        all_target_tiles_visible,
        sources: sources.into_values().collect(),
    }
}

fn current_air_defense(
    intelligence: &StrategicIntelligence,
    cluster: &[&BuildingContact],
) -> CurrentAirDefense {
    let sources: BTreeMap<_, _> = target_cluster_air_defense(intelligence, cluster)
        .sources
        .into_iter()
        .filter(|source| {
            source.evidence == ContactEvidence::Current
                && current_operational_aa_source(intelligence, source.source)
        })
        .map(|source| (source.source, source.firepower_per_100_ticks))
        .collect();
    let total_firepower = sources.values().fold(0_u64, |total, value| {
        total.saturating_add(u64::from(*value))
    });
    let mut suppressible_firepower = 0_u64;
    let mut suppressible_hp = 0_u64;
    let mut suppressible_mobile_units = BTreeSet::new();
    let mut untargetable_air_firepower = 0_u64;
    let mut untargetable_air_hp = 0_u64;
    for (source, source_firepower) in &sources {
        match source {
            AirDefenseSource::Unit { id, kind, tile } => {
                let Some(contact) = intelligence.units().iter().find(|contact| {
                    contact.id == *id
                        && contact.kind == *kind
                        && contact.tile == *tile
                        && contact.evidence == ContactEvidence::Current
                        && contact.hp > 0
                }) else {
                    continue;
                };
                if contact.body_domain() == Domain::Ground {
                    suppressible_firepower =
                        suppressible_firepower.saturating_add(u64::from(*source_firepower));
                    suppressible_hp = suppressible_hp.saturating_add(u64::from(contact.hp));
                    suppressible_mobile_units.insert(contact.id);
                } else {
                    untargetable_air_firepower =
                        untargetable_air_firepower.saturating_add(u64::from(*source_firepower));
                    untargetable_air_hp = untargetable_air_hp.saturating_add(u64::from(contact.hp));
                }
            }
            AirDefenseSource::Building {
                id: Some(id),
                player,
                kind,
                anchor,
            } => {
                let Some(contact) = intelligence.buildings().iter().find(|contact| {
                    contact.id == Some(*id)
                        && contact.player == *player
                        && contact.kind == *kind
                        && contact.anchor == *anchor
                        && contact.evidence == ContactEvidence::Current
                        && contact.built
                        && contact.hp > 0
                }) else {
                    continue;
                };
                suppressible_firepower =
                    suppressible_firepower.saturating_add(u64::from(*source_firepower));
                suppressible_hp = suppressible_hp.saturating_add(u64::from(contact.hp));
            }
            AirDefenseSource::Building { id: None, .. } => {}
        }
    }
    CurrentAirDefense {
        total_firepower,
        suppressible_firepower,
        suppressible_hp,
        suppressible_mobile_units,
        untargetable_air_firepower,
        untargetable_air_hp,
    }
}

pub(super) fn current_operational_aa_source(
    intelligence: &StrategicIntelligence,
    source: AirDefenseSource,
) -> bool {
    match source {
        AirDefenseSource::Unit { id, kind, tile } => intelligence.units().iter().any(|contact| {
            contact.id == id
                && contact.kind == kind
                && contact.tile == tile
                && contact.hp > 0
                && contact.evidence == ContactEvidence::Current
        }),
        AirDefenseSource::Building {
            id: Some(id),
            player,
            kind,
            anchor,
        } => intelligence.buildings().iter().any(|contact| {
            contact.id == Some(id)
                && contact.player == player
                && contact.kind == kind
                && contact.anchor == anchor
                && contact.hp > 0
                && contact.built
                && contact.evidence == ContactEvidence::Current
        }),
        AirDefenseSource::Building { id: None, .. } => false,
    }
}

fn preservation_order(family: ForceFamily, faction: oxide_sim::state::Faction) -> Vec<UnitKind> {
    match family {
        ForceFamily::Recon => vec![Role::Scout.unit_for(faction)],
        ForceFamily::Suppression => vec![UnitKind::Bombard, UnitKind::Avalanche],
        ForceFamily::Strike => vec![
            Role::AirGround.unit_for(faction),
            Role::Bomber.unit_for(faction),
        ],
    }
}

fn new_provider_order(family: ForceFamily, faction: oxide_sim::state::Faction) -> Vec<UnitKind> {
    let baseline = baseline_provider(family, faction);
    advanced_provider(family, faction)
        .map_or_else(|| vec![baseline], |advanced| vec![advanced, baseline])
}

fn baseline_provider(family: ForceFamily, faction: oxide_sim::state::Faction) -> UnitKind {
    match family {
        ForceFamily::Recon => Role::Scout.unit_for(faction),
        ForceFamily::Suppression => UnitKind::Bombard,
        ForceFamily::Strike => Role::AirGround.unit_for(faction),
    }
}

fn advanced_provider(family: ForceFamily, faction: oxide_sim::state::Faction) -> Option<UnitKind> {
    match family {
        ForceFamily::Recon => None,
        ForceFamily::Suppression => Some(UnitKind::Avalanche),
        ForceFamily::Strike => Some(Role::Bomber.unit_for(faction)),
    }
}

fn provider_capability(
    family: ForceFamily,
    kind: UnitKind,
    faction: oxide_sim::state::Faction,
) -> u64 {
    if family == ForceFamily::Recon {
        return NORMALIZED_PROVIDER;
    }
    let baseline = match family {
        ForceFamily::Recon => unreachable!("recon returned above"),
        ForceFamily::Suppression => UnitKind::Bombard,
        ForceFamily::Strike => Role::AirGround.unit_for(faction),
    };
    normalized_ratio(ground_firepower(kind), ground_firepower(baseline))
}

pub(super) fn suppression_capability(kind: UnitKind, faction: oxide_sim::state::Faction) -> u64 {
    provider_capability(ForceFamily::Suppression, kind, faction)
}

pub(super) fn strike_capability(kind: UnitKind, faction: oxide_sim::state::Faction) -> u64 {
    provider_capability(ForceFamily::Strike, kind, faction)
}

fn normalized_ratio(value: u64, baseline: u64) -> u64 {
    value
        .saturating_mul(NORMALIZED_PROVIDER)
        .div_ceil(baseline.max(1))
        .max(1)
}

fn normalized_work(work: u64, baseline_dps100: u64, effect_periods: Tick) -> u64 {
    if work == 0 {
        return 0;
    }
    work.saturating_mul(NORMALIZED_PROVIDER)
        .div_ceil(baseline_dps100.max(1).saturating_mul(effect_periods.max(1)))
}

fn ground_firepower(kind: UnitKind) -> u64 {
    kind.stats()
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.covers(Domain::Ground))
        .map(weapon_burst_dps100)
        .sum()
}

pub(super) fn building_value(kind: BuildingKind) -> u8 {
    match kind {
        BuildingKind::Crucible => 10,
        BuildingKind::Airworks => 9,
        BuildingKind::Fabricator => 8,
        BuildingKind::Foundry => 7,
        BuildingKind::Extractor => 6,
        BuildingKind::Bastion | BuildingKind::RepairBay => 5,
        BuildingKind::Reclaimer | BuildingKind::Array => 4,
        BuildingKind::Turret => 2,
        BuildingKind::FlakTurret | BuildingKind::Barricade | BuildingKind::ScuttleCharge => 0,
    }
}

fn provider_rank(
    family: ForceFamily,
    faction: oxide_sim::state::Faction,
    kind: UnitKind,
) -> (usize, UnitKind) {
    let rank = new_provider_order(family, faction)
        .into_iter()
        .position(|candidate| candidate == kind)
        .unwrap_or(usize::MAX);
    (rank, kind)
}

fn provider_priority_rank(
    profile: &ResolvedProfile,
    faction: oxide_sim::state::Faction,
    tranche: ProviderDemandTranche,
) -> (u8, u8, usize, UnitKind) {
    let priority_rank = match tranche.priority {
        ProviderPriority::Minimum => 0,
        ProviderPriority::Marginal => 1,
    };
    let family_rank = match tranche.priority {
        ProviderPriority::Minimum => match tranche.family {
            ForceFamily::Recon => 0,
            ForceFamily::Suppression => 1,
            ForceFamily::Strike => 2,
        },
        ProviderPriority::Marginal if profile.traits.air > profile.traits.siege => {
            match tranche.family {
                ForceFamily::Strike => 0,
                ForceFamily::Suppression => 1,
                ForceFamily::Recon => 2,
            }
        }
        ProviderPriority::Marginal => match tranche.family {
            ForceFamily::Suppression => 0,
            ForceFamily::Strike => 1,
            ForceFamily::Recon => 2,
        },
    };
    let kind_rank = match tranche.priority {
        ProviderPriority::Minimum => preservation_order(tranche.family, faction)
            .into_iter()
            .position(|kind| kind == tranche.kind)
            .unwrap_or(usize::MAX),
        ProviderPriority::Marginal => provider_rank(tranche.family, faction, tranche.kind).0,
    };
    (priority_rank, family_rank, kind_rank, tranche.kind)
}

fn manhattan(a: TilePos, b: TilePos) -> u32 {
    a.x.abs_diff(b.x).saturating_add(a.y.abs_diff(b.y))
}

#[cfg(test)]
mod tests;
