//! Deterministic scheduling for exact production demand.
//!
//! This module chooses only among the unit kinds explicitly requested by its
//! caller. It does not select capability substitutes, consume forecast income,
//! or mutate the resource snapshot.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use super::{ProducerEgress, ProductionTiming, ResourceSnapshot};
use crate::ids::BuildingId;
use crate::stats::{Domain, UnitKind};
use chassis::Tick;

/// A request for an exact number of one concrete unit kind.
///
/// The containing slice is ordered from highest to lowest priority. Repeated
/// kinds remain separate priority tranches rather than being combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductionDemand {
    /// Exact unit kind requested by the owning strategy.
    pub(crate) kind: UnitKind,
    /// Number of new queue appends requested.
    pub(crate) count: usize,
}

/// One exact producer append selected by [`plan_production_with_access`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlannedAppend {
    /// Completed producer that receives the append.
    pub(crate) producer: BuildingId,
    /// Exact unit kind appended to the producer.
    pub(crate) kind: UnitKind,
    /// Honest readiness and egress evidence after earlier planned appends.
    pub(crate) timing: ProductionTiming,
}

/// Deterministic result of scheduling exact production demands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProductionSchedule {
    /// New queue appends in deterministic selection order.
    pub(crate) appends: Vec<PlannedAppend>,
    /// Current scrap consumed by `appends`.
    pub(crate) spent: u32,
    /// Current scrap protected for higher-priority appends that fit the fixed
    /// horizon but are waiting for a live queue slot.
    pub(crate) deferred_scrap: u32,
    /// Full cost of the next selected append when current budget stopped work.
    pub(crate) next_unfunded_cost: Option<u32>,
}

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
        match self {
            #[cfg(test)]
            Self::Unrestricted => &[],
            Self::RestrictedKinds {
                paid_exclusions, ..
            } => paid_exclusions,
        }
    }

    fn allows_paid(&self, producer: BuildingId, kind: UnitKind) -> bool {
        self.paid_allowed.binary_search(&(producer, kind)).is_ok()
    }
}

/// One member of a complete fixed-horizon lane assignment.
///
/// Keeping this separate from [`PlannedAppend`] matters: an assignment may
/// reserve a future queue position that cannot be appended on the current
/// command tick.
#[derive(Debug, Clone, Copy)]
struct HorizonAssignment {
    lane_index: usize,
    timing: ProductionTiming,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LaneCapacityClass {
    eligible_kinds: Vec<UnitKind>,
    remaining_capacities: Vec<Tick>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AssignmentSearchKey {
    remaining_counts: Vec<usize>,
    lane_classes: Vec<LaneCapacityClass>,
}

#[derive(Debug)]
enum HorizonAssignmentResult {
    Found(Vec<HorizonAssignment>),
    Impossible,
}

/// Immutable lane eligibility plus the available production time before a
/// shared deadline.
///
/// Concrete lane ids are intentionally kept outside the feasibility state.
/// Lanes with the same eligibility signature and remaining capacity are
/// interchangeable for feasibility, even though lowering later chooses one
/// exact lane by readiness and id.
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

/// Exact feasibility oracle over canonical lane-capacity classes.
///
/// Each recursive step consumes one requested provider, so the search is
/// finite. Canonical capacity multisets collapse permutations of concrete
/// producers without imposing an arbitrary state cutoff on a feasible force
/// package.
struct AssignmentSearch<'a> {
    problem: &'a HorizonProblem,
    memo: BTreeMap<AssignmentSearchKey, bool>,
    visited_states: usize,
}

impl<'a> AssignmentSearch<'a> {
    const MAX_EAGER_LANE_PATTERNS: usize = 20_000;

    fn new(problem: &'a HorizonProblem) -> Self {
        Self {
            problem,
            memo: BTreeMap::new(),
            visited_states: 0,
        }
    }

    fn fits_concrete(&mut self, remaining_counts: &[usize], capacities: &[Tick]) -> bool {
        self.fits(
            remaining_counts.to_vec(),
            self.problem.canonical_lane_classes(capacities),
        )
    }

    fn fits(&mut self, remaining_counts: Vec<usize>, lane_classes: Vec<LaneCapacityClass>) -> bool {
        if remaining_counts.iter().all(|&count| count == 0) {
            return true;
        }

        let key = AssignmentSearchKey {
            remaining_counts,
            lane_classes,
        };
        if let Some(&result) = self.memo.get(&key) {
            return result;
        }
        self.visited_states = self.visited_states.saturating_add(1);

        if !remaining_work_fits_canonical_capacity(
            &self.problem.kinds,
            &key.remaining_counts,
            &key.lane_classes,
        ) {
            self.memo.insert(key, false);
            return false;
        }

        if let Some(result) = self.fits_single_lane_class(
            &key.remaining_counts,
            key.lane_classes
                .first()
                .filter(|_| key.lane_classes.len() == 1),
        ) {
            self.memo.insert(key, result);
            return result;
        }

        let Some(kind_index) = most_constrained_remaining_kind(
            &self.problem.kinds,
            &key.remaining_counts,
            &key.lane_classes,
        ) else {
            self.memo.insert(key, true);
            return true;
        };
        let kind = self.problem.kinds[kind_index];
        let duration = Tick::from(kind.stats().train_ticks);
        let mut candidates = Vec::new();
        for (class_index, class) in key.lane_classes.iter().enumerate() {
            if class.eligible_kinds.binary_search(&kind).is_err() {
                continue;
            }
            for (capacity_index, &capacity) in class.remaining_capacities.iter().enumerate() {
                if capacity < duration
                    || capacity_index > 0
                        && class.remaining_capacities[capacity_index - 1] == capacity
                {
                    continue;
                }
                candidates.push((capacity - duration, class_index, capacity_index));
            }
        }
        candidates.sort_unstable();

        for (_, class_index, capacity_index) in candidates {
            let mut next_counts = key.remaining_counts.clone();
            next_counts[kind_index] -= 1;
            let mut next_classes = key.lane_classes.clone();
            next_classes[class_index].remaining_capacities[capacity_index] -= duration;
            next_classes[class_index]
                .remaining_capacities
                .sort_unstable();
            if self.fits(next_counts, next_classes) {
                self.memo.insert(key, true);
                return true;
            }
        }

        self.memo.insert(key, false);
        false
    }

    /// Packs one interchangeable producer class a lane at a time when its
    /// exact pattern set is small. This is only a search-order optimization;
    /// larger pattern sets continue through the general exact job search.
    fn fits_single_lane_class(
        &mut self,
        remaining_counts: &[usize],
        class: Option<&LaneCapacityClass>,
    ) -> Option<bool> {
        let class = class?;
        let (&capacity, later_capacities) = class.remaining_capacities.split_first()?;
        let mut maxima = Vec::with_capacity(self.problem.kinds.len());
        let mut pattern_count = 1_usize;
        for (&kind, &count) in self.problem.kinds.iter().zip(remaining_counts) {
            let duration = Tick::from(kind.stats().train_ticks);
            let maximum = if class.eligible_kinds.binary_search(&kind).is_ok() {
                count.min(usize::try_from(capacity / duration).unwrap_or(usize::MAX))
            } else {
                0
            };
            maxima.push(maximum);
            pattern_count = pattern_count.saturating_mul(maximum.saturating_add(1));
        }
        if pattern_count > Self::MAX_EAGER_LANE_PATTERNS {
            return None;
        }

        let durations: Vec<_> = self
            .problem
            .kinds
            .iter()
            .map(|kind| Tick::from(kind.stats().train_ticks))
            .collect();
        let mut patterns = Vec::with_capacity(pattern_count);
        LanePatternEnumerator {
            durations: &durations,
            remaining_counts,
            maxima: &maxima,
            capacity,
            out: &mut patterns,
        }
        .enumerate(0, 0, &mut vec![0; maxima.len()]);
        patterns.sort_unstable_by(|(left_used, left), (right_used, right)| {
            right_used.cmp(left_used).then_with(|| right.cmp(left))
        });

        for (_, allocation) in patterns {
            let mut next_counts = remaining_counts.to_vec();
            for (remaining, assigned) in next_counts.iter_mut().zip(allocation) {
                *remaining -= assigned;
            }
            let next_classes = if later_capacities.is_empty() {
                Vec::new()
            } else {
                vec![LaneCapacityClass {
                    eligible_kinds: class.eligible_kinds.clone(),
                    remaining_capacities: later_capacities.to_vec(),
                }]
            };
            if self.fits(next_counts, next_classes) {
                return Some(true);
            }
        }
        Some(false)
    }
}

struct LanePatternEnumerator<'a> {
    durations: &'a [Tick],
    remaining_counts: &'a [usize],
    maxima: &'a [usize],
    capacity: Tick,
    out: &'a mut Vec<(Tick, Vec<usize>)>,
}

impl LanePatternEnumerator<'_> {
    fn enumerate(&mut self, kind_index: usize, used: Tick, allocation: &mut [usize]) {
        let Some(&duration) = self.durations.get(kind_index) else {
            let unused = self.capacity - used;
            let maximal = self
                .durations
                .iter()
                .enumerate()
                .all(|(index, &candidate)| {
                    allocation[index] == self.remaining_counts[index] || candidate > unused
                });
            if maximal {
                self.out.push((used, allocation.to_vec()));
            }
            return;
        };

        for count in 0..=self.maxima[kind_index] {
            let Some(next_used) = Tick::try_from(count)
                .ok()
                .and_then(|count| duration.checked_mul(count))
                .and_then(|added| used.checked_add(added))
            else {
                break;
            };
            if next_used > self.capacity {
                break;
            }
            allocation[kind_index] = count;
            self.enumerate(kind_index + 1, next_used, allocation);
        }
        allocation[kind_index] = 0;
    }
}

/// Schedules exact unit demand against completed producer lanes.
///
/// Each accepted append is included in later timing calculations for its lane.
/// Demands are considered in caller order. An infeasible higher-priority demand
/// does not block independent lower-priority work, but a feasible
/// higher-priority demand owns the remaining budget even when its next append
/// cannot yet be afforded.
/// Ground units require currently proven egress, and every selected unit must
/// be present in an observation before the caller's fixed deadline under the
/// conservative no-block bound. A unit that completes during the deadline's
/// production phase is too late because bot decisions precede production.
/// `budget` is additionally bounded by the snapshot's current bank; forecast
/// income is never spendable here.
pub(crate) fn plan_production_with_access(
    resources: &ResourceSnapshot,
    demands: &[ProductionDemand],
    deadline: Tick,
    budget: u32,
    access: &ProductionAccess,
) -> ProductionSchedule {
    let requested: Vec<_> = demands
        .iter()
        .copied()
        .filter(|demand| demand.count > 0)
        .collect();
    let requested_kinds: Vec<_> = requested
        .iter()
        .flat_map(|demand| core::iter::repeat_n(demand.kind, demand.count))
        .collect();
    let assignment = partial_horizon_assignment(resources, &requested_kinds, deadline, access);
    lower_horizon_assignment(resources, &requested, &assignment, budget)
}

/// Lowers a priority-preserving horizon assignment without inventing forecast
/// credit.
///
/// The assignment chooses accepted producer lanes together. This prevents an
/// early, short provider from taking the only lane on which a later, long
/// provider can meet the shared deadline. An unassigned request was proven not
/// to fit alongside all earlier accepted work. Current queue slots and current
/// scrap still decide which assigned providers can be commanded now.
fn lower_horizon_assignment(
    resources: &ResourceSnapshot,
    requested: &[ProductionDemand],
    assignment: &[Option<HorizonAssignment>],
    budget: u32,
) -> ProductionSchedule {
    let mut assigned_by_lane = vec![0_usize; resources.producers().len()];
    let spendable = budget.min(resources.current_scrap().amount());
    let mut remaining_budget = spendable;
    let mut appends = Vec::new();
    let mut spent = 0_u32;
    let mut deferred_scrap = 0_u32;
    let mut next_unfunded_cost = None;

    let mut assignment_index = 0_usize;
    'demands: for demand in requested {
        for _ in 0..demand.count {
            let assigned = assignment
                .get(assignment_index)
                .copied()
                .expect("an assignment has one entry per requested provider");
            assignment_index += 1;
            let Some(assigned) = assigned else {
                continue;
            };
            let lane = &resources.producers()[assigned.lane_index];
            let slot = lane.open_slots().nth(assigned_by_lane[assigned.lane_index]);
            assigned_by_lane[assigned.lane_index] =
                assigned_by_lane[assigned.lane_index].saturating_add(1);
            let cost = demand.kind.stats().cost;
            if cost > remaining_budget {
                next_unfunded_cost = Some(cost);
                break 'demands;
            }

            remaining_budget -= cost;
            if let Some(slot) = slot {
                spent = spent.saturating_add(cost);
                appends.push(PlannedAppend {
                    producer: slot.producer,
                    kind: demand.kind,
                    timing: assigned.timing,
                });
            } else {
                deferred_scrap = deferred_scrap.saturating_add(cost);
            }
        }
    }

    ProductionSchedule {
        appends,
        spent,
        deferred_scrap,
        next_unfunded_cost,
    }
}

/// Finds the lexicographically largest feasible subset in caller order.
///
/// Every newly requested provider is tested together with all earlier accepted
/// work, allowing the solver to move those earlier providers between lanes. A
/// failed addition therefore retains the last complete assignment instead of
/// falling back to a locally greedy schedule. Later independent work remains
/// eligible after a proven-impossible addition.
fn partial_horizon_assignment(
    resources: &ResourceSnapshot,
    requested: &[UnitKind],
    deadline: Tick,
    access: &ProductionAccess,
) -> Vec<Option<HorizonAssignment>> {
    let mut accepted_indices = Vec::with_capacity(requested.len());
    let mut accepted_kinds = Vec::with_capacity(requested.len());
    let mut planned_by_lane = vec![Vec::<UnitKind>::new(); resources.producers().len()];
    let mut assignment = vec![None; requested.len()];

    for (request_index, &kind) in requested.iter().enumerate() {
        if !kind_has_horizon_lane(resources, kind, deadline, access) {
            continue;
        }

        if let Some(assigned) =
            first_horizon_extension(resources, kind, &planned_by_lane, deadline, access)
        {
            planned_by_lane[assigned.lane_index].push(kind);
            accepted_indices.push(request_index);
            accepted_kinds.push(kind);
            assignment[request_index] = Some(assigned);
            continue;
        }

        accepted_kinds.push(kind);
        match complete_horizon_assignment(resources, &accepted_kinds, deadline, access) {
            HorizonAssignmentResult::Found(updated) => {
                accepted_indices.push(request_index);
                planned_by_lane.iter_mut().for_each(Vec::clear);
                for (&accepted_kind, assigned) in accepted_kinds.iter().zip(&updated) {
                    planned_by_lane[assigned.lane_index].push(accepted_kind);
                }
                for (&accepted_index, assigned) in accepted_indices.iter().zip(&updated) {
                    assignment[accepted_index] = Some(*assigned);
                }
            }
            HorizonAssignmentResult::Impossible => {
                accepted_kinds.pop();
            }
        }
    }

    assignment
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

/// Whether every requested append can finish through the allowed completed
/// producer lanes before `deadline`, independent of when forecast income
/// becomes spendable.
///
/// This is structural feasibility evidence for a strategy that already owns a
/// bounded forecast. It neither grants current credit nor permits an append;
/// [`plan_production_with_access`] remains the command-lowering boundary.
pub(crate) fn production_demands_fit_horizon_with_access(
    resources: &ResourceSnapshot,
    demands: &[ProductionDemand],
    deadline: Tick,
    access: &ProductionAccess,
) -> bool {
    let requested: Vec<_> = demands
        .iter()
        .filter(|demand| demand.count > 0)
        .flat_map(|demand| core::iter::repeat_n(demand.kind, demand.count))
        .collect();
    matches!(
        complete_horizon_assignment(resources, &requested, deadline, access),
        HorizonAssignmentResult::Found(_)
    )
}

fn complete_horizon_assignment(
    resources: &ResourceSnapshot,
    requested: &[UnitKind],
    deadline: Tick,
    access: &ProductionAccess,
) -> HorizonAssignmentResult {
    complete_horizon_assignment_diagnosed(resources, requested, deadline, access).0
}

fn complete_horizon_assignment_diagnosed(
    resources: &ResourceSnapshot,
    requested: &[UnitKind],
    deadline: Tick,
    access: &ProductionAccess,
) -> (HorizonAssignmentResult, usize) {
    let problem = HorizonProblem::new(resources, requested, deadline, access);
    let mut remaining_counts = problem.request_counts(requested);
    let mut remaining_capacities = problem.initial_capacities.clone();
    let mut search = AssignmentSearch::new(&problem);
    if !search.fits_concrete(&remaining_counts, &remaining_capacities) {
        return (HorizonAssignmentResult::Impossible, search.visited_states);
    }

    let mut planned_by_lane = vec![Vec::<UnitKind>::new(); resources.producers().len()];
    let mut assignment = Vec::with_capacity(requested.len());
    for &kind in requested {
        let kind_index = problem
            .kinds
            .binary_search(&kind)
            .expect("the problem kind list contains every request");
        remaining_counts[kind_index] -= 1;
        let duration = Tick::from(kind.stats().train_ticks);
        let mut selected = None;
        for (lane_index, timing, _) in
            horizon_candidates(resources, kind, &planned_by_lane, deadline, access)
        {
            let prior_capacity = remaining_capacities[lane_index];
            let Some(next_capacity) = prior_capacity.checked_sub(duration) else {
                continue;
            };
            remaining_capacities[lane_index] = next_capacity;
            if search.fits_concrete(&remaining_counts, &remaining_capacities) {
                selected = Some(HorizonAssignment { lane_index, timing });
                planned_by_lane[lane_index].push(kind);
                break;
            }
            remaining_capacities[lane_index] = prior_capacity;
        }
        let Some(selected) = selected else {
            return (HorizonAssignmentResult::Impossible, search.visited_states);
        };
        assignment.push(selected);
    }

    (
        HorizonAssignmentResult::Found(assignment),
        search.visited_states,
    )
}

fn most_constrained_remaining_kind(
    kinds: &[UnitKind],
    remaining_counts: &[usize],
    lane_classes: &[LaneCapacityClass],
) -> Option<usize> {
    kinds
        .iter()
        .enumerate()
        .filter(|(kind_index, _)| remaining_counts[*kind_index] > 0)
        .min_by_key(|(kind_index, kind)| {
            let duration = Tick::from(kind.stats().train_ticks);
            let available_slots = available_slots_for_kind(**kind, duration, lane_classes);
            (
                available_slots.saturating_sub(remaining_counts[*kind_index] as u128),
                available_slots,
                Reverse(duration),
                **kind,
            )
        })
        .map(|(kind_index, _)| kind_index)
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

fn first_horizon_extension(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    planned_by_lane: &[Vec<UnitKind>],
    deadline: Tick,
    access: &ProductionAccess,
) -> Option<HorizonAssignment> {
    horizon_candidates(resources, kind, planned_by_lane, deadline, access)
        .into_iter()
        .next()
        .map(|(lane_index, timing, _)| HorizonAssignment { lane_index, timing })
}

fn horizon_candidates(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    planned_by_lane: &[Vec<UnitKind>],
    deadline: Tick,
    access: &ProductionAccess,
) -> Vec<(usize, ProductionTiming, BuildingId)> {
    let mut candidates: Vec<_> = resources
        .producers()
        .iter()
        .enumerate()
        .filter_map(|(lane_index, lane)| {
            if !access.allows(lane.producer, kind) {
                return None;
            }
            let mut proposed = planned_by_lane[lane_index].clone();
            proposed.push(kind);
            let timing = lane.horizon_timing(&proposed)?;
            (timing.no_block_latest_ready_tick < deadline
                && egress_is_credible_for(kind, timing.current_egress))
            .then_some((lane_index, timing, lane.producer))
        })
        .collect();
    candidates.sort_unstable_by_key(|(lane_index, timing, producer)| {
        (timing.no_block_latest_ready_tick, *producer, *lane_index)
    });
    candidates
}

fn kind_has_horizon_lane(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    deadline: Tick,
    access: &ProductionAccess,
) -> bool {
    resources.producers().iter().any(|lane| {
        access.allows(lane.producer, kind)
            && lane.horizon_timing(&[kind]).is_some_and(|timing| {
                timing.no_block_latest_ready_tick < deadline
                    && egress_is_credible_for(kind, timing.current_egress)
            })
    })
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
