//! Test-only funded-horizon oracle and package derivation wrappers.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct FundedLane {
    pub(super) eligible_kinds: Vec<UnitKind>,
    pub(super) available_tick: Tick,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FundedLaneClass {
    eligible_kinds: Vec<UnitKind>,
    available_ticks: Vec<Tick>,
}

pub(super) fn funded_providers_fit(
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

pub(super) fn provider_demands_fit_funded_horizon(
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

pub(super) fn funded_lane_schedule_fits(
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

/// Decision evidence for package sizing, which reads only the profile,
/// observation, and intelligence.
pub(super) fn package_evidence<'a>(
    profile: &'a ResolvedProfile,
    observation: &'a Observation,
    intelligence: &'a StrategicIntelligence,
) -> AirEvidence<'a> {
    let home = TilePos::new(0, 0);
    AirEvidence {
        profile,
        tuning: crate::difficulty::DifficultyTuning::for_level(profile.difficulty),
        obs: observation,
        intel: intelligence,
        home,
        public_map: None,
        orientation: crate::orient::Orientation::for_home(observation, home),
    }
}

/// The largest package against `target`'s current cluster.
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
    derive_connected_force_package_options_for_cluster(
        package_evidence(profile, observation, intelligence),
        ConnectedTargetEvidence {
            primary: target,
            cluster: &cluster,
            committed: None,
        },
        production,
        unavailable,
        constraints,
        false,
    )
    .map(ConnectedForcePackageOptions::into_largest)
}

impl ConnectedForcePackageOptions {
    pub(in crate::strategy) fn into_largest(self) -> ConnectedForcePackage {
        self.marginal.into_iter().last().unwrap_or(self.minimum)
    }
}
