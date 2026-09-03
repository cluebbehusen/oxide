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
use crate::ids::{PlayerId, UnitId};
use crate::stats::{BOMB_SALVO_SPACING, BuildingKind, Domain, Role, UnitKind, WeaponStats};
use chassis::Tick;
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::TilePos;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// One lowerable production family and the total number the operation wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProviderDemand {
    pub(super) kind: UnitKind,
    pub(super) count: usize,
}

/// Strategic priority carried across package derivation and production
/// lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ProviderPriority {
    Minimum,
    Marginal,
}

/// One ordered tranche of exact providers with the same strategic priority,
/// tactical family, and concrete kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl<'a> ProductionEvidence<'a> {
    pub(super) const fn new(resources: &'a ResourceSnapshot, access: &'a ProductionAccess) -> Self {
        Self { resources, access }
    }
}

/// Primary objective and the canonical current cluster retained by connected
/// tactical admission.
#[derive(Debug, Clone, Copy)]
pub(super) struct ConnectedTargetEvidence<'a> {
    pub(super) primary: &'a BuildingContact,
    pub(super) cluster: &'a [&'a BuildingContact],
}

/// Why a current connected opportunity cannot admit its minimum package.
///
/// These reasons describe only admission of the common minimum repertoire.
/// Once that minimum is feasible, a later inability to fund or finish another
/// marginal provider simply ends opportunity scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ForcePackageRejection {
    /// A zero cadence cannot produce a deterministic future command boundary.
    InvalidDecisionCadence,
    /// The fixed preparation deadline is before the current observation.
    InvalidDeadline { observed_at: Tick, deadline: Tick },
    /// The proposed target is remembered rather than visible now.
    TargetNotCurrent,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedForcePackage {
    /// Observation tick that supplied this package's evidence.
    pub(super) derived_at: Tick,
    /// Absolute tick by which every newly requested provider must be ready.
    pub(super) preparation_deadline: Tick,
    /// Canonical current targets whose value and defenses sized this package.
    /// Tactical selection remains inside this admitted set.
    pub(super) target_anchors: Vec<TilePos>,
    pub(super) recon: Vec<ProviderDemand>,
    pub(super) suppression: Vec<ProviderDemand>,
    pub(super) strike: Vec<ProviderDemand>,
    /// Every desired provider in derivation priority order. Existing and paid
    /// providers remain represented so lowering can consume them before
    /// scheduling the missing remainder.
    pub(super) provider_priority: Vec<ProviderDemandTranche>,
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
const TARGET_CLUSTER_RADIUS: u32 = 4;
const MAX_TARGET_WEIGHT: u64 = 10;
/// The existing connected strike phase's bounded window, expressed as 100-tick
/// damage periods. This converts observed durability into useful firepower;
/// it is not a roster ceiling.
const TACTICAL_EFFECT_WINDOW: Tick = 1_200;

type PackageCandidateScore = (u64, u128, Reverse<u32>, u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::bot) enum ForceFamily {
    Recon,
    Suppression,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FundedProvider {
    kind: UnitKind,
    command_tick: Tick,
}

#[derive(Debug, Clone)]
struct FundedLane {
    eligible_kinds: Vec<UnitKind>,
    available_tick: Tick,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(Debug, Clone)]
struct PackageBuilder<'a> {
    faction: crate::state::Faction,
    observed_at: Tick,
    deadline: Tick,
    decision_cadence: Tick,
    current_scrap: u32,
    protected_forecast_scrap: u32,
    forecast: &'a ResourceForecast,
    resources: &'a ResourceSnapshot,
    committed_scrap: u32,
    production_access: &'a ProductionAccess,
    funded_providers: Vec<FundedProvider>,
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
struct FundedHorizonSearch<'a> {
    providers: &'a [FundedProvider],
    deadline: Tick,
    failed: BTreeMap<usize, Vec<Vec<FundedLaneClass>>>,
}

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
pub(super) fn derive_connected_force_package_for_cluster(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    targets: ConnectedTargetEvidence<'_>,
    production: ProductionEvidence<'_>,
    unavailable: &[UnitId],
    constraints: PreparationConstraints,
) -> Result<ConnectedForcePackage, ForcePackageRejection> {
    let ConnectedTargetEvidence {
        primary: target,
        cluster,
    } = targets;
    let ProductionEvidence {
        resources,
        access: production_access,
    } = production;
    let PreparationConstraints {
        deadline: preparation_deadline,
        decision_cadence,
        protected_forecast_scrap,
    } = constraints;
    if decision_cadence == 0 {
        return Err(ForcePackageRejection::InvalidDecisionCadence);
    }
    if preparation_deadline < observation.tick {
        return Err(ForcePackageRejection::InvalidDeadline {
            observed_at: observation.tick,
            deadline: preparation_deadline,
        });
    }
    if target.evidence != ContactEvidence::Current {
        return Err(ForcePackageRejection::TargetNotCurrent);
    }
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
                && contact.evidence == ContactEvidence::Current
                && contact.built
                && contact.hp > 0
                && building_value(contact.kind) > 0
                && manhattan(contact.anchor, target.anchor) <= TARGET_CLUSTER_RADIUS
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
    let strike_work = cluster_hp.saturating_add(target_value / MAX_TARGET_WEIGHT);
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
    let minimum_candidates =
        minimum_package_candidates(profile, template.clone(), minimum_capability);
    if minimum_candidates.is_empty() {
        return Err(diagnose_minimum_rejection(
            template,
            observation,
            resources,
            minimum_capability,
        ));
    }
    let mut builder = best_complete_portfolio(
        profile,
        useful_capability,
        bombing.useful,
        minimum_candidates,
    );

    builder.canonicalize_provider_priority(profile);
    builder.sort_demands();
    let chosen_capability = builder.capability;
    Ok(ConnectedForcePackage {
        derived_at: observation.tick,
        preparation_deadline,
        target_anchors: cluster.iter().map(|contact| contact.anchor).collect(),
        recon: builder.recon,
        suppression: builder.suppression,
        strike: builder.strike,
        provider_priority: builder.provider_priority,
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
            if candidates.is_empty() {
                return candidates;
            }
        }
    }
    candidates
}

fn diagnose_minimum_rejection(
    mut builder: PackageBuilder<'_>,
    observation: &Observation,
    resources: &ResourceSnapshot,
    minimum: NormalizedCapability,
) -> ForcePackageRejection {
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
                return ForcePackageRejection::MissingCompletedProviderCapability { family };
            }
            if let Err(failure) = builder.add_first_new_provider(
                family,
                ProviderPriority::Minimum,
                &preservation_order(family, builder.faction),
            ) {
                return match failure {
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
                };
            }
        }
    }
    unreachable!("an exhaustive minimum search cannot fail when the diagnostic path succeeds")
}

fn best_complete_portfolio<'a>(
    profile: &ResolvedProfile,
    useful: NormalizedCapability,
    useful_bombing: u64,
    minimum_candidates: Vec<PackageBuilder<'a>>,
) -> PackageBuilder<'a> {
    let mut seen = BTreeSet::new();
    let mut pending = VecDeque::new();
    for candidate in minimum_candidates {
        if seen.insert(candidate.search_key()) {
            pending.push_back(candidate);
        }
    }

    let mut best = None;
    while let Some(candidate) = pending.pop_front() {
        best = Some(match best {
            Some(current) => select_package_candidate(
                profile,
                useful,
                useful_bombing,
                current,
                candidate.clone(),
            ),
            None => candidate.clone(),
        });
        for family in [ForceFamily::Suppression, ForceFamily::Strike] {
            if candidate.capability_for(family) >= useful.for_family(family)
                && (family != ForceFamily::Strike || candidate.bombing >= useful_bombing)
            {
                continue;
            }
            for successor in candidate.provider_successors(family, ProviderPriority::Marginal) {
                if !successor.priority_is_canonical(profile) {
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
                if !advances_family && !advances_bombing {
                    continue;
                }
                if seen.insert(successor.search_key()) {
                    pending.push_back(successor);
                }
            }
        }
    }
    best.expect("the common minimum produced at least one complete package")
}

fn unique_search_states<'a>(candidates: Vec<PackageBuilder<'a>>) -> Vec<PackageBuilder<'a>> {
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.search_key()))
        .collect()
}

fn select_package_candidate<'a>(
    profile: &ResolvedProfile,
    useful: NormalizedCapability,
    useful_bombing: u64,
    conservative: PackageBuilder<'a>,
    advanced: PackageBuilder<'a>,
) -> PackageBuilder<'a> {
    let conservative_score =
        package_candidate_score(profile, useful, useful_bombing, &conservative);
    let advanced_score = package_candidate_score(profile, useful, useful_bombing, &advanced);
    if advanced_score > conservative_score {
        advanced
    } else {
        conservative
    }
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

    fn add_new_kind_variants(
        &self,
        family: ForceFamily,
        priority: ProviderPriority,
        kind: UnitKind,
    ) -> Result<Vec<Self>, AddProviderFailure> {
        let cost = kind.stats().cost;
        let mut structurally_funded = self.funded_providers.clone();
        structurally_funded.push(FundedProvider {
            kind,
            command_tick: self
                .funded_providers
                .last()
                .map_or(self.observed_at, |provider| provider.command_tick),
        });
        if !funded_providers_fit(
            self.resources,
            &structurally_funded,
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
        if !funded_providers_fit(
            successor.resources,
            &successor.funded_providers,
            successor.deadline,
            successor.production_access,
        ) {
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
        let required = self.committed_scrap.checked_add(cost)?;
        if self.current_scrap >= required {
            return Some(self.observed_at);
        }
        if self.available_scrap_at(self.deadline) < required {
            return None;
        }

        let mut earliest = self.observed_at;
        let mut latest = self.deadline;
        while earliest < latest {
            let middle = earliest.saturating_add(latest.saturating_sub(earliest) / 2);
            if self.available_scrap_at(middle) >= required {
                latest = middle;
            } else {
                earliest = middle.saturating_add(1);
            }
        }
        next_decision_tick_after(earliest, self.decision_cadence)
    }

    fn available_scrap_at(&self, tick: Tick) -> u32 {
        self.current_scrap.saturating_add(
            self.forecast
                .income_through(tick)
                .amount()
                .saturating_sub(self.protected_forecast_scrap),
        )
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
                && manhattan(contact.anchor, original_anchor) <= TARGET_CLUSTER_RADIUS
        })
        .collect()
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

fn current_operational_aa_source(
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

fn preservation_order(family: ForceFamily, faction: crate::state::Faction) -> Vec<UnitKind> {
    match family {
        ForceFamily::Recon => vec![Role::Scout.unit_for(faction)],
        ForceFamily::Suppression => vec![UnitKind::Bombard, UnitKind::Avalanche],
        ForceFamily::Strike => vec![
            Role::AirGround.unit_for(faction),
            Role::Bomber.unit_for(faction),
        ],
    }
}

fn new_provider_order(family: ForceFamily, faction: crate::state::Faction) -> Vec<UnitKind> {
    let baseline = baseline_provider(family, faction);
    advanced_provider(family, faction)
        .map_or_else(|| vec![baseline], |advanced| vec![advanced, baseline])
}

fn baseline_provider(family: ForceFamily, faction: crate::state::Faction) -> UnitKind {
    match family {
        ForceFamily::Recon => Role::Scout.unit_for(faction),
        ForceFamily::Suppression => UnitKind::Bombard,
        ForceFamily::Strike => Role::AirGround.unit_for(faction),
    }
}

fn advanced_provider(family: ForceFamily, faction: crate::state::Faction) -> Option<UnitKind> {
    match family {
        ForceFamily::Recon => None,
        ForceFamily::Suppression => Some(UnitKind::Avalanche),
        ForceFamily::Strike => Some(Role::Bomber.unit_for(faction)),
    }
}

fn provider_capability(family: ForceFamily, kind: UnitKind, faction: crate::state::Faction) -> u64 {
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

pub(super) fn suppression_capability(kind: UnitKind, faction: crate::state::Faction) -> u64 {
    provider_capability(ForceFamily::Suppression, kind, faction)
}

pub(super) fn strike_capability(kind: UnitKind, faction: crate::state::Faction) -> u64 {
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
    faction: crate::state::Faction,
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
    faction: crate::state::Faction,
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
mod tests {
    use super::super::super::intelligence::StrategicIntelligence;
    use super::super::super::observation::{BuildingObs, UnitObs};
    use super::super::super::profile::{PersonalityTraits, Specialty};
    use super::super::super::resources::{ProductionDemand, ResourceSnapshot, plan_production};
    use super::*;
    use crate::ids::{BuildingId, PlayerId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::state::Faction;
    use crate::stats::QUEUE_CAP;

    const TEST_DECISION_CADENCE: Tick = 12;

    fn constraints(deadline: Tick, protected_forecast_scrap: u32) -> PreparationConstraints {
        PreparationConstraints {
            deadline,
            decision_cadence: TEST_DECISION_CADENCE,
            protected_forecast_scrap,
        }
    }

    fn profile(air: u8, siege: u8) -> ResolvedProfile {
        profile_for(BotDifficulty::Prime, BotStance::Balanced, air, siege)
    }

    fn profile_for(
        difficulty: BotDifficulty,
        stance: BotStance,
        air: u8,
        siege: u8,
    ) -> ResolvedProfile {
        ResolvedProfile {
            difficulty,
            stance,
            personality_seed: 7,
            primary: if air > siege {
                Specialty::Air
            } else {
                Specialty::Siege
            },
            secondary: if air > siege {
                Specialty::Siege
            } else {
                Specialty::Air
            },
            traits: PersonalityTraits {
                air,
                siege,
                support: 50,
                fortification: 50,
                greed: 50,
                guile: 50,
            },
        }
    }

    fn observation(scrap: u32) -> Observation {
        Observation {
            tick: 100,
            scrap,
            map_width: 30,
            map_height: 30,
            visible: vec![true; 900],
            explored: vec![true; 900],
            ..Observation::default()
        }
    }

    fn building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(player),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn unit(id: u32, player: u8, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(player),
            kind,
            tile,
            hp: kind.stats().max_hp,
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

    fn add_producer(
        observation: &mut Observation,
        id: u32,
        kind: BuildingKind,
        anchor: TilePos,
        queue: Vec<UnitKind>,
    ) {
        observation
            .my_buildings
            .push(building(id, observation.me.0, kind, anchor));
        observation.my_queues.push(queue);
    }

    fn add_owned_building(
        observation: &mut Observation,
        id: u32,
        kind: BuildingKind,
        anchor: TilePos,
        built: bool,
    ) {
        let mut owned = building(id, observation.me.0, kind, anchor);
        owned.built = built;
        observation.my_buildings.push(owned);
        observation.my_queues.push(Vec::new());
    }

    fn add_baseline_tech(observation: &mut Observation) {
        add_producer(
            observation,
            10,
            BuildingKind::Foundry,
            TilePos::new(2, 2),
            Vec::new(),
        );
        add_producer(
            observation,
            11,
            BuildingKind::Fabricator,
            TilePos::new(5, 2),
            Vec::new(),
        );
        add_producer(
            observation,
            12,
            BuildingKind::Airworks,
            TilePos::new(8, 2),
            Vec::new(),
        );
    }

    fn add_complete_tech(observation: &mut Observation) {
        add_baseline_tech(observation);
        add_producer(
            observation,
            13,
            BuildingKind::Crucible,
            TilePos::new(11, 2),
            Vec::new(),
        );
    }

    fn intelligence_with_target(
        observation: &mut Observation,
        anti_air: usize,
    ) -> (StrategicIntelligence, BuildingContact) {
        observation.enemy_buildings.push(building(
            100,
            1,
            BuildingKind::Crucible,
            TilePos::new(20, 20),
        ));
        let anchors = [
            TilePos::new(18, 18),
            TilePos::new(20, 18),
            TilePos::new(22, 18),
            TilePos::new(18, 22),
        ];
        for (index, anchor) in anchors.into_iter().take(anti_air).enumerate() {
            observation.enemy_buildings.push(building(
                101 + u32::try_from(index).expect("small fixture"),
                1,
                BuildingKind::FlakTurret,
                anchor,
            ));
        }
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.kind == BuildingKind::Crucible)
            .expect("current target")
            .clone();
        (intelligence, target)
    }

    fn derive(
        profile: &ResolvedProfile,
        observation: &Observation,
        intelligence: &StrategicIntelligence,
        target: &BuildingContact,
        unavailable: &[UnitId],
        deadline: Tick,
    ) -> Result<ConnectedForcePackage, ForcePackageRejection> {
        derive_with_forecast_reserve(
            profile,
            observation,
            intelligence,
            target,
            unavailable,
            deadline,
            0,
        )
    }

    fn derive_with_forecast_reserve(
        profile: &ResolvedProfile,
        observation: &Observation,
        intelligence: &StrategicIntelligence,
        target: &BuildingContact,
        unavailable: &[UnitId],
        deadline: Tick,
        protected_forecast_scrap: u32,
    ) -> Result<ConnectedForcePackage, ForcePackageRejection> {
        derive_connected_force_package(
            profile,
            observation,
            intelligence,
            target,
            ProductionEvidence::new(
                &ResourceSnapshot::from_observation(observation),
                &ProductionAccess::Unrestricted,
            ),
            unavailable,
            constraints(deadline, protected_forecast_scrap),
        )
    }

    fn demand(demands: &[ProviderDemand], kind: UnitKind) -> usize {
        demands
            .iter()
            .find(|demand| demand.kind == kind)
            .map_or(0, |demand| demand.count)
    }

    fn realized_capability(
        package: &ConnectedForcePackage,
        family: ForceFamily,
        faction: Faction,
    ) -> u64 {
        let demands = match family {
            ForceFamily::Recon => &package.recon,
            ForceFamily::Suppression => &package.suppression,
            ForceFamily::Strike => &package.strike,
        };
        demands.iter().fold(0_u64, |total, demand| {
            let count = u64::try_from(demand.count).unwrap_or(u64::MAX);
            total.saturating_add(
                provider_capability(family, demand.kind, faction).saturating_mul(count),
            )
        })
    }

    fn demanded_scrap(package: &ConnectedForcePackage) -> u32 {
        package
            .recon
            .iter()
            .chain(&package.suppression)
            .chain(&package.strike)
            .fold(0_u32, |total, demand| {
                total.saturating_add(
                    demand
                        .kind
                        .stats()
                        .cost
                        .saturating_mul(u32::try_from(demand.count).unwrap_or(u32::MAX)),
                )
            })
    }

    fn baseline_portfolio_oracle(
        profile: &ResolvedProfile,
        useful: NormalizedCapability,
        scrap: u32,
        faction: Faction,
    ) -> (usize, usize, PackageCandidateScore) {
        let scout = Role::Scout.unit_for(faction);
        let strike = Role::AirGround.unit_for(faction);
        let scout_cost = scout.stats().cost;
        let suppression_cost = UnitKind::Bombard.stats().cost;
        let strike_cost = strike.stats().cost;
        let mut best: Option<(usize, usize, PackageCandidateScore)> = None;
        for suppression_count in 1..=usize::try_from(scrap / suppression_cost).unwrap_or(0) {
            for strike_count in 1..=usize::try_from(scrap / strike_cost).unwrap_or(0) {
                let cost = scout_cost
                    .saturating_add(
                        suppression_cost
                            .saturating_mul(u32::try_from(suppression_count).unwrap_or(u32::MAX)),
                    )
                    .saturating_add(
                        strike_cost.saturating_mul(u32::try_from(strike_count).unwrap_or(u32::MAX)),
                    );
                if cost > scrap {
                    continue;
                }
                let suppression = NORMALIZED_PROVIDER
                    .saturating_mul(u64::try_from(suppression_count).unwrap_or(u64::MAX));
                let strike = NORMALIZED_PROVIDER
                    .saturating_mul(u64::try_from(strike_count).unwrap_or(u64::MAX));
                let capped_suppression = suppression.min(useful.suppression);
                let capped_strike = strike.min(useful.strike);
                let total_useful = NORMALIZED_PROVIDER
                    .min(useful.recon)
                    .saturating_add(capped_suppression)
                    .saturating_add(capped_strike);
                let personality_value = u128::from(capped_suppression)
                    .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.siege)))
                    .saturating_add(
                        u128::from(capped_strike)
                            .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.air))),
                    );
                let raw = NORMALIZED_PROVIDER
                    .saturating_add(suppression)
                    .saturating_add(strike);
                let score = (total_useful, personality_value, Reverse(cost), raw);
                let candidate = (suppression_count, strike_count, score);
                if best.as_ref().is_none_or(|current| candidate.2 > current.2) {
                    best = Some(candidate);
                }
            }
        }
        best.expect("the oracle budget funds the common baseline minimum")
    }

    fn baseline_aa_fixture(
        scrap: u32,
        target_hp: u32,
        first_flak_hp: u32,
    ) -> (Observation, StrategicIntelligence, BuildingContact) {
        let mut observation = observation(scrap);
        add_baseline_tech(&mut observation);
        let mut target = building(100, 1, BuildingKind::Foundry, TilePos::new(20, 20));
        target.hp = target_hp;
        let mut first_flak = building(101, 1, BuildingKind::FlakTurret, TilePos::new(18, 18));
        first_flak.hp = first_flak_hp;
        observation.enemy_buildings.extend([
            target,
            first_flak,
            building(102, 1, BuildingKind::FlakTurret, TilePos::new(20, 18)),
        ]);
        observation
            .enemy_units
            .push(unit(200, 1, UnitKind::Stinger, TilePos::new(20, 19)));
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.id == Some(BuildingId(100)))
            .expect("current Foundry target")
            .clone();
        (observation, intelligence, target)
    }

    fn minimum_connected_cost(faction: Faction) -> u32 {
        Role::Scout
            .unit_for(faction)
            .stats()
            .cost
            .saturating_add(UnitKind::Bombard.stats().cost)
            .saturating_add(Role::AirGround.unit_for(faction).stats().cost)
    }

    fn add_valuable_cluster(observation: &mut Observation) {
        for (index, (kind, anchor)) in [
            (BuildingKind::Foundry, TilePos::new(20, 17)),
            (BuildingKind::Airworks, TilePos::new(17, 20)),
            (BuildingKind::Fabricator, TilePos::new(22, 20)),
            (BuildingKind::Extractor, TilePos::new(20, 22)),
        ]
        .into_iter()
        .enumerate()
        {
            observation.enemy_buildings.push(building(
                150 + u32::try_from(index).expect("small fixture"),
                1,
                kind,
                anchor,
            ));
        }
    }

    fn add_dense_ground_targets(observation: &mut Observation) {
        for (index, tile) in [
            TilePos::new(18, 20),
            TilePos::new(19, 19),
            TilePos::new(19, 21),
            TilePos::new(20, 18),
            TilePos::new(20, 22),
            TilePos::new(21, 19),
            TilePos::new(21, 22),
            TilePos::new(22, 20),
            TilePos::new(22, 21),
            TilePos::new(23, 20),
        ]
        .into_iter()
        .enumerate()
        {
            observation.enemy_units.push(unit(
                300 + u32::try_from(index).expect("small fixture"),
                1,
                UnitKind::Lancer,
                tile,
            ));
        }
    }

    #[test]
    fn full_salvo_values_moth_above_single_bomb_condor() {
        assert!(ground_firepower(UnitKind::Moth) > ground_firepower(UnitKind::Condor));
        assert!(
            provider_capability(ForceFamily::Strike, UnitKind::Moth, Faction::Cupric)
                > NORMALIZED_PROVIDER
        );
    }

    #[test]
    fn bombing_geometry_uses_the_real_salvo_spacing_and_axis() {
        let observation = observation(0);
        let weapon = UnitKind::Moth
            .stats()
            .weapons
            .first()
            .expect("Moth has a bomb stick");
        let aim = TilePos::new(10, 10).center();
        let horizontal = bomb_salvo_impacts(&observation, aim, 0, weapon);
        let vertical = bomb_salvo_impacts(&observation, aim, 64, weapon);
        assert_eq!(horizontal.len(), usize::from(weapon.salvo));
        assert_eq!(
            horizontal[1].x - horizontal[0].x,
            BOMB_SALVO_SPACING,
            "adjacent impacts use the authoritative stick spacing"
        );
        assert_eq!(horizontal[1].y, horizontal[0].y);
        assert_eq!(vertical[1].x, vertical[0].x);
        assert_eq!(vertical[1].y - vertical[0].y, BOMB_SALVO_SPACING);

        let along_horizontal_axis = TilePos::new(13, 10).center();
        assert!(horizontal.iter().any(|impact| bombing_splash_reaches(
            *impact,
            along_horizontal_axis,
            weapon
        )));
        assert!(vertical.iter().all(|impact| !bombing_splash_reaches(
            *impact,
            along_horizontal_axis,
            weapon
        )));
    }

    #[test]
    fn bombing_opportunity_counts_each_current_victim_once() {
        let mut observation = observation(0);
        observation.faction = Faction::Cupric;
        observation.enemy_buildings.extend([
            building(100, 1, BuildingKind::Crucible, TilePos::new(20, 20)),
            building(101, 1, BuildingKind::ScuttleCharge, TilePos::new(19, 20)),
        ]);
        observation
            .enemy_units
            .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&observation);
        let cluster = current_target_cluster(&intelligence, PlayerId(1), TilePos::new(20, 20));
        let opportunity =
            current_bombing_opportunity(&observation, &intelligence, &cluster, UnitKind::Moth);
        let vulnerable_hp = u64::from(UnitKind::Lancer.stats().max_hp)
            .saturating_add(u64::from(BuildingKind::ScuttleCharge.base_stats().max_hp));
        let weapon = UnitKind::Moth.stats().weapons[0];
        let salvo_damage = u64::from(weapon.damage) * u64::from(weapon.salvo);

        assert_eq!(
            opportunity.useful,
            normalize_bombing_work(vulnerable_hp, salvo_damage),
            "a victim reachable on several candidate axes contributes its HP once"
        );
        assert!(opportunity.per_provider > 0);
        assert!(opportunity.per_provider <= opportunity.useful);
    }

    #[test]
    fn remembered_ground_contacts_do_not_create_bombing_opportunity() {
        let mut seen = observation(0);
        seen.enemy_buildings.push(building(
            100,
            1,
            BuildingKind::Crucible,
            TilePos::new(20, 20),
        ));
        seen.enemy_units
            .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&seen);
        let seen_cluster = current_target_cluster(&intelligence, PlayerId(1), TilePos::new(20, 20));
        assert!(
            current_bombing_opportunity(&seen, &intelligence, &seen_cluster, UnitKind::Condor)
                .useful
                > 0
        );

        let mut later = observation(0);
        later.tick = seen.tick + TEST_DECISION_CADENCE;
        later.visible.fill(false);
        later.explored.fill(true);
        later.enemy_buildings.push(building(
            100,
            1,
            BuildingKind::Crucible,
            TilePos::new(20, 20),
        ));
        for tile in [
            TilePos::new(20, 20),
            TilePos::new(21, 20),
            TilePos::new(20, 21),
            TilePos::new(21, 21),
        ] {
            let index = usize::try_from(tile.y * later.map_width + tile.x).expect("visible tile");
            later.visible[index] = true;
        }
        intelligence.update(&later);
        assert_eq!(
            intelligence.units().first().map(|contact| contact.evidence),
            Some(ContactEvidence::Remembered)
        );
        let later_cluster =
            current_target_cluster(&intelligence, PlayerId(1), TilePos::new(20, 20));
        assert_eq!(
            current_bombing_opportunity(&later, &intelligence, &later_cluster, UnitKind::Condor,),
            BombingOpportunity::default()
        );
    }

    #[test]
    fn suppressible_air_defense_does_not_double_discount_bombing() {
        let mut without_flak = observation(0);
        without_flak.faction = Faction::Cupric;
        without_flak.enemy_buildings.push(building(
            100,
            1,
            BuildingKind::Crucible,
            TilePos::new(20, 20),
        ));
        without_flak
            .enemy_units
            .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
        let mut without_flak_intelligence = StrategicIntelligence::default();
        without_flak_intelligence.update(&without_flak);
        let without_flak_cluster = current_target_cluster(
            &without_flak_intelligence,
            PlayerId(1),
            TilePos::new(20, 20),
        );
        let expected = current_bombing_opportunity(
            &without_flak,
            &without_flak_intelligence,
            &without_flak_cluster,
            UnitKind::Moth,
        );

        let mut with_flak = without_flak;
        with_flak.enemy_buildings.push(building(
            101,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(19, 20),
        ));
        let mut with_flak_intelligence = StrategicIntelligence::default();
        with_flak_intelligence.update(&with_flak);
        let with_flak_cluster =
            current_target_cluster(&with_flak_intelligence, PlayerId(1), TilePos::new(20, 20));

        assert_eq!(
            current_bombing_opportunity(
                &with_flak,
                &with_flak_intelligence,
                &with_flak_cluster,
                UnitKind::Moth,
            ),
            expected,
            "the suppression family owns current ground-targetable anti-air risk"
        );
    }

    #[test]
    fn mobile_air_defense_is_suppression_work_not_optional_bombing_value() {
        let mut baseline = observation(0);
        baseline.faction = Faction::Cupric;
        baseline.enemy_buildings.push(building(
            100,
            1,
            BuildingKind::Crucible,
            TilePos::new(20, 20),
        ));
        baseline
            .enemy_units
            .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
        let mut baseline_intelligence = StrategicIntelligence::default();
        baseline_intelligence.update(&baseline);
        let baseline_cluster =
            current_target_cluster(&baseline_intelligence, PlayerId(1), TilePos::new(20, 20));
        let expected = current_bombing_opportunity(
            &baseline,
            &baseline_intelligence,
            &baseline_cluster,
            UnitKind::Moth,
        );

        let mut defended = baseline;
        defended.enemy_units.extend([
            unit(201, 1, UnitKind::Stinger, TilePos::new(20, 19)),
            unit(202, 1, UnitKind::Flakhound, TilePos::new(19, 20)),
        ]);
        defended.enemy_units.sort_unstable_by_key(|unit| unit.id);
        let mut defended_intelligence = StrategicIntelligence::default();
        defended_intelligence.update(&defended);
        let defended_cluster =
            current_target_cluster(&defended_intelligence, PlayerId(1), TilePos::new(20, 20));
        let defense = current_air_defense(&defended_intelligence, &defended_cluster);

        assert_eq!(
            defense.suppressible_mobile_units,
            BTreeSet::from([UnitId(201), UnitId(202)])
        );
        assert_eq!(
            current_bombing_opportunity(
                &defended,
                &defended_intelligence,
                &defended_cluster,
                UnitKind::Moth,
            ),
            expected,
            "mandatory mobile-AA suppression must not also manufacture optional bomber demand"
        );
        assert!(
            expected.useful > 0,
            "the nearby non-AA unit remains bombing value"
        );
    }

    #[test]
    fn extreme_coordinates_do_not_wrap_into_the_target_cluster() {
        assert_eq!(
            manhattan(
                TilePos::new(i32::MIN, i32::MIN),
                TilePos::new(i32::MAX, i32::MAX),
            ),
            u32::MAX
        );
    }

    #[test]
    fn invalid_deadlines_and_targets_have_distinct_rejections() {
        let mut observation = observation(10_000);
        add_complete_tech(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let resources = ResourceSnapshot::from_observation(&observation);

        assert_eq!(
            derive_connected_force_package(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
                &[],
                PreparationConstraints {
                    deadline: 500,
                    decision_cadence: 0,
                    protected_forecast_scrap: 0,
                },
            ),
            Err(ForcePackageRejection::InvalidDecisionCadence)
        );
        assert_eq!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                observation.tick - 1,
            ),
            Err(ForcePackageRejection::InvalidDeadline {
                observed_at: observation.tick,
                deadline: observation.tick - 1,
            })
        );

        let mut remembered = target.clone();
        remembered.evidence = ContactEvidence::Remembered;
        assert_eq!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &remembered,
                &[],
                500,
            ),
            Err(ForcePackageRejection::TargetNotCurrent)
        );

        let mut non_actionable = target.clone();
        non_actionable.hp = 0;
        assert_eq!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &non_actionable,
                &[],
                500,
            ),
            Err(ForcePackageRejection::TargetNotActionable)
        );
    }

    #[test]
    fn deadline_observation_can_revalidate_live_providers_but_cannot_start_new_work() {
        let mut live = observation(0);
        add_complete_tech(&mut live);
        live.my_units.extend([
            unit(
                40,
                live.me.0,
                Role::Scout.unit_for(live.faction),
                TilePos::new(7, 7),
            ),
            unit(41, live.me.0, UnitKind::Bombard, TilePos::new(8, 7)),
            unit(
                42,
                live.me.0,
                Role::AirGround.unit_for(live.faction),
                TilePos::new(9, 7),
            ),
        ]);
        let (live_intelligence, live_target) = intelligence_with_target(&mut live, 0);

        let package = derive(
            &profile(50, 50),
            &live,
            &live_intelligence,
            &live_target,
            &[],
            live.tick,
        )
        .expect("the deadline observation may retain providers that already exist");
        assert_eq!(
            package
                .recon
                .iter()
                .map(|demand| demand.count)
                .sum::<usize>(),
            1
        );
        assert_eq!(
            package
                .suppression
                .iter()
                .map(|demand| demand.count)
                .sum::<usize>(),
            1
        );
        assert_eq!(
            package
                .strike
                .iter()
                .map(|demand| demand.count)
                .sum::<usize>(),
            1
        );

        let mut unbuilt = observation(50_000);
        add_complete_tech(&mut unbuilt);
        let (unbuilt_intelligence, unbuilt_target) = intelligence_with_target(&mut unbuilt, 0);
        assert!(
            derive(
                &profile(50, 50),
                &unbuilt,
                &unbuilt_intelligence,
                &unbuilt_target,
                &[],
                unbuilt.tick,
            )
            .is_err(),
            "scrap and idle factories cannot begin work on the deadline observation"
        );
    }

    #[test]
    fn minimum_rejections_distinguish_resources_capability_and_timing() {
        let exact_cost = minimum_connected_cost(Faction::Ferrous);
        let mut underfunded = observation(exact_cost - 1);
        add_complete_tech(&mut underfunded);
        let (underfunded_intelligence, underfunded_target) =
            intelligence_with_target(&mut underfunded, 0);
        assert_eq!(
            derive(
                &profile(50, 50),
                &underfunded,
                &underfunded_intelligence,
                &underfunded_target,
                &[],
                500,
            ),
            Err(ForcePackageRejection::InsufficientResources {
                family: ForceFamily::Strike,
                required_scrap: exact_cost,
                available_scrap: exact_cost - 1,
                deadline_shortfall: 1,
            })
        );

        let mut missing_strike_producer = observation(10_000);
        add_producer(
            &mut missing_strike_producer,
            10,
            BuildingKind::Foundry,
            TilePos::new(2, 2),
            Vec::new(),
        );
        add_producer(
            &mut missing_strike_producer,
            11,
            BuildingKind::Fabricator,
            TilePos::new(5, 2),
            Vec::new(),
        );
        missing_strike_producer.my_units.push(unit(
            40,
            missing_strike_producer.me.0,
            Role::Scout.unit_for(missing_strike_producer.faction),
            TilePos::new(7, 7),
        ));
        let (missing_intelligence, missing_target) =
            intelligence_with_target(&mut missing_strike_producer, 0);
        assert_eq!(
            derive(
                &profile(50, 50),
                &missing_strike_producer,
                &missing_intelligence,
                &missing_target,
                &[],
                1_000,
            ),
            Err(ForcePackageRejection::MissingCompletedProviderCapability {
                family: ForceFamily::Strike,
            })
        );

        let mut too_late = observation(10_000);
        add_complete_tech(&mut too_late);
        too_late.my_units.push(unit(
            40,
            too_late.me.0,
            Role::Scout.unit_for(too_late.faction),
            TilePos::new(7, 7),
        ));
        too_late.my_units.push(unit(
            41,
            too_late.me.0,
            Role::AirGround.unit_for(too_late.faction),
            TilePos::new(8, 7),
        ));
        let (late_intelligence, late_target) = intelligence_with_target(&mut too_late, 0);
        let deadline = too_late.tick + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1;
        assert_eq!(
            derive(
                &profile(50, 50),
                &too_late,
                &late_intelligence,
                &late_target,
                &[],
                deadline,
            ),
            Err(ForcePackageRejection::PreparationWindowTooShort {
                family: ForceFamily::Suppression,
                observed_at: too_late.tick,
                deadline,
            })
        );
    }

    #[test]
    fn current_air_domain_aa_rejects_a_ground_suppression_package() {
        let mut observation = observation(10_000);
        add_complete_tech(&mut observation);
        let interceptor_tile = TilePos::new(20, 19);
        observation
            .enemy_units
            .push(unit(200, 1, UnitKind::Talon, interceptor_tile));
        let (mut intelligence, target) = intelligence_with_target(&mut observation, 0);

        let rejection = derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect_err("ground artillery cannot suppress a current airborne interceptor");
        let ForcePackageRejection::UntargetableCurrentAirDefense {
            firepower,
            hit_points,
        } = rejection
        else {
            panic!("unexpected rejection: {rejection:?}");
        };
        assert!(firepower > 0);
        assert_eq!(hit_points, u64::from(UnitKind::Talon.stats().max_hp));

        let mut grounded = observation.clone();
        grounded.enemy_units[0].grounded = true;
        let (grounded_intelligence, grounded_target) = intelligence_with_target(&mut grounded, 0);
        let grounded_package = derive(
            &profile(50, 50),
            &grounded,
            &grounded_intelligence,
            &grounded_target,
            &[],
            2_500,
        )
        .expect("ground artillery can suppress a currently landed interceptor");
        assert!(grounded_package.suppressible_aa_firepower > 0);
        assert_eq!(grounded_package.observed_aa_firepower, firepower);

        let mut later = observation.clone();
        later.tick += 1;
        later.enemy_units.clear();
        let index = usize::try_from(interceptor_tile.y * later.map_width + interceptor_tile.x)
            .expect("fixture index");
        later.visible[index] = false;
        intelligence.update(&later);
        let current_target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.id == target.id)
            .expect("target remains current")
            .clone();
        assert!(
            derive(
                &profile(50, 50),
                &later,
                &intelligence,
                &current_target,
                &[],
                2_500,
            )
            .is_ok(),
            "remembered airborne AA is risk evidence, not a current untargetable blocker"
        );
    }

    #[test]
    fn minimum_package_has_exact_scrap_deadline_and_slot_boundaries() {
        let exact_cost = minimum_connected_cost(Faction::Ferrous);
        let mut exact = observation(exact_cost);
        add_complete_tech(&mut exact);
        let (intelligence, target) = intelligence_with_target(&mut exact, 0);

        let package = derive(&profile(50, 50), &exact, &intelligence, &target, &[], 500)
            .expect("the exact lower-tier minimum is feasible");
        assert_eq!(
            package.recon.iter().map(|item| item.count).sum::<usize>(),
            1
        );
        assert_eq!(
            package
                .suppression
                .iter()
                .map(|item| item.count)
                .sum::<usize>(),
            1
        );
        assert_eq!(
            package.strike.iter().map(|item| item.count).sum::<usize>(),
            1
        );
        assert!(
            package.chosen_capability.strike < package.useful_capability.strike,
            "inability to fund marginal scale must not reject a feasible minimum"
        );

        let mut one_scrap_short = exact.clone();
        one_scrap_short.scrap = exact_cost - 1;
        assert!(
            derive(
                &profile(50, 50),
                &one_scrap_short,
                &intelligence,
                &target,
                &[],
                500,
            )
            .is_err()
        );
        assert!(
            derive(
                &profile(50, 50),
                &exact,
                &intelligence,
                &target,
                &[],
                exact.tick + Tick::from(UnitKind::Bombard.stats().train_ticks) - 2,
            )
            .is_err(),
            "the Bombard cannot finish one tick before its conservative ready boundary"
        );

        let mut no_suppression_slot = exact.clone();
        let fabricator = no_suppression_slot
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Fabricator)
            .expect("fabricator");
        no_suppression_slot.my_queues[fabricator] = vec![UnitKind::Lancer; QUEUE_CAP];
        assert!(
            derive(
                &profile(50, 50),
                &no_suppression_slot,
                &intelligence,
                &target,
                &[],
                500,
            )
            .is_err(),
            "a full Fabricator queue is not a production slot"
        );
    }

    #[test]
    fn a_full_queue_can_supply_the_minimum_after_it_drains() {
        let mut observation = observation(10_000);
        add_producer(
            &mut observation,
            10,
            BuildingKind::Foundry,
            TilePos::new(2, 2),
            Vec::new(),
        );
        add_producer(
            &mut observation,
            11,
            BuildingKind::Fabricator,
            TilePos::new(5, 2),
            vec![UnitKind::Lancer; QUEUE_CAP],
        );
        add_producer(
            &mut observation,
            12,
            BuildingKind::Airworks,
            TilePos::new(8, 2),
            Vec::new(),
        );
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let queue_ticks = u64::try_from(QUEUE_CAP).expect("small queue")
            * Tick::from(UnitKind::Lancer.stats().train_ticks);
        let ready_after =
            observation.tick + queue_ticks + Tick::from(UnitKind::Bombard.stats().train_ticks);

        assert!(matches!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                ready_after - 1,
            ),
            Err(ForcePackageRejection::PreparationWindowTooShort {
                family: ForceFamily::Suppression,
                ..
            })
        ));

        let package = derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            ready_after,
        )
        .expect("the lane can refill after its paid queue drains");
        assert_eq!(demand(&package.suppression, UnitKind::Bombard), 1);
    }

    #[test]
    fn only_completed_income_can_make_a_forecast_funded_package_feasible() {
        let exact_cost = minimum_connected_cost(Faction::Ferrous);
        let mut unfinished = observation(exact_cost - 1);
        add_complete_tech(&mut unfinished);
        add_owned_building(
            &mut unfinished,
            20,
            BuildingKind::Reclaimer,
            TilePos::new(14, 2),
            false,
        );
        let (intelligence, target) = intelligence_with_target(&mut unfinished, 0);
        assert!(
            derive(
                &profile(50, 50),
                &unfinished,
                &intelligence,
                &target,
                &[],
                500,
            )
            .is_err()
        );

        let mut completed = unfinished.clone();
        completed
            .my_buildings
            .iter_mut()
            .find(|building| building.id == BuildingId(20))
            .expect("reclaimer")
            .built = true;
        let package = derive(
            &profile(50, 50),
            &completed,
            &intelligence,
            &target,
            &[],
            500,
        )
        .expect("completed recurring income funds the final scrap in time");
        assert!(package.forecast_scrap > 0);
    }

    #[test]
    fn forecast_income_waits_for_the_next_bot_decision_before_training() {
        let strike = Role::AirGround.unit_for(Faction::Ferrous);
        let mut observation = observation(strike.stats().cost - 1);
        add_complete_tech(&mut observation);
        add_owned_building(
            &mut observation,
            20,
            BuildingKind::Reclaimer,
            TilePos::new(14, 2),
            true,
        );
        observation.my_units.push(unit(
            40,
            observation.me.0,
            Role::Scout.unit_for(observation.faction),
            TilePos::new(7, 7),
        ));
        observation.my_units.push(unit(
            41,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(8, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let income_period = crate::stats::RECLAIMER_PERIOD;
        let payment_tick = observation.tick.div_ceil(income_period) * income_period;
        let next_decision = next_decision_tick_after(payment_tick, TEST_DECISION_CADENCE)
            .expect("the fixture has a following cadence");
        let falsely_ready_if_command_preceded_income =
            payment_tick + Tick::from(strike.stats().train_ticks);
        let first_observable = next_decision + Tick::from(strike.stats().train_ticks);

        assert!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                falsely_ready_if_command_preceded_income,
            )
            .is_err(),
            "income paid after commands on P cannot fund a Train command on P"
        );
        assert!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                first_observable,
            )
            .is_ok(),
            "the provider can first appear after training starts on the next decision cadence"
        );
    }

    #[test]
    fn forecast_funded_training_can_fit_prime_cadence_but_not_a_slower_cadence() {
        let strike = Role::AirGround.unit_for(Faction::Ferrous);
        let mut observation = observation(strike.stats().cost - 1);
        add_complete_tech(&mut observation);
        add_owned_building(
            &mut observation,
            20,
            BuildingKind::Reclaimer,
            TilePos::new(14, 2),
            true,
        );
        observation.my_units.push(unit(
            40,
            observation.me.0,
            Role::Scout.unit_for(observation.faction),
            TilePos::new(7, 7),
        ));
        observation.my_units.push(unit(
            41,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(8, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let resources = ResourceSnapshot::from_observation(&observation);
        let access = ProductionAccess::Unrestricted;
        let payment_tick = observation.tick.div_ceil(crate::stats::RECLAIMER_PERIOD)
            * crate::stats::RECLAIMER_PERIOD;
        let prime_cadence =
            crate::bot::difficulty::DifficultyTuning::for_level(BotDifficulty::Prime).cadence;
        assert_eq!(prime_cadence, 12);
        let deadline = next_decision_tick_after(payment_tick, prime_cadence)
            .expect("the fixture has a Prime command cadence")
            + Tick::from(strike.stats().train_ticks);

        let prime = derive_connected_force_package(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &access),
            &[],
            PreparationConstraints {
                deadline,
                decision_cadence: prime_cadence,
                protected_forecast_scrap: 0,
            },
        );
        assert!(
            prime.is_ok(),
            "Prime can issue the forecast-funded Train command soon enough: {prime:?}"
        );

        let slower = derive_connected_force_package(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &access),
            &[],
            PreparationConstraints {
                deadline,
                decision_cadence: 24,
                protected_forecast_scrap: 0,
            },
        );
        assert_eq!(
            slower,
            Err(ForcePackageRejection::PreparationWindowTooShort {
                family: ForceFamily::Strike,
                observed_at: observation.tick,
                deadline,
            }),
            "a 24-tick controller cannot spend that same payment before the fixed deadline"
        );
    }

    #[test]
    fn older_forecast_shortfall_owns_income_before_a_new_package() {
        let strike = Role::AirGround.unit_for(Faction::Ferrous);
        let mut observation = observation(strike.stats().cost - 1);
        add_complete_tech(&mut observation);
        add_owned_building(
            &mut observation,
            20,
            BuildingKind::Reclaimer,
            TilePos::new(14, 2),
            true,
        );
        observation.my_units.push(unit(
            40,
            observation.me.0,
            Role::Scout.unit_for(observation.faction),
            TilePos::new(7, 7),
        ));
        observation.my_units.push(unit(
            41,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(8, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let income_period = crate::stats::RECLAIMER_PERIOD;
        let first_payment = observation.tick.div_ceil(income_period) * income_period;
        let first_command = next_decision_tick_after(first_payment, TEST_DECISION_CADENCE)
            .expect("first command cadence");
        let first_deadline = first_command + Tick::from(strike.stats().train_ticks);

        assert!(
            derive_with_forecast_reserve(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                first_deadline,
                0,
            )
            .is_ok(),
            "the first forecast scrap can fund the package when unclaimed"
        );
        assert!(
            derive_with_forecast_reserve(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                first_deadline,
                1,
            )
            .is_err(),
            "an older one-scrap shortfall owns the first payment and cannot be double-promised"
        );

        let second_payment = first_payment + income_period;
        let second_command = next_decision_tick_after(second_payment, TEST_DECISION_CADENCE)
            .expect("second command cadence");
        let later_deadline = second_command + Tick::from(strike.stats().train_ticks);
        assert!(
            derive_with_forecast_reserve(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                later_deadline,
                1,
            )
            .is_ok(),
            "later income remains available after honoring the older promise"
        );
    }

    #[test]
    fn longer_deadline_never_loses_useful_capability() {
        let mut observation = observation(1_460);
        add_complete_tech(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let resources = ResourceSnapshot::from_observation(&observation);

        let early = derive_connected_force_package(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(500, 0),
        )
        .expect("lower tiers fit the early deadline");
        assert_eq!(demand(&early.suppression, UnitKind::Avalanche), 0);
        assert_eq!(demand(&early.strike, UnitKind::Condor), 0);
        assert!(demand(&early.suppression, UnitKind::Bombard) > 0);
        assert!(demand(&early.strike, UnitKind::Buzzard) > 0);

        let late = derive_connected_force_package(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(2_000, 0),
        )
        .expect("the later deadline retains every earlier feasible portfolio");
        assert!(
            capped_useful_objective(
                &profile(50, 50),
                late.useful_capability,
                late.useful_bombing,
                late.chosen_capability,
                late.chosen_bombing,
            ) >= capped_useful_objective(
                &profile(50, 50),
                early.useful_capability,
                early.useful_bombing,
                early.chosen_capability,
                early.chosen_bombing,
            ),
            "a longer preparation window must not weaken the useful package"
        );
    }

    #[test]
    fn wealth_and_completed_throughput_scale_realized_force_monotonically() {
        let mut constrained = observation(minimum_connected_cost(Faction::Ferrous));
        add_complete_tech(&mut constrained);
        add_valuable_cluster(&mut constrained);
        let (constrained_intelligence, constrained_target) =
            intelligence_with_target(&mut constrained, 0);
        let constrained_package = derive(
            &profile(80, 20),
            &constrained,
            &constrained_intelligence,
            &constrained_target,
            &[],
            2_200,
        )
        .expect("minimum package");

        let mut wealthy = constrained.clone();
        wealthy.scrap = 10_000;
        let wealthy_package = derive(
            &profile(80, 20),
            &wealthy,
            &constrained_intelligence,
            &constrained_target,
            &[],
            2_200,
        )
        .expect("wealthy package");
        let constrained_strike = realized_capability(
            &constrained_package,
            ForceFamily::Strike,
            constrained.faction,
        );
        let wealthy_strike =
            realized_capability(&wealthy_package, ForceFamily::Strike, wealthy.faction);
        assert!(wealthy_strike > constrained_strike);
        assert!(
            realized_capability(&wealthy_package, ForceFamily::Suppression, wealthy.faction,)
                >= realized_capability(
                    &constrained_package,
                    ForceFamily::Suppression,
                    constrained.faction,
                )
        );

        add_producer(
            &mut wealthy,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            Vec::new(),
        );
        let additional_throughput = derive(
            &profile(80, 20),
            &wealthy,
            &constrained_intelligence,
            &constrained_target,
            &[],
            2_200,
        )
        .expect("second completed Airworks contributes throughput");
        assert!(
            realized_capability(&additional_throughput, ForceFamily::Strike, wealthy.faction,)
                > wealthy_strike
        );
    }

    #[test]
    fn exact_wealth_boundary_cannot_turn_an_admitted_package_into_rejection() {
        let mut scarce = observation(759);
        add_complete_tech(&mut scarce);
        let (intelligence, target) = intelligence_with_target(&mut scarce, 0);
        let scarce_package = derive(
            &profile(50, 50),
            &scarce,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect("759 scrap can fund the complete lower-tier package");

        let mut one_more = scarce.clone();
        one_more.scrap = 760;
        let one_more_package = derive(
            &profile(50, 50),
            &one_more,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect("making one advanced provider affordable cannot reject the package");

        assert!(
            capped_useful_objective(
                &profile(50, 50),
                one_more_package.useful_capability,
                one_more_package.useful_bombing,
                one_more_package.chosen_capability,
                one_more_package.chosen_bombing,
            ) >= capped_useful_objective(
                &profile(50, 50),
                scarce_package.useful_capability,
                scarce_package.useful_bombing,
                scarce_package.chosen_capability,
                scarce_package.chosen_bombing,
            )
        );
    }

    #[test]
    fn dense_valuable_target_selects_new_bombers_for_both_factions() {
        for faction in [Faction::Ferrous, Faction::Cupric] {
            let mut observation = observation(10_000);
            observation.faction = faction;
            add_complete_tech(&mut observation);
            add_producer(
                &mut observation,
                14,
                BuildingKind::Airworks,
                TilePos::new(14, 2),
                Vec::new(),
            );
            add_valuable_cluster(&mut observation);
            add_dense_ground_targets(&mut observation);
            let (intelligence, target) = intelligence_with_target(&mut observation, 0);

            let package = derive(
                &profile(90, 10),
                &observation,
                &intelligence,
                &target,
                &[],
                5_000,
            )
            .expect("a wealthy full-tech package can exploit visible collateral");

            assert!(package.useful_bombing > 0, "{faction:?}: {package:?}");
            assert!(package.chosen_bombing > 0, "{faction:?}: {package:?}");
            assert!(
                demand(&package.strike, Role::Bomber.unit_for(faction)) > 0,
                "{faction:?}: {package:?}"
            );
            assert!(
                demand(&package.strike, Role::AirGround.unit_for(faction)) > 0,
                "the common direct-strike minimum remains present: {package:?}"
            );
        }
    }

    #[test]
    fn dense_target_falls_back_when_bomber_tech_or_time_is_unavailable() {
        for faction in [Faction::Ferrous, Faction::Cupric] {
            let mut baseline = observation(10_000);
            baseline.faction = faction;
            add_baseline_tech(&mut baseline);
            add_dense_ground_targets(&mut baseline);
            let (baseline_intelligence, baseline_target) =
                intelligence_with_target(&mut baseline, 0);
            let baseline_package = derive(
                &profile(90, 10),
                &baseline,
                &baseline_intelligence,
                &baseline_target,
                &[],
                5_000,
            )
            .expect("a dense target does not require advanced bomber tech");
            assert!(baseline_package.useful_bombing > 0);
            assert_eq!(baseline_package.chosen_bombing, 0);
            assert_eq!(
                demand(&baseline_package.strike, Role::Bomber.unit_for(faction)),
                0
            );
            assert!(demand(&baseline_package.strike, Role::AirGround.unit_for(faction)) > 0);

            let mut rushed = observation(10_000);
            rushed.faction = faction;
            add_complete_tech(&mut rushed);
            add_dense_ground_targets(&mut rushed);
            let (rushed_intelligence, rushed_target) = intelligence_with_target(&mut rushed, 0);
            let rushed_package = derive(
                &profile(90, 10),
                &rushed,
                &rushed_intelligence,
                &rushed_target,
                &[],
                500,
            )
            .expect("the short horizon still admits its lower-tier minimum");
            assert!(rushed_package.useful_bombing > 0);
            assert_eq!(rushed_package.chosen_bombing, 0);
            assert_eq!(
                demand(&rushed_package.strike, Role::Bomber.unit_for(faction)),
                0
            );
            assert!(demand(&rushed_package.strike, Role::AirGround.unit_for(faction)) > 0);
        }
    }

    #[test]
    fn air_personality_changes_a_real_competitive_mix_without_gating_bombers() {
        let mut template = observation(10_000);
        add_complete_tech(&mut template);
        add_producer(
            &mut template,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            Vec::new(),
        );
        add_valuable_cluster(&mut template);
        add_dense_ground_targets(&mut template);
        let (intelligence, target) = intelligence_with_target(&mut template, 0);
        let restrained_profile = profile(10, 50);

        let restrained_rich = derive(
            &restrained_profile,
            &template,
            &intelligence,
            &target,
            &[],
            5_000,
        )
        .expect("dense evidence keeps bombing available to an air-light personality");
        assert!(
            demand(&restrained_rich.strike, UnitKind::Condor) > 0,
            "personality must not gate the bomber repertoire: {restrained_rich:?}"
        );

        let air_profile = profile(90, 50);
        let mut witness = None;
        let victim_max_hp = UnitKind::Lancer.stats().max_hp;
        for scrap in (1_000..=1_200).step_by(10) {
            for visible_hp in 2..=victim_max_hp.saturating_mul(2) {
                let mut observation = observation(scrap);
                add_complete_tech(&mut observation);
                observation.enemy_units.extend([
                    unit(200, 1, UnitKind::Lancer, TilePos::new(19, 20)),
                    unit(201, 1, UnitKind::Lancer, TilePos::new(20, 19)),
                ]);
                observation.enemy_units[0].hp = visible_hp.div_ceil(2);
                observation.enemy_units[1].hp = visible_hp / 2;
                let (intelligence, target) = intelligence_with_target(&mut observation, 0);
                let restrained = derive(
                    &restrained_profile,
                    &observation,
                    &intelligence,
                    &target,
                    &[],
                    5_000,
                )
                .expect("the bounded full-tech fixture remains feasible");
                let air = derive(
                    &air_profile,
                    &observation,
                    &intelligence,
                    &target,
                    &[],
                    5_000,
                )
                .expect("personality cannot change package feasibility");
                let restrained_total = capped_useful_objective(
                    &restrained_profile,
                    restrained.useful_capability,
                    restrained.useful_bombing,
                    restrained.chosen_capability,
                    restrained.chosen_bombing,
                )
                .0;
                let air_total = capped_useful_objective(
                    &air_profile,
                    air.useful_capability,
                    air.useful_bombing,
                    air.chosen_capability,
                    air.chosen_bombing,
                )
                .0;
                if restrained_total == air_total && restrained.strike != air.strike {
                    witness = Some((restrained, air, restrained_total, air_total));
                    break;
                }
            }
            if witness.is_some() {
                break;
            }
        }

        let (restrained, air, restrained_total, air_total) =
            witness.expect("the real provider economy contains a competitive mixed-air choice");
        assert_eq!(restrained.useful_capability, air.useful_capability);
        assert_eq!(restrained.useful_bombing, air.useful_bombing);
        assert_eq!(restrained_total, air_total);
        assert!(restrained.chosen_capability.strike >= restrained.minimum_capability.strike);
        assert!(air.chosen_capability.strike >= air.minimum_capability.strike);
        assert!(
            demand(&air.strike, UnitKind::Condor) > demand(&restrained.strike, UnitKind::Condor)
        );
        assert!(
            demand(&air.strike, UnitKind::Buzzard) < demand(&restrained.strike, UnitKind::Buzzard)
        );
    }

    #[test]
    fn bombing_commitment_is_monotone_with_wealth_and_visible_collateral() {
        for faction in [Faction::Ferrous, Faction::Cupric] {
            let mut moderate = observation(minimum_connected_cost(faction));
            moderate.faction = faction;
            add_complete_tech(&mut moderate);
            add_producer(
                &mut moderate,
                14,
                BuildingKind::Airworks,
                TilePos::new(14, 2),
                Vec::new(),
            );
            add_dense_ground_targets(&mut moderate);
            moderate.enemy_units.truncate(4);
            let (moderate_intelligence, moderate_target) =
                intelligence_with_target(&mut moderate, 0);
            let scarce = derive(
                &profile(70, 30),
                &moderate,
                &moderate_intelligence,
                &moderate_target,
                &[],
                5_000,
            )
            .expect("the exact common-minimum bank remains admissible");

            let mut wealthy = moderate.clone();
            wealthy.scrap = 10_000;
            let wealthy_package = derive(
                &profile(70, 30),
                &wealthy,
                &moderate_intelligence,
                &moderate_target,
                &[],
                5_000,
            )
            .expect("wealth funds useful bombing without weakening the package");
            assert!(wealthy_package.chosen_bombing >= scarce.chosen_bombing);
            assert!(
                demand(&wealthy_package.strike, Role::Bomber.unit_for(faction))
                    >= demand(&scarce.strike, Role::Bomber.unit_for(faction))
            );

            let mut dense = wealthy.clone();
            dense.enemy_units.clear();
            add_dense_ground_targets(&mut dense);
            let (dense_intelligence, dense_target) = intelligence_with_target(&mut dense, 0);
            let dense_package = derive(
                &profile(70, 30),
                &dense,
                &dense_intelligence,
                &dense_target,
                &[],
                5_000,
            )
            .expect("more visible collateral remains an actionable opportunity");
            assert!(dense_package.useful_bombing >= wealthy_package.useful_bombing);
            assert!(dense_package.chosen_bombing >= wealthy_package.chosen_bombing);
            assert!(
                demand(&dense_package.strike, Role::Bomber.unit_for(faction))
                    >= demand(&wealthy_package.strike, Role::Bomber.unit_for(faction))
            );
        }
    }

    #[test]
    fn completed_tech_does_not_spend_for_equal_capped_capability() {
        for faction in [Faction::Ferrous, Faction::Cupric] {
            let mut observation = observation(10_000);
            observation.faction = faction;
            add_complete_tech(&mut observation);
            observation.enemy_buildings.push(building(
                100,
                1,
                BuildingKind::Turret,
                TilePos::new(20, 20),
            ));
            let mut intelligence = StrategicIntelligence::default();
            intelligence.update(&observation);
            let target = intelligence
                .buildings()
                .iter()
                .find(|contact| contact.id == Some(BuildingId(100)))
                .expect("current Turret target")
                .clone();

            let package = derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                2_500,
            )
            .expect("the cheap complete package is feasible");

            assert_eq!(package.chosen_capability, package.minimum_capability);
            assert_eq!(package.useful_bombing, 0);
            assert_eq!(package.chosen_bombing, 0);
            assert_eq!(
                package.recon,
                vec![ProviderDemand {
                    kind: Role::Scout.unit_for(faction),
                    count: 1
                }]
            );
            assert_eq!(
                package.suppression,
                vec![ProviderDemand {
                    kind: UnitKind::Bombard,
                    count: 1,
                }]
            );
            assert_eq!(
                package.strike,
                vec![ProviderDemand {
                    kind: Role::AirGround.unit_for(faction),
                    count: 1,
                }]
            );
            assert_eq!(demanded_scrap(&package), minimum_connected_cost(faction));
            assert_eq!(demand(&package.suppression, UnitKind::Avalanche), 0);
            assert_eq!(demand(&package.strike, Role::Bomber.unit_for(faction)), 0);
        }
    }

    #[test]
    fn stronger_aa_cannot_hide_a_better_feasible_baseline_portfolio() {
        let profile = profile(50, 50);
        for (first_flak_hp, expected_suppression_useful) in [(150, 4_050), (177, 4_100)] {
            let (observation, intelligence, target) = baseline_aa_fixture(700, 724, first_flak_hp);
            let package = derive(&profile, &observation, &intelligence, &target, &[], 2_500)
                .expect("the fixed budget admits a complete baseline portfolio");
            assert_eq!(package.useful_capability.strike, 2_050);
            assert_eq!(
                package.useful_capability.suppression,
                expected_suppression_useful
            );
            assert_eq!(demand(&package.suppression, UnitKind::Bombard), 2);
            assert_eq!(demand(&package.strike, UnitKind::Buzzard), 2);
            assert_eq!(demanded_scrap(&package), 700);
            assert_eq!(
                package.chosen_capability,
                NormalizedCapability {
                    recon: 1_000,
                    suppression: 2_000,
                    strike: 2_000,
                }
            );
        }
    }

    #[test]
    fn baseline_portfolio_matches_an_independent_count_oracle_at_demand_boundaries() {
        let profile = profile(73, 41);
        for target_hp in [352, 353, 705, 706, 724] {
            for first_flak_hp in [149, 150, 176, 177] {
                for scrap in 380..=900 {
                    let (observation, intelligence, target) =
                        baseline_aa_fixture(scrap, target_hp, first_flak_hp);
                    let package =
                        derive(&profile, &observation, &intelligence, &target, &[], 2_500)
                            .expect("the sweep starts at the exact common-minimum budget");
                    let (suppression_count, strike_count, oracle_score) = baseline_portfolio_oracle(
                        &profile,
                        package.useful_capability,
                        scrap,
                        observation.faction,
                    );
                    assert_eq!(
                        (
                            demand(&package.suppression, UnitKind::Bombard),
                            demand(&package.strike, UnitKind::Buzzard),
                        ),
                        (suppression_count, strike_count),
                        "target hp {target_hp}, flak hp {first_flak_hp}, scrap {scrap}"
                    );
                    let chosen_score = (
                        package
                            .chosen_capability
                            .recon
                            .min(package.useful_capability.recon)
                            .saturating_add(
                                package
                                    .chosen_capability
                                    .suppression
                                    .min(package.useful_capability.suppression),
                            )
                            .saturating_add(
                                package
                                    .chosen_capability
                                    .strike
                                    .min(package.useful_capability.strike),
                            ),
                        u128::from(
                            package
                                .chosen_capability
                                .suppression
                                .min(package.useful_capability.suppression),
                        )
                        .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.siege)))
                        .saturating_add(
                            u128::from(
                                package
                                    .chosen_capability
                                    .strike
                                    .min(package.useful_capability.strike),
                            )
                            .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.air))),
                        ),
                        Reverse(demanded_scrap(&package)),
                        package
                            .chosen_capability
                            .recon
                            .saturating_add(package.chosen_capability.suppression)
                            .saturating_add(package.chosen_capability.strike),
                    );
                    assert_eq!(
                        chosen_score, oracle_score,
                        "target hp {target_hp}, flak hp {first_flak_hp}, scrap {scrap}"
                    );
                }
            }
        }
    }

    #[test]
    fn unaffordable_preferred_family_does_not_block_affordable_useful_force() {
        let mut observation = observation(500);
        add_complete_tech(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 1);
        let package = derive(
            &profile(10, 90),
            &observation,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect("the complete minimum plus one affordable marginal provider fits");

        assert_eq!(package.chosen_capability.suppression, 1_000);
        assert_eq!(package.chosen_capability.strike, 2_000);
        assert_eq!(
            package
                .provider_priority
                .iter()
                .find(|tranche| tranche.priority == ProviderPriority::Marginal)
                .map(|tranche| (tranche.family, tranche.kind)),
            Some((ForceFamily::Strike, UnitKind::Buzzard)),
            "the affordable second-choice family must remain available"
        );
    }

    #[test]
    fn completed_advanced_tech_never_weakens_the_selected_package() {
        let mut baseline = observation(1_200);
        add_baseline_tech(&mut baseline);
        add_valuable_cluster(&mut baseline);
        let (intelligence, target) = intelligence_with_target(&mut baseline, 0);
        let baseline_package = derive(
            &profile(70, 30),
            &baseline,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect("baseline production admits a connected package");

        let mut advanced = baseline.clone();
        add_producer(
            &mut advanced,
            13,
            BuildingKind::Crucible,
            TilePos::new(11, 2),
            Vec::new(),
        );
        let advanced_package = derive(
            &profile(70, 30),
            &advanced,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect("completed advanced tech cannot revoke admission");

        assert!(
            capped_useful_objective(
                &profile(70, 30),
                advanced_package.useful_capability,
                advanced_package.useful_bombing,
                advanced_package.chosen_capability,
                advanced_package.chosen_bombing,
            ) >= capped_useful_objective(
                &profile(70, 30),
                baseline_package.useful_capability,
                baseline_package.useful_bombing,
                baseline_package.chosen_capability,
                baseline_package.chosen_bombing,
            )
        );
    }

    #[test]
    fn package_selection_is_deterministic() {
        let mut observation = observation(1_537);
        add_complete_tech(&mut observation);
        add_valuable_cluster(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 1);
        let expected = derive(
            &profile(73, 41),
            &observation,
            &intelligence,
            &target,
            &[],
            2_317,
        );

        for _ in 0..32 {
            assert_eq!(
                derive(
                    &profile(73, 41),
                    &observation,
                    &intelligence,
                    &target,
                    &[],
                    2_317,
                ),
                expected
            );
        }
    }

    #[test]
    fn package_objective_is_monotone_over_bounded_wealth_horizon_and_tech_sweeps() {
        let profile = profile(73, 41);
        let mut baseline = observation(0);
        add_baseline_tech(&mut baseline);
        add_valuable_cluster(&mut baseline);
        let (intelligence, target) = intelligence_with_target(&mut baseline, 0);
        let mut advanced = baseline.clone();
        add_producer(
            &mut advanced,
            13,
            BuildingKind::Crucible,
            TilePos::new(11, 2),
            Vec::new(),
        );
        let deadlines = [500, 900, 1_300, 1_700, 2_100, 2_500];

        for template in [&baseline, &advanced] {
            for deadline in deadlines {
                let mut previous_objective = None;
                for scrap in 0..=2_500 {
                    let mut observation = template.clone();
                    observation.scrap = scrap;
                    match derive(
                        &profile,
                        &observation,
                        &intelligence,
                        &target,
                        &[],
                        deadline,
                    ) {
                        Ok(package) => {
                            let objective = capped_useful_objective(
                                &profile,
                                package.useful_capability,
                                package.useful_bombing,
                                package.chosen_capability,
                                package.chosen_bombing,
                            );
                            if let Some(previous) = previous_objective {
                                assert!(
                                    objective >= previous,
                                    "scrap {scrap} at deadline {deadline} regressed {previous:?} to {objective:?}"
                                );
                            }
                            previous_objective = Some(objective);
                        }
                        Err(rejection) => assert!(
                            previous_objective.is_none(),
                            "scrap {scrap} at deadline {deadline} revoked admission: {rejection:?}"
                        ),
                    }
                }
            }
        }

        for scrap in 0..=2_500 {
            let mut previous_objective = None;
            for deadline in deadlines {
                let mut observation = advanced.clone();
                observation.scrap = scrap;
                match derive(
                    &profile,
                    &observation,
                    &intelligence,
                    &target,
                    &[],
                    deadline,
                ) {
                    Ok(package) => {
                        let objective = capped_useful_objective(
                            &profile,
                            package.useful_capability,
                            package.useful_bombing,
                            package.chosen_capability,
                            package.chosen_bombing,
                        );
                        if let Some(previous) = previous_objective {
                            assert!(
                                objective >= previous,
                                "deadline {deadline} with scrap {scrap} regressed {previous:?} to {objective:?}"
                            );
                        }
                        previous_objective = Some(objective);
                    }
                    Err(rejection) => assert!(
                        previous_objective.is_none(),
                        "deadline {deadline} with scrap {scrap} revoked admission: {rejection:?}"
                    ),
                }
            }
        }

        for deadline in deadlines {
            for scrap in 0..=2_500 {
                let mut baseline_observation = baseline.clone();
                baseline_observation.scrap = scrap;
                let Ok(baseline_package) = derive(
                    &profile,
                    &baseline_observation,
                    &intelligence,
                    &target,
                    &[],
                    deadline,
                ) else {
                    continue;
                };
                let mut advanced_observation = advanced.clone();
                advanced_observation.scrap = scrap;
                let advanced_package = derive(
                    &profile,
                    &advanced_observation,
                    &intelligence,
                    &target,
                    &[],
                    deadline,
                )
                .unwrap_or_else(|rejection| {
                    panic!(
                        "completed tech revoked admission at scrap {scrap}, deadline {deadline}: {rejection:?}"
                    )
                });
                let baseline_objective = capped_useful_objective(
                    &profile,
                    baseline_package.useful_capability,
                    baseline_package.useful_bombing,
                    baseline_package.chosen_capability,
                    baseline_package.chosen_bombing,
                );
                let advanced_objective = capped_useful_objective(
                    &profile,
                    advanced_package.useful_capability,
                    advanced_package.useful_bombing,
                    advanced_package.chosen_capability,
                    advanced_package.chosen_bombing,
                );
                assert!(
                    advanced_objective >= baseline_objective,
                    "completed tech regressed scrap {scrap}, deadline {deadline} from {baseline_objective:?} to {advanced_objective:?}"
                );
            }
        }
    }

    #[test]
    fn bounded_wealth_sweeps_cover_personality_and_target_shape_variants() {
        for (air, siege, valuable_cluster, anti_air) in
            [(90, 10, false, 0), (50, 50, true, 0), (10, 90, true, 2)]
        {
            let profile = profile(air, siege);
            let mut template = observation(0);
            add_complete_tech(&mut template);
            if valuable_cluster {
                add_valuable_cluster(&mut template);
            }
            let (intelligence, target) = intelligence_with_target(&mut template, anti_air);

            for deadline in [500, 1_300, 2_500] {
                let mut previous_objective = None;
                for scrap in 0..=2_500 {
                    let mut observation = template.clone();
                    observation.scrap = scrap;
                    match derive(
                        &profile,
                        &observation,
                        &intelligence,
                        &target,
                        &[],
                        deadline,
                    ) {
                        Ok(package) => {
                            let objective = capped_useful_objective(
                                &profile,
                                package.useful_capability,
                                package.useful_bombing,
                                package.chosen_capability,
                                package.chosen_bombing,
                            );
                            if let Some(previous) = previous_objective {
                                assert!(
                                    objective >= previous,
                                    "profile ({air}, {siege}), target ({valuable_cluster}, {anti_air}), scrap {scrap}, deadline {deadline} regressed {previous:?} to {objective:?}"
                                );
                            }
                            previous_objective = Some(objective);
                        }
                        Err(rejection) => assert!(
                            previous_objective.is_none(),
                            "profile ({air}, {siege}), target ({valuable_cluster}, {anti_air}), scrap {scrap}, deadline {deadline} revoked admission: {rejection:?}"
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn one_lane_can_admit_and_lower_more_than_one_live_queue_of_providers() {
        let mut observation = observation(100_000);
        add_producer(
            &mut observation,
            10,
            BuildingKind::Foundry,
            TilePos::new(2, 2),
            Vec::new(),
        );
        add_producer(
            &mut observation,
            11,
            BuildingKind::Fabricator,
            TilePos::new(5, 2),
            Vec::new(),
        );
        add_producer(
            &mut observation,
            12,
            BuildingKind::Airworks,
            TilePos::new(8, 2),
            Vec::new(),
        );
        let cluster_anchors = (-4_i32..=4).flat_map(|dy| {
            (-4_i32..=4).filter_map(move |dx| {
                (dx.abs().saturating_add(dy.abs()) <= 4 && (dx, dy) != (0, 0))
                    .then_some(TilePos::new(20 + dx, 20 + dy))
            })
        });
        for (index, anchor) in cluster_anchors.take(24).enumerate() {
            observation.enemy_buildings.push(building(
                200 + u32::try_from(index).expect("small fixture"),
                1,
                BuildingKind::Foundry,
                anchor,
            ));
        }
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let deadline = 50_000;
        let resources = ResourceSnapshot::from_observation(&observation);
        let package = derive_connected_force_package(
            &profile(90, 10),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(deadline, 0),
        )
        .expect("a wealthy long-horizon operation can exceed one live queue");
        let buzzards = demand(&package.strike, UnitKind::Buzzard);
        assert!(
            buzzards > QUEUE_CAP,
            "fixture must exercise horizon demand beyond the live queue: {package:?}"
        );

        let demands: Vec<_> = package
            .provider_priority
            .iter()
            .map(|demand| ProductionDemand {
                kind: demand.kind,
                count: demand.count,
            })
            .collect();
        let first = plan_production(&resources, &demands, deadline, observation.scrap);
        let airworks_appends: Vec<_> = first
            .appends
            .iter()
            .filter(|append| append.producer == BuildingId(12))
            .collect();
        assert_eq!(airworks_appends.len(), QUEUE_CAP);
        assert_eq!(
            airworks_appends
                .iter()
                .filter(|append| append.kind == UnitKind::Buzzard)
                .count(),
            QUEUE_CAP - 1,
            "the scout owns the lane's first slot before strike production"
        );
        assert!(
            first
                .unmet
                .iter()
                .any(|demand| demand.kind == UnitKind::Buzzard && demand.count > 0)
        );

        let mut refilled = observation.clone();
        let airworks = refilled
            .my_buildings
            .iter()
            .position(|building| building.id == BuildingId(12))
            .expect("Airworks remains present");
        refilled.my_queues[airworks] = vec![UnitKind::Buzzard; QUEUE_CAP - 1];
        let resources = ResourceSnapshot::from_observation(&refilled);
        let refill = plan_production(
            &resources,
            &[ProductionDemand {
                kind: UnitKind::Buzzard,
                count: buzzards - (QUEUE_CAP - 1),
            }],
            deadline,
            refilled.scrap,
        );
        assert_eq!(refill.appends.len(), 1);
        assert_eq!(refill.appends[0].producer, BuildingId(12));
        assert_eq!(refill.appends[0].kind, UnitKind::Buzzard);
    }

    #[test]
    fn more_valuable_current_targets_never_reduce_strike_demand() {
        let mut single = observation(10_000);
        add_complete_tech(&mut single);
        let (single_intelligence, single_target) = intelligence_with_target(&mut single, 0);
        let single_package = derive(
            &profile(80, 20),
            &single,
            &single_intelligence,
            &single_target,
            &[],
            2_200,
        )
        .expect("single-target package");

        let mut cluster = observation(10_000);
        add_complete_tech(&mut cluster);
        add_valuable_cluster(&mut cluster);
        let (cluster_intelligence, cluster_target) = intelligence_with_target(&mut cluster, 0);
        let cluster_package = derive(
            &profile(80, 20),
            &cluster,
            &cluster_intelligence,
            &cluster_target,
            &[],
            2_200,
        )
        .expect("cluster package");

        assert!(cluster_package.target_value > single_package.target_value);
        assert!(cluster_package.useful_capability.strike > single_package.useful_capability.strike);
        assert!(
            realized_capability(&cluster_package, ForceFamily::Strike, cluster.faction)
                >= realized_capability(&single_package, ForceFamily::Strike, single.faction)
        );
    }

    #[test]
    fn only_current_ground_targetable_aa_adds_suppression_work() {
        let mut undefended = observation(10_000);
        add_complete_tech(&mut undefended);
        let (undefended_intelligence, undefended_target) =
            intelligence_with_target(&mut undefended, 0);
        let undefended_package = derive(
            &profile(20, 80),
            &undefended,
            &undefended_intelligence,
            &undefended_target,
            &[],
            2_500,
        )
        .expect("undefended package");

        let mut static_defense = observation(10_000);
        add_complete_tech(&mut static_defense);
        let (static_intelligence, static_target) = intelligence_with_target(&mut static_defense, 3);
        let static_package = derive(
            &profile(20, 80),
            &static_defense,
            &static_intelligence,
            &static_target,
            &[],
            2_500,
        )
        .expect("static-AA package");
        assert!(static_package.suppressible_aa_firepower > 0);
        assert!(
            static_package.useful_capability.suppression
                > undefended_package.useful_capability.suppression
        );
        assert!(
            realized_capability(
                &static_package,
                ForceFamily::Suppression,
                static_defense.faction,
            ) > realized_capability(
                &undefended_package,
                ForceFamily::Suppression,
                undefended.faction,
            )
        );

        let mut mobile_defense = observation(10_000);
        add_complete_tech(&mut mobile_defense);
        for (id, kind, tile) in [
            (200, UnitKind::Flakhound, TilePos::new(20, 19)),
            (201, UnitKind::Stinger, TilePos::new(19, 20)),
            (202, UnitKind::Sentinel, TilePos::new(21, 20)),
            (203, UnitKind::Flakhound, TilePos::new(20, 21)),
        ] {
            mobile_defense.enemy_units.push(unit(id, 1, kind, tile));
        }
        let (mobile_intelligence, mobile_target) = intelligence_with_target(&mut mobile_defense, 0);
        let mobile_package = derive(
            &profile(20, 80),
            &mobile_defense,
            &mobile_intelligence,
            &mobile_target,
            &[],
            2_500,
        )
        .expect("mobile-AA package");
        assert!(mobile_package.observed_aa_firepower > 0);
        assert_eq!(
            mobile_package.suppressible_aa_firepower,
            mobile_package.observed_aa_firepower
        );
        assert!(
            mobile_package.useful_capability.suppression
                > undefended_package.useful_capability.suppression
        );
        assert!(
            realized_capability(
                &mobile_package,
                ForceFamily::Suppression,
                mobile_defense.faction,
            ) > realized_capability(
                &undefended_package,
                ForceFamily::Suppression,
                undefended.faction,
            )
        );
        let cluster = current_target_cluster(
            &mobile_intelligence,
            mobile_target.player,
            mobile_target.anchor,
        );
        let defense = current_air_defense(&mobile_intelligence, &cluster);
        let expected_hp = [
            UnitKind::Flakhound,
            UnitKind::Stinger,
            UnitKind::Sentinel,
            UnitKind::Flakhound,
        ]
        .into_iter()
        .map(|kind| u64::from(kind.stats().max_hp))
        .sum::<u64>();
        assert_eq!(defense.suppressible_hp, expected_hp);
        assert_eq!(defense.untargetable_air_firepower, 0);

        let mut unfinished_defense = observation(10_000);
        add_complete_tech(&mut unfinished_defense);
        let mut unfinished = building(201, 1, BuildingKind::FlakTurret, TilePos::new(19, 20));
        unfinished.built = false;
        unfinished_defense.enemy_buildings.push(unfinished);
        let (unfinished_intelligence, unfinished_target) =
            intelligence_with_target(&mut unfinished_defense, 0);
        let unfinished_package = derive(
            &profile(20, 80),
            &unfinished_defense,
            &unfinished_intelligence,
            &unfinished_target,
            &[],
            2_500,
        )
        .expect("unfinished-AA package");
        assert_eq!(unfinished_package.observed_aa_firepower, 0);
        assert_eq!(unfinished_package.suppressible_aa_firepower, 0);

        let mut destroyed_defense = observation(10_000);
        add_complete_tech(&mut destroyed_defense);
        let mut destroyed = building(202, 1, BuildingKind::FlakTurret, TilePos::new(19, 20));
        destroyed.hp = 0;
        destroyed_defense.enemy_buildings.push(destroyed);
        let (destroyed_intelligence, destroyed_target) =
            intelligence_with_target(&mut destroyed_defense, 0);
        let destroyed_package = derive(
            &profile(20, 80),
            &destroyed_defense,
            &destroyed_intelligence,
            &destroyed_target,
            &[],
            2_500,
        )
        .expect("destroyed-AA package");
        assert_eq!(destroyed_package.observed_aa_firepower, 0);
        assert_eq!(destroyed_package.suppressible_aa_firepower, 0);

        let mut remembered_intelligence = static_intelligence;
        let mut after_scout_left = static_defense;
        after_scout_left.tick += 1;
        after_scout_left
            .enemy_buildings
            .retain(|building| building.kind != BuildingKind::FlakTurret);
        for x in 17..=22 {
            for y in 18..=22 {
                let index =
                    usize::try_from(y * after_scout_left.map_width + x).expect("fixture index");
                after_scout_left.visible[index] = false;
            }
        }
        remembered_intelligence.update(&after_scout_left);
        let remembered_target = remembered_intelligence
            .buildings()
            .iter()
            .find(|contact| contact.kind == BuildingKind::Crucible)
            .expect("current target")
            .clone();
        let remembered_package = derive(
            &profile(20, 80),
            &after_scout_left,
            &remembered_intelligence,
            &remembered_target,
            &[],
            2_500,
        )
        .expect("remembered-AA package");
        assert_eq!(remembered_package.observed_aa_firepower, 0);
        assert_eq!(remembered_package.suppressible_aa_firepower, 0);
    }

    #[test]
    fn air_defense_covers_the_complete_target_footprint_and_deduplicates_sources() {
        let mut observation = observation(10_000);
        add_complete_tech(&mut observation);
        let target_anchor = TilePos::new(20, 10);
        let flak_anchor = TilePos::new(26, 11);
        observation.enemy_buildings.extend([
            building(100, 1, BuildingKind::Crucible, target_anchor),
            building(101, 1, BuildingKind::FlakTurret, flak_anchor),
        ]);
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == target_anchor)
            .expect("current target");
        let cluster = vec![target];

        assert!(
            intelligence
                .air_defense_at(target_anchor)
                .sources
                .iter()
                .all(|source| !matches!(
                    source.source,
                    AirDefenseSource::Building {
                        id: Some(BuildingId(101)),
                        ..
                    }
                )),
            "the regression depends on the anchor itself falling outside flak range"
        );
        let assessment = target_cluster_air_defense(&intelligence, &cluster);
        assert!(assessment.all_target_tiles_visible);
        assert_eq!(
            assessment
                .sources
                .iter()
                .filter(|source| matches!(
                    source.source,
                    AirDefenseSource::Building {
                        id: Some(BuildingId(101)),
                        ..
                    }
                ))
                .count(),
            1,
            "one flak source may cover several footprint tiles but must be budgeted once"
        );

        let package = derive(
            &profile(20, 80),
            &observation,
            &intelligence,
            target,
            &[],
            2_500,
        )
        .expect("the visible static defense is suppressible");
        assert!(package.observed_aa_firepower > 0);
        assert_eq!(
            package.observed_aa_firepower,
            package.suppressible_aa_firepower
        );
    }

    #[test]
    fn target_cluster_counts_each_covering_air_defense_source_once() {
        let primary = TilePos::new(20, 20);
        let secondary = TilePos::new(24, 20);
        let shared_source = UnitId(200);
        let secondary_only_source = UnitId(201);
        let mut observation = observation(10_000);
        add_complete_tech(&mut observation);
        observation
            .enemy_buildings
            .push(building(150, 1, BuildingKind::Reclaimer, secondary));
        observation.enemy_units.extend([
            unit(200, 1, UnitKind::Flakhound, TilePos::new(22, 20)),
            unit(201, 1, UnitKind::Flakhound, TilePos::new(28, 20)),
        ]);
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let cluster = current_target_cluster(&intelligence, target.player, target.anchor);
        assert_eq!(cluster.len(), 2);

        let primary_coverage = intelligence.air_defense_at(primary);
        let secondary_coverage = intelligence.air_defense_at(secondary);
        let shared_primary = primary_coverage
            .sources
            .iter()
            .find(|source| {
                matches!(
                    source.source,
                    AirDefenseSource::Unit { id, .. } if id == shared_source
                )
            })
            .expect("the shared source covers the primary target");
        let shared_secondary = secondary_coverage
            .sources
            .iter()
            .find(|source| {
                matches!(
                    source.source,
                    AirDefenseSource::Unit { id, .. } if id == shared_source
                )
            })
            .expect("the shared source also covers the secondary target");
        let secondary_only = secondary_coverage
            .sources
            .iter()
            .find(|source| {
                matches!(
                    source.source,
                    AirDefenseSource::Unit { id, .. } if id == secondary_only_source
                )
            })
            .expect("the second source covers only the secondary target");
        assert!(
            primary_coverage.sources.iter().all(|source| !matches!(
                source.source,
                AirDefenseSource::Unit { id, .. } if id == secondary_only_source
            )),
            "the second source must not cover the primary target"
        );

        let defense = current_air_defense(&intelligence, &cluster);
        assert_eq!(
            defense.total_firepower,
            u64::from(
                shared_primary
                    .firepower_per_100_ticks
                    .max(shared_secondary.firepower_per_100_ticks)
            ) + u64::from(secondary_only.firepower_per_100_ticks),
            "coverage of both cluster buildings must not count one source twice"
        );
        assert_eq!(
            defense.suppressible_hp,
            u64::from(UnitKind::Flakhound.stats().max_hp) * 2
        );
    }

    #[test]
    fn parallel_advanced_and_lower_tier_lanes_form_a_stronger_mixed_package() {
        let mut observation = observation(10_000);
        add_complete_tech(&mut observation);
        add_producer(
            &mut observation,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            Vec::new(),
        );
        add_valuable_cluster(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 3);

        let package = derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            1_100,
        )
        .expect("wealthy mixed-tier package");
        assert!(demand(&package.suppression, UnitKind::Avalanche) > 0);
        assert!(demand(&package.suppression, UnitKind::Bombard) > 0);
        assert!(demand(&package.strike, UnitKind::Buzzard) > 0);
        assert!(
            package.chosen_capability.suppression
                > suppression_capability(UnitKind::Avalanche, observation.faction,),
            "the complete package benefits from both parallel suppression lanes"
        );
    }

    #[test]
    fn minimum_tranche_precedes_forecast_funded_marginal_suppression() {
        let minimum_scrap = UnitKind::Kestrel
            .stats()
            .cost
            .saturating_add(UnitKind::Avalanche.stats().cost)
            .saturating_add(UnitKind::Condor.stats().cost);
        let mut observation = observation(minimum_scrap);
        add_complete_tech(&mut observation);
        add_owned_building(
            &mut observation,
            14,
            BuildingKind::Reclaimer,
            TilePos::new(14, 2),
            true,
        );
        observation
            .my_buildings
            .last_mut()
            .expect("the refinery was appended")
            .tier = 1;
        observation.enemy_buildings.extend([
            building(100, 1, BuildingKind::Turret, TilePos::new(20, 20)),
            building(101, 1, BuildingKind::FlakTurret, TilePos::new(19, 20)),
            building(102, 1, BuildingKind::FlakTurret, TilePos::new(21, 20)),
        ]);
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.kind == BuildingKind::Turret)
            .expect("current target")
            .clone();

        let package = derive(
            &profile(10, 90),
            &observation,
            &intelligence,
            &target,
            &[],
            8_000,
        )
        .expect("forecast income can fund one useful suppression provider");

        let first_marginal = package
            .provider_priority
            .iter()
            .position(|tranche| tranche.priority == ProviderPriority::Marginal)
            .expect("the forecast funds useful marginal suppression");
        assert!(
            package.provider_priority[..first_marginal]
                .iter()
                .all(|tranche| tranche.priority == ProviderPriority::Minimum)
        );
        assert!(
            package.provider_priority[first_marginal..]
                .iter()
                .all(|tranche| tranche.priority == ProviderPriority::Marginal)
        );
        assert_eq!(
            package.provider_priority[..first_marginal]
                .iter()
                .map(|tranche| tranche.family)
                .collect::<Vec<_>>(),
            ForceFamily::ALL
        );
        assert!(
            package.provider_priority[first_marginal..]
                .iter()
                .any(|tranche| tranche.family == ForceFamily::Suppression)
        );
        assert_eq!(package.current_scrap, minimum_scrap);
        assert!(package.forecast_scrap >= UnitKind::Avalanche.stats().cost);
    }

    #[test]
    fn lowering_keeps_the_selected_airwork_portfolio_feasible() {
        let mut observation = observation(940);
        add_complete_tech(&mut observation);
        add_producer(
            &mut observation,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            Vec::new(),
        );
        for (building, queue) in observation
            .my_buildings
            .iter()
            .zip(&mut observation.my_queues)
        {
            if building.kind == BuildingKind::Airworks {
                *queue = vec![UnitKind::Skyhook; 4];
            }
        }
        observation.my_units.push(unit(
            40,
            observation.me.0,
            UnitKind::Kestrel,
            TilePos::new(7, 7),
        ));
        observation.my_units.push(unit(
            41,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(8, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let deadline = 2_500;
        let resources = ResourceSnapshot::from_observation(&observation);

        let package = derive_connected_force_package(
            &profile(90, 10),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(deadline, 0),
        )
        .expect("the chosen strike portfolio fits the fixed window");

        let demands: Vec<_> = package
            .strike
            .iter()
            .map(|demand| ProductionDemand {
                kind: demand.kind,
                count: demand.count,
            })
            .collect();
        let schedule = plan_production(&resources, &demands, deadline, observation.scrap);

        assert!(schedule.unmet.is_empty());
        assert_eq!(
            schedule.appends.len(),
            package.strike.iter().map(|d| d.count).sum::<usize>()
        );
        for demand in &package.strike {
            assert_eq!(
                schedule
                    .appends
                    .iter()
                    .filter(|append| append.kind == demand.kind)
                    .count(),
                demand.count
            );
        }
        assert!(
            schedule
                .appends
                .iter()
                .all(|append| append.timing.no_block_latest_ready_tick < deadline)
        );
    }

    #[test]
    fn derived_minimum_and_lowering_share_the_same_constrained_lane_assignment() {
        let mut observation = observation(
            UnitKind::Kestrel
                .stats()
                .cost
                .saturating_add(UnitKind::Buzzard.stats().cost),
        );
        add_complete_tech(&mut observation);
        add_producer(
            &mut observation,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            vec![UnitKind::Talon],
        );
        observation.my_queue_progress = vec![0; observation.my_buildings.len()];
        let busy_airworks = observation
            .my_buildings
            .iter()
            .position(|building| building.id == BuildingId(14))
            .expect("the second Airworks is present");
        observation.my_queue_progress[busy_airworks] = 30;
        observation.my_units.push(unit(
            40,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(7, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let deadline = 370;
        let resources = ResourceSnapshot::from_observation(&observation);

        let package = derive_connected_force_package(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(deadline, 0),
        )
        .expect("the minimum fits only when the short scout uses the busy lane");
        let demands: Vec<_> = package
            .provider_priority
            .iter()
            .filter(|demand| demand.family != ForceFamily::Suppression)
            .map(|demand| ProductionDemand {
                kind: demand.kind,
                count: demand.count,
            })
            .collect();

        assert_eq!(
            demands,
            vec![
                ProductionDemand {
                    kind: UnitKind::Kestrel,
                    count: 1,
                },
                ProductionDemand {
                    kind: UnitKind::Buzzard,
                    count: 1,
                },
            ]
        );
        let schedule = plan_production(&resources, &demands, deadline, observation.scrap);

        assert!(schedule.unmet.is_empty());
        assert_eq!(
            schedule
                .appends
                .iter()
                .map(|append| (append.kind, append.producer))
                .collect::<Vec<_>>(),
            vec![
                (UnitKind::Kestrel, BuildingId(14)),
                (UnitKind::Buzzard, BuildingId(12)),
            ]
        );
        assert!(
            schedule
                .appends
                .iter()
                .all(|append| append.timing.no_block_latest_ready_tick < deadline)
        );
    }

    #[test]
    fn existing_and_paid_lower_tiers_are_not_replaced_by_tier_three() {
        let mut observation = observation(2_000);
        add_complete_tech(&mut observation);
        let airworks = observation
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Airworks)
            .expect("airworks");
        observation.my_queues[airworks].push(UnitKind::Buzzard);
        observation.my_units.push(unit(
            40,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(7, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut observation, 1);
        let resources = ResourceSnapshot::from_observation(&observation);

        let package = derive_connected_force_package(
            &profile(80, 80),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(2_500, 0),
        )
        .expect("existing providers plus completed production can assemble");
        assert!(demand(&package.suppression, UnitKind::Bombard) >= 1);
        assert!(demand(&package.strike, UnitKind::Buzzard) >= 1);
        assert_eq!(
            package
                .provider_priority
                .iter()
                .filter(|demand| demand.priority == ProviderPriority::Minimum)
                .map(|demand| (demand.family, demand.kind, demand.count))
                .collect::<Vec<_>>(),
            [
                (ForceFamily::Recon, UnitKind::Kestrel, 1),
                (ForceFamily::Suppression, UnitKind::Bombard, 1),
                (ForceFamily::Strike, UnitKind::Buzzard, 1),
            ],
            "live and already-paid lower-tier providers retain minimum priority"
        );
    }

    #[test]
    fn paid_queue_work_must_spawn_before_the_deadline_observation() {
        let mut observation = observation(0);
        add_complete_tech(&mut observation);
        let airworks = observation
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Airworks)
            .expect("airworks");
        observation.my_queues[airworks].push(UnitKind::Buzzard);
        observation.my_units.push(unit(
            40,
            observation.me.0,
            Role::Scout.unit_for(observation.faction),
            TilePos::new(7, 7),
        ));
        observation.my_units.push(unit(
            41,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(8, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);
        let ready_tick = observation.tick + Tick::from(UnitKind::Buzzard.stats().train_ticks) - 1;

        assert!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                ready_tick - 1,
            )
            .is_err()
        );
        assert!(
            derive(
                &profile(50, 50),
                &observation,
                &intelligence,
                &target,
                &[],
                ready_tick,
            )
            .is_err(),
            "the aircraft spawns after the deadline decision has already run"
        );
        let package = derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            ready_tick + 1,
        )
        .expect("the paid aircraft exists in the next tick's observation");
        assert_eq!(demand(&package.strike, UnitKind::Buzzard), 1);
    }

    #[test]
    fn blocked_ground_egress_does_not_promise_a_paid_provider() {
        let mut open = observation(0);
        add_complete_tech(&mut open);
        let fabricator = open
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Fabricator)
            .expect("fabricator");
        open.my_queues[fabricator].push(UnitKind::Bombard);
        open.my_units.push(unit(
            40,
            open.me.0,
            Role::Scout.unit_for(open.faction),
            TilePos::new(7, 7),
        ));
        open.my_units.push(unit(
            41,
            open.me.0,
            Role::AirGround.unit_for(open.faction),
            TilePos::new(8, 7),
        ));
        let (intelligence, target) = intelligence_with_target(&mut open, 0);
        let ready_tick = open.tick + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1;
        assert!(
            derive(
                &profile(50, 50),
                &open,
                &intelligence,
                &target,
                &[],
                ready_tick + 1,
            )
            .is_ok(),
            "an open paid Bombard is credible"
        );

        let mut blocked = open.clone();
        let producer = blocked.my_buildings[fabricator].clone();
        blocked.known_scrap.extend(
            crate::tick::rect_adjacent_tiles(
                producer.anchor,
                producer.kind.tier_stats(producer.tier).size,
            )
            .map(|tile| (tile, 1)),
        );
        blocked
            .known_scrap
            .sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
        assert!(
            derive(
                &profile(50, 50),
                &blocked,
                &intelligence,
                &target,
                &[],
                ready_tick + 1,
            )
            .is_err(),
            "paid ground work with no observed spawn tile has no finite ready promise"
        );
    }

    #[test]
    fn equivalent_portfolios_receive_canonical_personality_order() {
        let mut observation = observation(2_160);
        add_complete_tech(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 2);
        let resources = ResourceSnapshot::from_observation(&observation);

        let air = derive_connected_force_package(
            &profile(90, 10),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(2_500, 0),
        )
        .expect("air package");
        let siege = derive_connected_force_package(
            &profile(10, 90),
            &observation,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(2_500, 0),
        )
        .expect("siege package");

        assert_eq!(air.minimum_capability, siege.minimum_capability);
        assert_eq!(air.recon, siege.recon);
        assert_eq!(air.suppression, siege.suppression);
        assert_eq!(air.strike, siege.strike);
        assert_ne!(air.provider_priority, siege.provider_priority);
        assert_eq!(
            air.provider_priority
                .iter()
                .find(|demand| demand.priority == ProviderPriority::Marginal)
                .map(|demand| demand.family),
            Some(ForceFamily::Strike)
        );
        assert_eq!(
            siege
                .provider_priority
                .iter()
                .find(|demand| demand.priority == ProviderPriority::Marginal)
                .map(|demand| demand.family),
            Some(ForceFamily::Suppression)
        );
    }

    #[test]
    fn every_difficulty_stance_and_personality_retains_the_minimum_repertoire() {
        let exact_cost = minimum_connected_cost(Faction::Ferrous);
        let mut observation = observation(exact_cost);
        add_complete_tech(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);

        for difficulty in BotDifficulty::ALL {
            for stance in BotStance::ALL {
                for seed in [0, 1, u64::MAX] {
                    let profile =
                        ResolvedProfile::resolve(BotConfig::scripted(difficulty, stance, seed));
                    let package = derive(&profile, &observation, &intelligence, &target, &[], 500)
                        .unwrap_or_else(|_rejection| {
                            panic!("{difficulty}/{stance}/{seed} lost a provider family")
                        });
                    assert!(!package.recon.is_empty());
                    assert!(!package.suppression.is_empty());
                    assert!(!package.strike.is_empty());
                    assert!(
                        realized_capability(&package, ForceFamily::Recon, observation.faction)
                            >= package.minimum_capability.recon
                    );
                    assert!(
                        realized_capability(
                            &package,
                            ForceFamily::Suppression,
                            observation.faction,
                        ) >= package.minimum_capability.suppression
                    );
                    assert!(
                        realized_capability(&package, ForceFamily::Strike, observation.faction)
                            >= package.minimum_capability.strike
                    );
                }
            }
        }
    }

    fn rich_parallel_suppression_fixture(staggered: bool) -> Observation {
        let mut observation = observation(1_000_000);
        add_producer(
            &mut observation,
            10,
            BuildingKind::Foundry,
            TilePos::new(2, 2),
            Vec::new(),
        );
        for index in 0..12_u32 {
            add_producer(
                &mut observation,
                20 + index,
                BuildingKind::Fabricator,
                TilePos::new(2 + 3 * (index % 8) as i32, 5 + 3 * (index / 8) as i32),
                staggered
                    .then_some(vec![UnitKind::Lancer])
                    .unwrap_or_default(),
            );
        }
        add_producer(
            &mut observation,
            40,
            BuildingKind::Airworks,
            TilePos::new(2, 14),
            Vec::new(),
        );
        if staggered {
            observation.my_queue_progress = vec![0; observation.my_buildings.len()];
            for (building, progress) in observation
                .my_buildings
                .iter()
                .zip(&mut observation.my_queue_progress)
            {
                if building.kind == BuildingKind::Fabricator {
                    *progress = (building.id.0 - 20) * 10;
                }
            }
        }
        observation.enemy_buildings.push(building(
            200,
            1,
            BuildingKind::Turret,
            TilePos::new(20, 20),
        ));
        let anti_air_tiles = [
            TilePos::new(20, 15),
            TilePos::new(17, 17),
            TilePos::new(18, 17),
            TilePos::new(19, 17),
            TilePos::new(20, 17),
            TilePos::new(21, 17),
            TilePos::new(22, 17),
            TilePos::new(23, 17),
            TilePos::new(17, 18),
            TilePos::new(18, 18),
            TilePos::new(19, 18),
            TilePos::new(20, 18),
        ];
        for (index, tile) in anti_air_tiles.into_iter().enumerate() {
            observation.enemy_units.push(unit(
                300 + u32::try_from(index).expect("small fixture"),
                1,
                UnitKind::Flakhound,
                tile,
            ));
        }
        observation
    }

    fn derive_rich_parallel_suppression(staggered: bool) -> ConnectedForcePackage {
        let observation = rich_parallel_suppression_fixture(staggered);
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.id == Some(BuildingId(200)))
            .expect("current target")
            .clone();
        derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect("the rich late-game package fits its fixed deadline")
    }

    #[test]
    fn equivalent_late_game_lanes_do_not_create_a_permutation_search() {
        let first = derive_rich_parallel_suppression(false);
        let second = derive_rich_parallel_suppression(false);

        assert_eq!(first, second);
        assert_eq!(demand(&first.suppression, UnitKind::Bombard), 12);
        assert!(first.chosen_capability.suppression >= first.useful_capability.suppression);
    }

    #[test]
    fn staggered_late_game_lanes_do_not_create_an_assignment_search() {
        let first = derive_rich_parallel_suppression(true);
        let second = derive_rich_parallel_suppression(true);

        assert_eq!(first, second);
        assert_eq!(demand(&first.suppression, UnitKind::Bombard), 12);
        assert!(first.chosen_capability.suppression >= first.useful_capability.suppression);
    }

    #[test]
    fn selected_funding_order_matches_canonical_lowering_priority() {
        let mut observation = observation(700);
        add_producer(
            &mut observation,
            10,
            BuildingKind::Foundry,
            TilePos::new(2, 2),
            Vec::new(),
        );
        add_producer(
            &mut observation,
            12,
            BuildingKind::Airworks,
            TilePos::new(8, 2),
            Vec::new(),
        );
        add_producer(
            &mut observation,
            13,
            BuildingKind::Crucible,
            TilePos::new(11, 2),
            Vec::new(),
        );
        let airworks = observation
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Airworks)
            .expect("airworks");
        observation.my_queues[airworks] =
            vec![UnitKind::Skyhook, UnitKind::Skyhook, UnitKind::Talon];
        add_owned_building(
            &mut observation,
            20,
            BuildingKind::Reclaimer,
            TilePos::new(14, 2),
            true,
        );
        observation
            .my_buildings
            .last_mut()
            .expect("the refinery was appended")
            .tier = 1;
        observation.my_units.extend([
            unit(40, observation.me.0, UnitKind::Kestrel, TilePos::new(7, 7)),
            unit(41, observation.me.0, UnitKind::Bombard, TilePos::new(8, 7)),
            unit(42, observation.me.0, UnitKind::Buzzard, TilePos::new(9, 7)),
        ]);
        observation.enemy_buildings.push(building(
            100,
            1,
            BuildingKind::Fabricator,
            TilePos::new(20, 20),
        ));
        observation.enemy_units.extend([
            unit(101, 1, UnitKind::Flakhound, TilePos::new(20, 15)),
            unit(102, 1, UnitKind::Flakhound, TilePos::new(19, 17)),
        ]);
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.id == Some(BuildingId(100)))
            .expect("current target")
            .clone();

        let package = derive(
            &profile(90, 10),
            &observation,
            &intelligence,
            &target,
            &[],
            1_476,
        )
        .expect("a canonically fundable package exists");

        assert!(package.provider_priority.windows(2).all(|pair| {
            provider_priority_rank(&profile(90, 10), observation.faction, pair[0])
                <= provider_priority_rank(&profile(90, 10), observation.faction, pair[1])
        }));
        assert!(
            package.provider_priority.contains(&ProviderDemandTranche {
                priority: ProviderPriority::Minimum,
                family: ForceFamily::Suppression,
                kind: UnitKind::Avalanche,
                count: 1,
            }),
            "priority={:?}",
            package.provider_priority
        );
        assert!(package.provider_priority.contains(&ProviderDemandTranche {
            priority: ProviderPriority::Marginal,
            family: ForceFamily::Strike,
            kind: UnitKind::Buzzard,
            count: 1,
        }));
        assert!(!package.provider_priority.iter().any(|tranche| {
            tranche.priority == ProviderPriority::Marginal && tranche.kind == UnitKind::Avalanche
        }));

        let mut fully_funded = observation.clone();
        fully_funded.scrap = 820;
        let resources = ResourceSnapshot::from_observation(&fully_funded);
        let fully_funded_package = derive_connected_force_package(
            &profile(90, 10),
            &fully_funded,
            &intelligence,
            &target,
            ProductionEvidence::new(&resources, &ProductionAccess::Unrestricted),
            &[],
            constraints(1_476, 0),
        )
        .expect("the neighboring canonical portfolio is fully funded now");
        assert!(
            fully_funded_package
                .provider_priority
                .contains(&ProviderDemandTranche {
                    priority: ProviderPriority::Marginal,
                    family: ForceFamily::Suppression,
                    kind: UnitKind::Avalanche,
                    count: 1,
                })
        );
    }

    fn brute_funded_lane_schedule_fits(
        lanes: &[FundedLane],
        providers: &[FundedProvider],
        deadline: Tick,
    ) -> bool {
        fn assign(
            lanes: &[FundedLane],
            providers: &[FundedProvider],
            deadline: Tick,
            provider_index: usize,
            available_ticks: &mut [Tick],
        ) -> bool {
            let Some(provider) = providers.get(provider_index).copied() else {
                return true;
            };
            let duration = Tick::from(provider.kind.stats().train_ticks);
            for lane_index in 0..lanes.len() {
                if lanes[lane_index]
                    .eligible_kinds
                    .binary_search(&provider.kind)
                    .is_err()
                {
                    continue;
                }
                let previous = available_ticks[lane_index];
                let Some(next) = previous.max(provider.command_tick).checked_add(duration) else {
                    continue;
                };
                if next > deadline {
                    continue;
                }
                available_ticks[lane_index] = next;
                if assign(
                    lanes,
                    providers,
                    deadline,
                    provider_index + 1,
                    available_ticks,
                ) {
                    return true;
                }
                available_ticks[lane_index] = previous;
            }
            false
        }

        let mut available_ticks: Vec<_> = lanes.iter().map(|lane| lane.available_tick).collect();
        assign(lanes, providers, deadline, 0, &mut available_ticks)
    }

    #[test]
    fn funded_lane_compression_matches_a_concrete_small_oracle() {
        let kinds = [UnitKind::Kestrel, UnitKind::Buzzard];
        let lane_variants = [
            (100, vec![kinds[0]]),
            (160, vec![kinds[0]]),
            (100, vec![kinds[1]]),
            (160, vec![kinds[1]]),
            (100, kinds.to_vec()),
            (160, kinds.to_vec()),
        ];
        let deadline = 520;
        for lane_count in 1..=3_u32 {
            let topology_count = lane_variants.len().pow(lane_count);
            for mut topology in 0..topology_count {
                let mut lanes = Vec::new();
                for _ in 0..lane_count {
                    let variant = topology % lane_variants.len();
                    topology /= lane_variants.len();
                    let (available_tick, eligible_kinds) = &lane_variants[variant];
                    lanes.push(FundedLane {
                        eligible_kinds: eligible_kinds.clone(),
                        available_tick: *available_tick,
                    });
                }
                for provider_count in 0..=4_usize {
                    for kind_bits in 0..(1_usize << provider_count) {
                        for late_count in 0..=provider_count {
                            let providers: Vec<_> = (0..provider_count)
                                .map(|index| FundedProvider {
                                    kind: kinds[(kind_bits >> index) & 1],
                                    command_tick: if index < provider_count - late_count {
                                        100
                                    } else {
                                        220
                                    },
                                })
                                .collect();
                            let expected =
                                brute_funded_lane_schedule_fits(&lanes, &providers, deadline);
                            let actual =
                                funded_lane_schedule_fits(lanes.clone(), &providers, deadline);
                            assert_eq!(
                                actual, expected,
                                "lanes={lanes:?}, providers={providers:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
