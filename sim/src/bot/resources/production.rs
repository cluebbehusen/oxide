//! Necessary capacity bounds and paid production inventory.
//!
//! This module checks the requested roster and paid inventory against producer
//! capacity. Shared allocation owns funded schedules and command lowering.

use std::collections::BTreeMap;

use super::{ProducerEgress, ResourceSnapshot};
use crate::ids::BuildingId;
use crate::stats::{Domain, UnitKind};
use chassis::Tick;

/// Producer-to-objective reachability supplied by an owning strategy.
///
/// The resource layer proves local queue and egress timing. A strategy whose
/// units must join a particular operation can additionally restrict exact
/// producer and unit-kind pairs to those with a usable tactical route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProductionAccess {
    allowed: Vec<(BuildingId, UnitKind)>,
    paid_allowed: Vec<(BuildingId, UnitKind)>,
    paid_exclusions: Vec<(BuildingId, UnitKind, usize)>,
}

impl ProductionAccess {
    #[cfg(test)]
    pub(crate) fn restricted_kinds(allowed: Vec<(BuildingId, UnitKind)>) -> Self {
        Self::restricted_kinds_with_paid(allowed.clone(), allowed)
    }

    /// Separately restricts new appends and already-paid queue work.
    pub(crate) fn restricted_kinds_with_paid(
        mut allowed: Vec<(BuildingId, UnitKind)>,
        mut paid_allowed: Vec<(BuildingId, UnitKind)>,
    ) -> Self {
        allowed.sort_unstable();
        allowed.dedup();
        paid_allowed.sort_unstable();
        paid_allowed.dedup();
        Self {
            allowed,
            paid_allowed,
            paid_exclusions: Vec::new(),
        }
    }

    pub(crate) fn excluding_paid(mut self, excluded: &[(BuildingId, UnitKind, usize)]) -> Self {
        self.paid_exclusions.extend_from_slice(excluded);
        self.paid_exclusions.sort_unstable();
        self.paid_exclusions.dedup();
        self
    }

    fn allows_paid_occurrence(
        &self,
        producer: BuildingId,
        kind: UnitKind,
        occurrence: usize,
    ) -> bool {
        self.paid_exclusions
            .binary_search(&(producer, kind, occurrence))
            .is_err()
    }

    pub(crate) fn allows(&self, producer: BuildingId, kind: UnitKind) -> bool {
        self.allowed.binary_search(&(producer, kind)).is_ok()
    }

    /// The already-paid queue occurrences this view refuses, so a plan
    /// derived against it can pass the same refusals to any sub-derivation
    /// that rebuilds an access view of its own.
    pub(crate) fn paid_exclusions(&self) -> &[(BuildingId, UnitKind, usize)] {
        &self.paid_exclusions
    }

    fn allows_paid(&self, producer: BuildingId, kind: UnitKind) -> bool {
        self.paid_allowed.binary_search(&(producer, kind)).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LaneCapacityClass {
    eligible_kinds: Vec<UnitKind>,
    remaining_capacities: Vec<Tick>,
}

/// Immutable lane eligibility plus the available production time before a
/// shared deadline.
///
/// Concrete lane ids are intentionally kept outside the feasibility state.
/// Lanes with the same eligibility signature and remaining capacity are
/// interchangeable for feasibility. Shared allocation separately chooses exact
/// lanes and purchase times.
#[derive(Debug)]
struct HorizonProblem {
    kinds: Vec<UnitKind>,
    lane_eligibility: Vec<Vec<UnitKind>>,
    initial_capacities: Vec<Tick>,
}

impl HorizonProblem {
    fn new(
        resources: &ResourceSnapshot,
        requested: &[UnitKind],
        deadline: Tick,
        access: &ProductionAccess,
    ) -> Self {
        let mut kinds = requested.to_vec();
        kinds.sort_unstable();
        kinds.dedup();

        let mut lane_eligibility = Vec::with_capacity(resources.producers().len());
        let mut initial_capacities = Vec::with_capacity(resources.producers().len());
        for lane in resources.producers() {
            let mut eligible_kinds = Vec::new();
            let mut lane_capacity = None;
            for &kind in &kinds {
                let Some(capacity) = lane_horizon_capacity(lane, kind, deadline, access) else {
                    continue;
                };
                debug_assert!(lane_capacity.is_none_or(|existing| existing == capacity));
                lane_capacity = Some(capacity);
                eligible_kinds.push(kind);
            }
            lane_eligibility.push(eligible_kinds);
            initial_capacities.push(lane_capacity.unwrap_or(0));
        }

        Self {
            kinds,
            lane_eligibility,
            initial_capacities,
        }
    }

    fn request_counts(&self, requested: &[UnitKind]) -> Vec<usize> {
        let mut counts = vec![0_usize; self.kinds.len()];
        for kind in requested {
            let kind_index = self
                .kinds
                .binary_search(kind)
                .expect("the problem kind list contains every request");
            counts[kind_index] = counts[kind_index].saturating_add(1);
        }
        counts
    }

    fn canonical_lane_classes(&self, capacities: &[Tick]) -> Vec<LaneCapacityClass> {
        let mut by_eligibility = BTreeMap::<Vec<UnitKind>, Vec<Tick>>::new();
        for (eligible_kinds, &capacity) in self.lane_eligibility.iter().zip(capacities) {
            if eligible_kinds.is_empty() {
                continue;
            }
            by_eligibility
                .entry(eligible_kinds.clone())
                .or_default()
                .push(capacity);
        }
        by_eligibility
            .into_iter()
            .map(|(eligible_kinds, mut remaining_capacities)| {
                remaining_capacities.sort_unstable();
                LaneCapacityClass {
                    eligible_kinds,
                    remaining_capacities,
                }
            })
            .collect()
    }
}

/// Necessary throughput bounds for ranking speculative rosters. A positive
/// result still requires a funded FIFO schedule before admission.
pub(crate) fn production_may_fit_horizon(
    resources: &ResourceSnapshot,
    requested: &[UnitKind],
    deadline: Tick,
    access: &ProductionAccess,
) -> bool {
    let problem = HorizonProblem::new(resources, requested, deadline, access);
    remaining_work_fits_canonical_capacity(
        &problem.kinds,
        &problem.request_counts(requested),
        &problem.canonical_lane_classes(&problem.initial_capacities),
    )
}

/// Necessary aggregate and per-kind throughput checks for a canonical state.
fn remaining_work_fits_canonical_capacity(
    kinds: &[UnitKind],
    remaining_counts: &[usize],
    lane_classes: &[LaneCapacityClass],
) -> bool {
    let requested_ticks = kinds
        .iter()
        .zip(remaining_counts)
        .map(|(kind, &count)| u128::from(kind.stats().train_ticks) * count as u128)
        .sum::<u128>();
    let available_ticks = lane_classes
        .iter()
        .flat_map(|class| &class.remaining_capacities)
        .map(|&capacity| u128::from(capacity))
        .sum::<u128>();
    if requested_ticks > available_ticks {
        return false;
    }

    // Work assigned to one lane is a sum of its eligible train durations and
    // therefore a multiple of their GCD. Capacity below the next such multiple
    // is unusable even when the raw aggregate has enough ticks.
    let modular_available_ticks = lane_classes
        .iter()
        .map(|class| {
            let divisor = class
                .eligible_kinds
                .iter()
                .filter_map(|kind| {
                    let kind_index = kinds
                        .binary_search(kind)
                        .expect("lane eligibility contains only problem kinds");
                    (remaining_counts[kind_index] > 0)
                        .then_some(Tick::from(kind.stats().train_ticks))
                })
                .reduce(greatest_common_divisor);
            class
                .remaining_capacities
                .iter()
                .map(|&capacity| divisor.map_or(0, |divisor| capacity - capacity % divisor))
                .map(u128::from)
                .sum::<u128>()
        })
        .sum::<u128>();
    if requested_ticks > modular_available_ticks {
        return false;
    }

    kinds.iter().zip(remaining_counts).all(|(&kind, &count)| {
        count == 0
            || count as u128
                <= available_slots_for_kind(
                    kind,
                    Tick::from(kind.stats().train_ticks),
                    lane_classes,
                )
    })
}

fn greatest_common_divisor(mut left: Tick, mut right: Tick) -> Tick {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn available_slots_for_kind(
    kind: UnitKind,
    duration: Tick,
    lane_classes: &[LaneCapacityClass],
) -> u128 {
    lane_classes
        .iter()
        .filter(|class| class.eligible_kinds.binary_search(&kind).is_ok())
        .flat_map(|class| &class.remaining_capacities)
        .map(|&capacity| u128::from(capacity / duration))
        .sum()
}

fn lane_horizon_capacity(
    lane: &super::ProducerLane,
    kind: UnitKind,
    deadline: Tick,
    access: &ProductionAccess,
) -> Option<Tick> {
    if !access.allows(lane.producer, kind) {
        return None;
    }
    let timing = lane.horizon_timing(&[kind])?;
    if timing.no_block_latest_ready_tick >= deadline
        || !egress_is_credible_for(kind, timing.current_egress)
    {
        return None;
    }
    deadline
        .checked_sub(1)?
        .checked_sub(timing.no_block_latest_ready_tick)?
        .checked_add(Tick::from(kind.stats().train_ticks))
}

/// Counts exact already-paid queue items that can credibly finish before
/// `deadline` and therefore exist in that tick's observation.
///
/// Exact owner-visible front progress is used when the observation rows align;
/// otherwise the bound conservatively assumes zero progress. Unknown or blocked
/// ground egress earns no credit. The current bank and forecast are irrelevant
/// because every counted item has already been purchased.
pub(crate) fn count_paid_queued_ready_with_access(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    deadline: Tick,
    access: &ProductionAccess,
) -> usize {
    paid_queued_ready_producers_with_access(resources, kind, deadline, access).len()
}

/// Exact producers whose already-paid queue contributes `kind` before an
/// operation deadline.
///
/// Repeated producer ids preserve queue multiplicity. Producers follow the
/// snapshot's canonical id order, and matching occurrences retain queue order.
pub(crate) fn paid_queued_ready_producers_with_access(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    deadline: Tick,
    access: &ProductionAccess,
) -> Vec<BuildingId> {
    paid_queued_ready_occurrences_with_access(resources, kind, deadline, access)
        .into_iter()
        .map(|(producer, _)| producer)
        .collect()
}

pub(crate) fn paid_queued_ready_occurrences_with_access(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    deadline: Tick,
    access: &ProductionAccess,
) -> Vec<(BuildingId, usize)> {
    resources
        .producers()
        .iter()
        .flat_map(|lane| {
            if !access.allows_paid(lane.producer, kind)
                || !egress_is_credible_for(kind, lane.ground_egress)
            {
                return Vec::new();
            }

            let mut preceding_ticks = 0_u64;
            let mut producers = Vec::new();
            let mut occurrence = 0;
            for (index, queued) in lane.queued.iter().enumerate() {
                let train_ticks = queued.stats().train_ticks;
                let item_ticks = if index == 0 {
                    lane.front_progress.map_or(train_ticks, |progress| {
                        train_ticks.saturating_sub(progress).max(1)
                    })
                } else {
                    train_ticks
                };
                let Some(next_preceding_ticks) =
                    preceding_ticks.checked_add(Tick::from(item_ticks))
                else {
                    break;
                };
                preceding_ticks = next_preceding_ticks;
                let Some(ready_tick) = preceding_ticks
                    .checked_sub(1)
                    .and_then(|ticks| lane.observed_at.checked_add(ticks))
                else {
                    break;
                };
                if ready_tick >= deadline {
                    break;
                }
                if *queued == kind {
                    if access.allows_paid_occurrence(lane.producer, kind, occurrence) {
                        producers.push((lane.producer, occurrence));
                    }
                    occurrence += 1;
                }
            }
            producers
        })
        .collect()
}

fn egress_is_credible_for(kind: UnitKind, egress: ProducerEgress) -> bool {
    kind.stats().domain != Domain::Ground
        || matches!(egress, ProducerEgress::NotRequired | ProducerEgress::Open)
}

#[cfg(test)]
mod tests;
