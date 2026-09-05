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

/// One exact producer append selected by [`plan_production`].
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
    /// Scheduled counts in caller priority order.
    #[cfg(test)]
    pub(crate) satisfied: Vec<ProductionDemand>,
    /// Unscheduled counts in caller priority order.
    #[cfg(test)]
    pub(crate) unmet: Vec<ProductionDemand>,
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
pub(crate) enum ProductionAccess {
    #[cfg(test)]
    Unrestricted,
    RestrictedKinds {
        allowed: Vec<(BuildingId, UnitKind)>,
        paid_allowed: Vec<(BuildingId, UnitKind)>,
    },
}

impl ProductionAccess {
    /// Restricts access to exact producer and unit-kind pairs.
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
        Self::RestrictedKinds {
            allowed,
            paid_allowed,
        }
    }

    pub(crate) fn allows(&self, producer: BuildingId, kind: UnitKind) -> bool {
        match self {
            #[cfg(test)]
            Self::Unrestricted => true,
            Self::RestrictedKinds { allowed, .. } => {
                allowed.binary_search(&(producer, kind)).is_ok()
            }
        }
    }

    fn allows_paid(&self, producer: BuildingId, kind: UnitKind) -> bool {
        match self {
            #[cfg(test)]
            Self::Unrestricted => true,
            Self::RestrictedKinds { paid_allowed, .. } => {
                paid_allowed.binary_search(&(producer, kind)).is_ok()
            }
        }
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
#[cfg(test)]
pub(crate) fn plan_production(
    resources: &ResourceSnapshot,
    demands: &[ProductionDemand],
    deadline: Tick,
    budget: u32,
) -> ProductionSchedule {
    plan_production_with_access(
        resources,
        demands,
        deadline,
        budget,
        &ProductionAccess::Unrestricted,
    )
}

/// [`plan_production`] with an operation-specific producer reachability bound.
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
    let mut remaining: Vec<_> = requested.iter().map(|demand| demand.count).collect();
    let mut scheduled = vec![0_usize; requested.len()];
    let mut assigned_by_lane = vec![0_usize; resources.producers().len()];
    let spendable = budget.min(resources.current_scrap().amount());
    let mut remaining_budget = spendable;
    let mut appends = Vec::new();
    let mut spent = 0_u32;
    let mut deferred_scrap = 0_u32;
    let mut next_unfunded_cost = None;

    let mut assignment_index = 0_usize;
    'demands: for (demand_index, demand) in requested.iter().enumerate() {
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
                scheduled[demand_index] += 1;
                remaining[demand_index] -= 1;
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

    production_schedule(
        requested,
        scheduled,
        remaining,
        appends,
        spent,
        deferred_scrap,
        next_unfunded_cost,
    )
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

fn production_schedule(
    _requested: &[ProductionDemand],
    _scheduled: Vec<usize>,
    _remaining: Vec<usize>,
    appends: Vec<PlannedAppend>,
    spent: u32,
    deferred_scrap: u32,
    next_unfunded_cost: Option<u32>,
) -> ProductionSchedule {
    #[cfg(test)]
    let satisfied = _requested
        .iter()
        .zip(_scheduled)
        .filter_map(|(demand, count)| {
            (count > 0).then_some(ProductionDemand {
                kind: demand.kind,
                count,
            })
        })
        .collect();
    #[cfg(test)]
    let unmet = _requested
        .iter()
        .zip(_remaining)
        .filter_map(|(demand, count)| {
            (count > 0).then_some(ProductionDemand {
                kind: demand.kind,
                count,
            })
        })
        .collect();

    ProductionSchedule {
        appends,
        #[cfg(test)]
        satisfied,
        #[cfg(test)]
        unmet,
        spent,
        deferred_scrap,
        next_unfunded_cost,
    }
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
#[cfg(test)]
pub(crate) fn count_paid_queued_ready(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    deadline: Tick,
) -> usize {
    count_paid_queued_ready_with_access(resources, kind, deadline, &ProductionAccess::Unrestricted)
}

/// [`count_paid_queued_ready`] with an operation-specific reachability bound.
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
                    producers.push(lane.producer);
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
mod tests {
    use super::super::{
        BuilderResource, CurrentScrap, ProducerLane, RecurringIncomeKind, RecurringIncomeStream,
        ResourceForecast, UnitResource, producer_preceding_ticks,
    };
    use super::*;
    use crate::stats::{BuildingKind, QUEUE_CAP};

    const OBSERVED_AT: Tick = 100;

    fn lane(
        producer: u32,
        building: BuildingKind,
        queued: Vec<UnitKind>,
        trainable: Vec<UnitKind>,
        egress: ProducerEgress,
    ) -> ProducerLane {
        lane_at(
            OBSERVED_AT,
            producer,
            building,
            queued,
            None,
            trainable,
            egress,
        )
    }

    fn lane_at(
        observed_at: Tick,
        producer: u32,
        building: BuildingKind,
        queued: Vec<UnitKind>,
        front_progress: Option<u32>,
        trainable: Vec<UnitKind>,
        egress: ProducerEgress,
    ) -> ProducerLane {
        let (earliest_preceding_ticks, no_block_latest_preceding_ticks) =
            producer_preceding_ticks(&queued, front_progress);
        ProducerLane {
            producer: BuildingId(producer),
            kind: building,
            queued,
            trainable,
            observed_at,
            front_progress,
            earliest_preceding_ticks,
            no_block_latest_preceding_ticks,
            ground_egress: egress,
        }
    }

    fn snapshot(scrap: u32, producers: Vec<ProducerLane>) -> ResourceSnapshot {
        let producer_slots = producers
            .iter()
            .flat_map(ProducerLane::open_slots)
            .collect();
        ResourceSnapshot {
            current_scrap: CurrentScrap(scrap),
            forecast: ResourceForecast {
                observed_at: OBSERVED_AT,
                income: Vec::new(),
            },
            units: Vec::<UnitResource>::new(),
            owned_buildings: Vec::new(),
            builders: Vec::<BuilderResource>::new(),
            producers,
            producer_slots,
        }
    }

    fn demand(kind: UnitKind, count: usize) -> ProductionDemand {
        ProductionDemand { kind, count }
    }

    fn lane_with_horizon_capacity(
        producer: u32,
        capacity: Tick,
        trainable: Vec<UnitKind>,
    ) -> ProducerLane {
        const HORIZON: Tick = 2_400;
        let preceding = HORIZON - capacity;
        let queued_kind = UnitKind::Condor;
        let queued_ticks = Tick::from(queued_kind.stats().train_ticks);
        let queue_count = preceding.div_ceil(queued_ticks);
        let total_ticks = queue_count * queued_ticks;
        let progress = u32::try_from(total_ticks - preceding).expect("fixture progress fits u32");
        lane_at(
            OBSERVED_AT,
            producer,
            BuildingKind::Airworks,
            vec![queued_kind; usize::try_from(queue_count).expect("fixture queue fits usize")],
            Some(progress),
            trainable,
            ProducerEgress::NotRequired,
        )
    }

    #[test]
    fn equal_lanes_use_id_then_spread_to_the_earlier_next_completion() {
        let resources = snapshot(
            1_000,
            vec![
                lane(
                    9,
                    BuildingKind::Foundry,
                    Vec::new(),
                    vec![UnitKind::Sentinel],
                    ProducerEgress::Open,
                ),
                lane(
                    3,
                    BuildingKind::Foundry,
                    Vec::new(),
                    vec![UnitKind::Sentinel],
                    ProducerEgress::Open,
                ),
            ],
        );

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Sentinel, 2)],
            Tick::MAX,
            1_000,
        );

        assert_eq!(
            schedule
                .appends
                .iter()
                .map(|append| append.producer)
                .collect::<Vec<_>>(),
            vec![BuildingId(3), BuildingId(9)]
        );
        assert_eq!(schedule.satisfied, vec![demand(UnitKind::Sentinel, 2)]);
        assert!(schedule.unmet.is_empty());
    }

    #[test]
    fn existing_queue_work_redirects_appends_to_the_earliest_lane() {
        let resources = snapshot(
            1_000,
            vec![
                lane(
                    9,
                    BuildingKind::Fabricator,
                    Vec::new(),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
                lane(
                    2,
                    BuildingKind::Fabricator,
                    vec![UnitKind::Bombard],
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
            ],
        );

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Bombard, 2)],
            Tick::MAX,
            1_000,
        );

        assert_eq!(
            schedule
                .appends
                .iter()
                .map(|append| append.producer)
                .collect::<Vec<_>>(),
            vec![BuildingId(9), BuildingId(2)]
        );
        assert_eq!(
            schedule.appends[0].timing.no_block_latest_ready_tick,
            OBSERVED_AT + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1
        );
        assert_eq!(
            schedule.appends[1].timing.no_block_latest_ready_tick,
            OBSERVED_AT + 2 * Tick::from(UnitKind::Bombard.stats().train_ticks) - 1
        );
    }

    #[test]
    fn later_appends_include_earlier_planned_work_on_the_same_lane() {
        let resources = snapshot(
            1_000,
            vec![lane(
                4,
                BuildingKind::Fabricator,
                Vec::new(),
                vec![UnitKind::Bombard],
                ProducerEgress::Open,
            )],
        );

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Bombard, 2)],
            Tick::MAX,
            1_000,
        );

        assert_eq!(schedule.appends.len(), 2);
        assert_eq!(
            schedule.appends[0].timing.no_block_latest_ready_tick,
            OBSERVED_AT + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1
        );
        assert_eq!(
            schedule.appends[1].timing.no_block_latest_ready_tick,
            OBSERVED_AT + 2 * Tick::from(UnitKind::Bombard.stats().train_ticks) - 1
        );
    }

    #[test]
    fn horizon_timing_reuses_queue_slots_without_opening_them_now() {
        let queued = vec![UnitKind::Lancer; crate::stats::QUEUE_CAP];
        let lane = lane(
            4,
            BuildingKind::Fabricator,
            queued.clone(),
            vec![UnitKind::Bombard],
            ProducerEgress::Open,
        );
        let planned = [UnitKind::Bombard];

        assert!(
            lane.production_timing(&planned).is_none(),
            "a full current queue exposes no command slot"
        );
        assert_eq!(lane.open_slots().count(), 0);

        let timing = lane
            .horizon_timing(&planned)
            .expect("the current queue drains before the later provider is needed");
        let preceding_ticks = queued.iter().fold(0_u64, |ticks, kind| {
            ticks + Tick::from(kind.stats().train_ticks)
        });
        assert_eq!(
            timing.no_block_latest_ready_tick,
            OBSERVED_AT + preceding_ticks + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1
        );
    }

    #[test]
    fn horizon_timing_does_not_cap_lifetime_throughput_at_queue_depth() {
        let lane = lane(
            4,
            BuildingKind::Fabricator,
            Vec::new(),
            vec![UnitKind::Bombard],
            ProducerEgress::Open,
        );
        let planned = vec![UnitKind::Bombard; crate::stats::QUEUE_CAP + 1];

        assert!(
            lane.production_timing(&planned).is_none(),
            "only the current queue remains capped"
        );
        let timing = lane
            .horizon_timing(&planned)
            .expect("a long horizon can use a producer after earlier slots drain");
        assert_eq!(
            timing.no_block_latest_ready_tick,
            OBSERVED_AT
                + u64::try_from(planned.len()).expect("small fixture")
                    * Tick::from(UnitKind::Bombard.stats().train_ticks)
                - 1
        );
    }

    #[test]
    fn horizon_feasibility_backtracks_for_a_ferrous_minimum() {
        let resources = snapshot(
            0,
            vec![
                lane(
                    1,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Kestrel, UnitKind::Condor],
                    ProducerEgress::NotRequired,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    vec![UnitKind::Talon],
                    vec![UnitKind::Kestrel, UnitKind::Condor],
                    ProducerEgress::NotRequired,
                ),
            ],
        );
        let deadline = OBSERVED_AT + Tick::from(UnitKind::Condor.stats().train_ticks) + 1;

        assert!(production_demands_fit_horizon_with_access(
            &resources,
            &[demand(UnitKind::Kestrel, 1), demand(UnitKind::Condor, 1),],
            deadline,
            &ProductionAccess::Unrestricted,
        ));
    }

    #[test]
    fn horizon_feasibility_backtracks_for_a_cupric_minimum() {
        let resources = snapshot(
            0,
            vec![
                lane(
                    1,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Gnat, UnitKind::Moth],
                    ProducerEgress::NotRequired,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    vec![UnitKind::Wisp],
                    vec![UnitKind::Gnat, UnitKind::Moth],
                    ProducerEgress::NotRequired,
                ),
            ],
        );
        let deadline = OBSERVED_AT + Tick::from(UnitKind::Moth.stats().train_ticks) + 1;

        assert!(production_demands_fit_horizon_with_access(
            &resources,
            &[demand(UnitKind::Gnat, 1), demand(UnitKind::Moth, 1)],
            deadline,
            &ProductionAccess::Unrestricted,
        ));
    }

    #[test]
    fn complete_lowering_uses_the_horizon_assignment_that_preserves_long_providers() {
        for (scout, strike, queued) in [
            (UnitKind::Kestrel, UnitKind::Condor, UnitKind::Talon),
            (UnitKind::Gnat, UnitKind::Moth, UnitKind::Wisp),
        ] {
            let resources = snapshot(
                scout.stats().cost.saturating_add(strike.stats().cost),
                vec![
                    lane(
                        1,
                        BuildingKind::Airworks,
                        Vec::new(),
                        vec![scout, strike],
                        ProducerEgress::NotRequired,
                    ),
                    lane(
                        2,
                        BuildingKind::Airworks,
                        vec![queued],
                        vec![scout, strike],
                        ProducerEgress::NotRequired,
                    ),
                ],
            );
            let deadline = OBSERVED_AT + Tick::from(strike.stats().train_ticks) + 1;

            let schedule = plan_production(
                &resources,
                &[demand(scout, 1), demand(strike, 1)],
                deadline,
                u32::MAX,
            );

            assert!(schedule.unmet.is_empty(), "{scout:?} plus {strike:?}");
            assert_eq!(
                schedule
                    .appends
                    .iter()
                    .map(|append| (append.kind, append.producer))
                    .collect::<Vec<_>>(),
                vec![(scout, BuildingId(2)), (strike, BuildingId(1))]
            );
            assert!(
                schedule
                    .appends
                    .iter()
                    .all(|append| append.timing.no_block_latest_ready_tick < deadline)
            );
        }
    }

    #[test]
    fn tiered_lane_eligibility_preserves_the_only_heavy_unit_lane() {
        let resources = snapshot(
            2_000,
            vec![
                lane(
                    1,
                    BuildingKind::Crucible,
                    Vec::new(),
                    vec![UnitKind::Bombard, UnitKind::Avalanche],
                    ProducerEgress::Open,
                ),
                lane(
                    2,
                    BuildingKind::Fabricator,
                    Vec::new(),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
            ],
        );
        let deadline = OBSERVED_AT + Tick::from(UnitKind::Avalanche.stats().train_ticks) + 1;

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Bombard, 1), demand(UnitKind::Avalanche, 1)],
            deadline,
            u32::MAX,
        );

        assert_eq!(
            schedule
                .appends
                .iter()
                .map(|append| (append.kind, append.producer))
                .collect::<Vec<_>>(),
            vec![
                (UnitKind::Bombard, BuildingId(2)),
                (UnitKind::Avalanche, BuildingId(1)),
            ]
        );
        assert!(schedule.unmet.is_empty());
    }

    #[test]
    fn an_impossible_tail_preserves_backtracked_higher_priority_work() {
        let resources = snapshot(
            5_000,
            vec![
                lane(
                    1,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Kestrel, UnitKind::Condor],
                    ProducerEgress::NotRequired,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    vec![UnitKind::Talon],
                    vec![UnitKind::Kestrel, UnitKind::Condor],
                    ProducerEgress::NotRequired,
                ),
                lane(
                    3,
                    BuildingKind::Fabricator,
                    Vec::new(),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
            ],
        );
        let demands = [
            demand(UnitKind::Kestrel, 1),
            demand(UnitKind::Bombard, 1),
            demand(UnitKind::Condor, 1),
            demand(UnitKind::Avalanche, 1),
        ];
        let deadline = OBSERVED_AT + Tick::from(UnitKind::Condor.stats().train_ticks) + 1;

        let schedule = plan_production(&resources, &demands, deadline, u32::MAX);

        assert_eq!(
            schedule
                .appends
                .iter()
                .map(|append| (append.kind, append.producer))
                .collect::<Vec<_>>(),
            vec![
                (UnitKind::Kestrel, BuildingId(2)),
                (UnitKind::Bombard, BuildingId(3)),
                (UnitKind::Condor, BuildingId(1)),
            ]
        );
        assert_eq!(schedule.satisfied, demands[..3]);
        assert_eq!(schedule.unmet, vec![demands[3]]);
    }

    #[test]
    fn a_large_symmetric_prefix_survives_a_late_impossible_demand() {
        let mut producers: Vec<_> = (1..=8)
            .map(|producer| {
                lane(
                    producer,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Darter],
                    ProducerEgress::NotRequired,
                )
            })
            .collect();
        producers.push(lane(
            20,
            BuildingKind::Fabricator,
            Vec::new(),
            vec![UnitKind::Bombard],
            ProducerEgress::Open,
        ));
        let resources = snapshot(5_000, producers);
        let demands = [
            demand(UnitKind::Darter, 12),
            demand(UnitKind::Avalanche, 1),
            demand(UnitKind::Bombard, 1),
        ];
        let deadline = OBSERVED_AT + 2 * Tick::from(UnitKind::Darter.stats().train_ticks) + 1;

        let first = plan_production(&resources, &demands, deadline, u32::MAX);
        let second = plan_production(&resources, &demands, deadline, u32::MAX);

        assert_eq!(
            first, second,
            "the bounded assignment must be deterministic"
        );
        assert_eq!(
            first.satisfied,
            vec![demand(UnitKind::Darter, 12), demand(UnitKind::Bombard, 1),]
        );
        assert_eq!(first.unmet, vec![demand(UnitKind::Avalanche, 1)]);
        assert_eq!(first.appends.len(), 13);
        assert_eq!(
            first.appends.last().map(|append| append.kind),
            Some(UnitKind::Bombard)
        );
    }

    #[test]
    fn over_capacity_work_retains_its_maximum_priority_prefix() {
        let mut producers: Vec<_> = (1..=8)
            .map(|producer| {
                lane(
                    producer,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Darter],
                    ProducerEgress::NotRequired,
                )
            })
            .collect();
        producers.push(lane(
            20,
            BuildingKind::Fabricator,
            Vec::new(),
            vec![UnitKind::Bombard],
            ProducerEgress::Open,
        ));
        let resources = snapshot(5_000, producers);
        let demands = [demand(UnitKind::Darter, 17), demand(UnitKind::Bombard, 1)];
        let deadline = OBSERVED_AT + 2 * Tick::from(UnitKind::Darter.stats().train_ticks) + 1;

        let schedule = plan_production(&resources, &demands, deadline, u32::MAX);

        assert_eq!(
            schedule.satisfied,
            vec![demand(UnitKind::Darter, 16), demand(UnitKind::Bombard, 1),]
        );
        assert_eq!(schedule.unmet, vec![demand(UnitKind::Darter, 1)]);
        assert_eq!(schedule.appends.len(), 17);
        assert_eq!(
            schedule.appends.last().map(|append| append.kind),
            Some(UnitKind::Bombard)
        );
    }

    #[test]
    fn fixed_connected_horizon_handles_sixteen_full_airworks_without_exhaustion() {
        let mut producers: Vec<_> = (1..=16)
            .map(|producer| {
                lane(
                    producer,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Kestrel, UnitKind::Darter],
                    ProducerEgress::NotRequired,
                )
            })
            .collect();
        producers.push(lane(
            20,
            BuildingKind::Fabricator,
            Vec::new(),
            vec![UnitKind::Bombard],
            ProducerEgress::Open,
        ));
        let resources = snapshot(100_000, producers);
        let mut requested = vec![UnitKind::Kestrel];
        requested.extend(core::iter::repeat_n(UnitKind::Darter, 256));
        requested.push(UnitKind::Bombard);
        let deadline = OBSERVED_AT + 2_400;

        let assignment = partial_horizon_assignment(
            &resources,
            &requested,
            deadline,
            &ProductionAccess::Unrestricted,
        );

        assert!(assignment[0].is_some(), "the minimum scout must fit");
        assert_eq!(
            assignment[1..=256]
                .iter()
                .filter(|assigned| assigned.is_some())
                .count(),
            255,
            "one 120-tick scout leaves room for 255 150-tick Darters across sixteen lanes"
        );
        assert!(
            assignment[257].is_some(),
            "a proven over-capacity air tail must not suppress independent ground work"
        );
    }

    #[test]
    fn irregular_airworks_loads_do_not_create_a_hidden_package_cap() {
        let trainable = vec![UnitKind::Kestrel, UnitKind::Buzzard, UnitKind::Condor];
        let resources = snapshot(
            100_000,
            vec![
                lane_at(
                    OBSERVED_AT,
                    1,
                    BuildingKind::Airworks,
                    vec![UnitKind::Buzzard, UnitKind::Buzzard],
                    Some(19),
                    trainable.clone(),
                    ProducerEgress::NotRequired,
                ),
                lane_at(
                    OBSERVED_AT,
                    2,
                    BuildingKind::Airworks,
                    vec![UnitKind::Kestrel],
                    Some(103),
                    trainable.clone(),
                    ProducerEgress::NotRequired,
                ),
                lane(
                    3,
                    BuildingKind::Airworks,
                    Vec::new(),
                    trainable.clone(),
                    ProducerEgress::NotRequired,
                ),
                lane_at(
                    OBSERVED_AT,
                    4,
                    BuildingKind::Airworks,
                    vec![UnitKind::Condor],
                    Some(157),
                    trainable,
                    ProducerEgress::NotRequired,
                ),
            ],
        );
        let mut requested = vec![UnitKind::Kestrel];
        requested.extend(core::iter::repeat_n(UnitKind::Buzzard, 4));
        requested.extend(core::iter::repeat_n(UnitKind::Condor, 3));
        requested.extend(core::iter::repeat_n(UnitKind::Buzzard, 29));
        let deadline = OBSERVED_AT + 2_400;

        let (result, visited_states) = complete_horizon_assignment_diagnosed(
            &resources,
            &requested,
            deadline,
            &ProductionAccess::Unrestricted,
        );
        let HorizonAssignmentResult::Found(assignment) = result else {
            panic!("the exact scheduler rejected a feasible connected air package");
        };

        assert_eq!(assignment.len(), requested.len());
        assert!(
            assignment
                .iter()
                .all(|assigned| assigned.timing.no_block_latest_ready_tick < deadline)
        );
        assert!(
            visited_states < 1_000,
            "canonical capacity search visited {visited_states} states"
        );
        let demands = [
            demand(UnitKind::Kestrel, 1),
            demand(UnitKind::Buzzard, 4),
            demand(UnitKind::Condor, 3),
            demand(UnitKind::Buzzard, 29),
        ];
        assert!(production_demands_fit_horizon_with_access(
            &resources,
            &demands,
            deadline,
            &ProductionAccess::Unrestricted,
        ));
        let first = plan_production(&resources, &demands, deadline, u32::MAX);
        let second = plan_production(&resources, &demands, deadline, u32::MAX);
        assert_eq!(first, second);
        assert!(
            first
                .appends
                .iter()
                .all(|append| append.timing.no_block_latest_ready_tick < deadline)
        );
    }

    #[test]
    fn modular_capacity_rejects_fragmented_partial_eligibility_immediately() {
        let all = vec![UnitKind::Kestrel, UnitKind::Buzzard, UnitKind::Condor];
        let air_ground_and_bomber = vec![UnitKind::Buzzard, UnitKind::Condor];
        let scout_and_air_ground = vec![UnitKind::Kestrel, UnitKind::Buzzard];
        let lanes = |bomber_only_capacity| {
            vec![
                lane_with_horizon_capacity(1, bomber_only_capacity, vec![UnitKind::Condor]),
                lane_with_horizon_capacity(2, 1_327, all.clone()),
                lane_with_horizon_capacity(3, 807, all.clone()),
                lane_with_horizon_capacity(4, 1_834, all.clone()),
                lane_with_horizon_capacity(5, 2_079, all.clone()),
                lane_with_horizon_capacity(6, 1_749, air_ground_and_bomber.clone()),
                lane_with_horizon_capacity(7, 2_056, scout_and_air_ground.clone()),
                lane_with_horizon_capacity(8, 779, scout_and_air_ground.clone()),
            ]
        };
        let requested: Vec<_> = core::iter::repeat_n(UnitKind::Kestrel, 19)
            .chain(core::iter::repeat_n(UnitKind::Buzzard, 15))
            .chain(core::iter::repeat_n(UnitKind::Condor, 8))
            .collect();
        let deadline = OBSERVED_AT + 2_400;

        let fragmented = snapshot(0, lanes(1_390));
        let (result, visited_states) = complete_horizon_assignment_diagnosed(
            &fragmented,
            &requested,
            deadline,
            &ProductionAccess::Unrestricted,
        );
        assert!(matches!(result, HorizonAssignmentResult::Impossible));
        assert_eq!(visited_states, 1);

        let relaxed = snapshot(0, lanes(1_790));
        let (result, _) = complete_horizon_assignment_diagnosed(
            &relaxed,
            &requested,
            deadline,
            &ProductionAccess::Unrestricted,
        );
        let HorizonAssignmentResult::Found(assignment) = result else {
            panic!("the modular bound rejected a feasible neighboring fixture");
        };
        assert_eq!(assignment.len(), requested.len());
        assert!(
            assignment
                .iter()
                .all(|assigned| assigned.timing.no_block_latest_ready_tick < deadline)
        );
    }

    #[test]
    fn horizon_feasibility_preserves_deadline_access_and_egress_bounds() {
        let air = snapshot(
            0,
            vec![
                lane(
                    1,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Kestrel],
                    ProducerEgress::NotRequired,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    vec![UnitKind::Talon],
                    vec![UnitKind::Kestrel],
                    ProducerEgress::NotRequired,
                ),
            ],
        );
        let ready = OBSERVED_AT + Tick::from(UnitKind::Kestrel.stats().train_ticks) - 1;
        assert!(!production_demands_fit_horizon_with_access(
            &air,
            &[demand(UnitKind::Kestrel, 1)],
            ready,
            &ProductionAccess::Unrestricted,
        ));
        assert!(production_demands_fit_horizon_with_access(
            &air,
            &[demand(UnitKind::Kestrel, 1)],
            ready + 1,
            &ProductionAccess::Unrestricted,
        ));
        assert!(!production_demands_fit_horizon_with_access(
            &air,
            &[demand(UnitKind::Kestrel, 1)],
            ready + 1,
            &ProductionAccess::restricted_kinds(vec![(BuildingId(2), UnitKind::Kestrel)]),
        ));

        let blocked_ground = snapshot(
            0,
            vec![lane(
                3,
                BuildingKind::Fabricator,
                Vec::new(),
                vec![UnitKind::Bombard],
                ProducerEgress::Blocked,
            )],
        );
        assert!(!production_demands_fit_horizon_with_access(
            &blocked_ground,
            &[demand(UnitKind::Bombard, 1)],
            Tick::MAX,
            &ProductionAccess::Unrestricted,
        ));
    }

    #[test]
    fn restricted_access_is_specific_to_the_unit_kind_on_one_producer() {
        let producer = BuildingId(7);
        let access = ProductionAccess::restricted_kinds(vec![
            (producer, UnitKind::Bombard),
            (producer, UnitKind::Bombard),
        ]);

        assert!(access.allows(producer, UnitKind::Bombard));
        assert!(!access.allows(producer, UnitKind::Lancer));
        assert!(!access.allows(BuildingId(8), UnitKind::Bombard));
    }

    #[test]
    fn paid_queue_access_does_not_authorize_a_new_append() {
        let producer = BuildingId(7);
        let resources = snapshot(
            0,
            vec![lane(
                producer.0,
                BuildingKind::Airworks,
                vec![UnitKind::Condor],
                Vec::new(),
                ProducerEgress::NotRequired,
            )],
        );
        let access = ProductionAccess::restricted_kinds_with_paid(
            Vec::new(),
            vec![(producer, UnitKind::Condor)],
        );

        assert!(!access.allows(producer, UnitKind::Condor));
        assert_eq!(
            count_paid_queued_ready_with_access(&resources, UnitKind::Condor, Tick::MAX, &access),
            1
        );
    }

    #[test]
    fn paid_queue_credit_requires_completion_before_the_deadline_observation() {
        let resources = snapshot(
            0,
            vec![lane(
                4,
                BuildingKind::Fabricator,
                vec![UnitKind::Lancer, UnitKind::Bombard, UnitKind::Bombard],
                vec![UnitKind::Bombard],
                ProducerEgress::Open,
            )],
        );
        let first_bombard_ready = OBSERVED_AT
            + Tick::from(UnitKind::Lancer.stats().train_ticks)
            + Tick::from(UnitKind::Bombard.stats().train_ticks)
            - 1;
        let second_bombard_ready =
            first_bombard_ready + Tick::from(UnitKind::Bombard.stats().train_ticks);

        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Bombard, first_bombard_ready - 1),
            0
        );
        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Bombard, first_bombard_ready),
            0,
            "a provider spawned during the deadline tick is absent when the bot decides"
        );
        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Bombard, first_bombard_ready + 1,),
            1
        );
        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Bombard, second_bombard_ready),
            1
        );
        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Bombard, second_bombard_ready + 1,),
            2
        );
        assert_eq!(
            paid_queued_ready_producers_with_access(
                &resources,
                UnitKind::Bombard,
                second_bombard_ready + 1,
                &ProductionAccess::Unrestricted,
            ),
            vec![BuildingId(4), BuildingId(4)],
            "the exact ownership surface preserves same-producer multiplicity"
        );
        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Avalanche, Tick::MAX),
            0
        );
    }

    #[test]
    fn paid_front_queue_keeps_its_fixed_ready_tick_across_decision_cadences() {
        let deadline = 280;
        let expected_ready = 279;

        for elapsed in [0_u32, 1, 12, 24, 60, 120, 179] {
            let observed_at = OBSERVED_AT + Tick::from(elapsed);
            let producer = || {
                lane_at(
                    observed_at,
                    4,
                    BuildingKind::Airworks,
                    vec![UnitKind::Buzzard],
                    Some(elapsed),
                    vec![UnitKind::Buzzard],
                    ProducerEgress::NotRequired,
                )
            };
            let first = snapshot(0, vec![producer()]);
            let second = snapshot(0, vec![producer()]);

            assert_eq!(
                count_paid_queued_ready(&first, UnitKind::Buzzard, deadline),
                1,
                "the paid Buzzard lost deadline credit after {elapsed} production ticks"
            );
            assert_eq!(
                count_paid_queued_ready(&first, UnitKind::Buzzard, expected_ready),
                0,
                "a unit produced on the deadline tick is not in that observation"
            );
            assert_eq!(
                first, second,
                "identical queue evidence must derive bit-identical resources"
            );

            let timing = first.producers()[0]
                .production_timing(&[UnitKind::Buzzard])
                .expect("one follow-up Buzzard fits the queue");
            assert_eq!(
                timing.earliest_ready_tick,
                expected_ready + Tick::from(UnitKind::Buzzard.stats().train_ticks)
            );
            assert_eq!(
                timing.no_block_latest_ready_tick,
                timing.earliest_ready_tick
            );
        }
    }

    #[test]
    fn paid_queue_credit_uses_only_the_current_queue_and_live_producer() {
        let at_tick_112 = |queued| {
            snapshot(
                0,
                vec![lane_at(
                    112,
                    4,
                    BuildingKind::Airworks,
                    queued,
                    Some(12),
                    vec![UnitKind::Buzzard, UnitKind::Condor],
                    ProducerEgress::NotRequired,
                )],
            )
        };
        let commissioned = at_tick_112(vec![UnitKind::Buzzard]);
        let changed_queue = at_tick_112(vec![UnitKind::Condor]);
        let lost_producer = snapshot(0, Vec::new());

        assert_eq!(
            count_paid_queued_ready(&commissioned, UnitKind::Buzzard, 280),
            1
        );
        assert_eq!(
            count_paid_queued_ready(&changed_queue, UnitKind::Buzzard, 280),
            0,
            "front progress cannot preserve credit after the queued kind changes"
        );
        assert_eq!(
            count_paid_queued_ready(&lost_producer, UnitKind::Buzzard, 280),
            0,
            "a destroyed producer cannot preserve credit for its former queue"
        );
    }

    #[test]
    fn paid_ground_queue_credit_requires_known_open_egress() {
        let almost_complete = UnitKind::Bombard.stats().train_ticks - 1;
        let resources = snapshot(
            0,
            vec![
                lane_at(
                    OBSERVED_AT,
                    1,
                    BuildingKind::Fabricator,
                    vec![UnitKind::Bombard],
                    Some(almost_complete),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Blocked,
                ),
                lane_at(
                    OBSERVED_AT,
                    2,
                    BuildingKind::Fabricator,
                    vec![UnitKind::Bombard],
                    Some(almost_complete),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Unknown,
                ),
                lane_at(
                    OBSERVED_AT,
                    3,
                    BuildingKind::Fabricator,
                    vec![UnitKind::Bombard],
                    Some(almost_complete),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
            ],
        );

        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Bombard, OBSERVED_AT + 1),
            1,
            "exact progress cannot turn a blocked or unknown doorstep into a deadline promise"
        );
    }

    #[test]
    fn paid_air_queue_credit_does_not_require_ground_egress() {
        let resources = snapshot(
            0,
            vec![lane(
                7,
                BuildingKind::Airworks,
                vec![UnitKind::Buzzard],
                vec![UnitKind::Buzzard],
                ProducerEgress::Unknown,
            )],
        );

        assert_eq!(
            count_paid_queued_ready(&resources, UnitKind::Buzzard, Tick::MAX),
            1
        );
    }

    #[test]
    fn fixed_deadline_requires_scheduled_work_before_the_deadline_observation() {
        let resources = snapshot(
            1_000,
            vec![lane(
                4,
                BuildingKind::Fabricator,
                Vec::new(),
                vec![UnitKind::Bombard],
                ProducerEgress::Open,
            )],
        );
        let first_ready = OBSERVED_AT + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1;

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Bombard, 2)],
            first_ready + 1,
            1_000,
        );

        assert_eq!(schedule.appends.len(), 1);
        assert_eq!(schedule.satisfied, vec![demand(UnitKind::Bombard, 1)]);
        assert_eq!(schedule.unmet, vec![demand(UnitKind::Bombard, 1)]);
        assert_eq!(schedule.spent, UnitKind::Bombard.stats().cost);
        assert_eq!(schedule.next_unfunded_cost, None);

        let too_late = plan_production(
            &resources,
            &[demand(UnitKind::Bombard, 1)],
            first_ready,
            1_000,
        );
        assert!(too_late.appends.is_empty());
        assert!(too_late.satisfied.is_empty());
        assert_eq!(too_late.unmet, vec![demand(UnitKind::Bombard, 1)]);
    }

    #[test]
    fn deadline_uses_the_no_block_latest_bound_not_optimistic_front_progress() {
        let resources = snapshot(
            1_000,
            vec![lane(
                4,
                BuildingKind::Fabricator,
                vec![UnitKind::Bombard],
                vec![UnitKind::Sentinel],
                ProducerEgress::Open,
            )],
        );
        let optimistic_ready = OBSERVED_AT + Tick::from(UnitKind::Sentinel.stats().train_ticks);

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Sentinel, 1)],
            optimistic_ready,
            1_000,
        );

        assert!(schedule.appends.is_empty());
        assert!(schedule.satisfied.is_empty());
        assert_eq!(schedule.unmet, vec![demand(UnitKind::Sentinel, 1)]);
        assert_eq!(schedule.next_unfunded_cost, None);
    }

    #[test]
    fn ground_requires_proven_egress_but_airworks_air_does_not() {
        let resources = snapshot(
            1_000,
            vec![
                lane(
                    1,
                    BuildingKind::Fabricator,
                    Vec::new(),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Blocked,
                ),
                lane(
                    2,
                    BuildingKind::Fabricator,
                    Vec::new(),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Unknown,
                ),
                lane(
                    3,
                    BuildingKind::Fabricator,
                    Vec::new(),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
                lane(
                    4,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Buzzard],
                    ProducerEgress::Unknown,
                ),
            ],
        );

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Bombard, 1), demand(UnitKind::Buzzard, 1)],
            Tick::MAX,
            1_000,
        );

        assert!(schedule.appends.iter().any(|append| {
            append.producer == BuildingId(3) && append.kind == UnitKind::Bombard
        }));
        assert!(schedule.appends.iter().any(|append| {
            append.producer == BuildingId(4)
                && append.kind == UnitKind::Buzzard
                && append.timing.current_egress == ProducerEgress::NotRequired
        }));
        assert!(schedule.unmet.is_empty());
    }

    #[test]
    fn an_unfunded_selected_append_stops_before_a_cheaper_later_one() {
        let resources = snapshot(
            150,
            vec![
                lane(
                    1,
                    BuildingKind::Fabricator,
                    Vec::new(),
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
                lane(
                    2,
                    BuildingKind::Foundry,
                    vec![UnitKind::Breaker],
                    vec![UnitKind::Sentinel],
                    ProducerEgress::Open,
                ),
            ],
        );

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Bombard, 1), demand(UnitKind::Sentinel, 1)],
            2_000,
            1_000,
        );

        assert!(schedule.appends.is_empty());
        assert_eq!(schedule.spent, 0);
        assert_eq!(
            schedule.next_unfunded_cost,
            Some(UnitKind::Bombard.stats().cost)
        );
        assert_eq!(
            schedule.unmet,
            vec![demand(UnitKind::Bombard, 1), demand(UnitKind::Sentinel, 1),]
        );
    }

    #[test]
    fn a_slot_blocked_priority_demand_stops_before_a_later_open_lane() {
        let bombard_cost = UnitKind::Bombard.stats().cost;
        let buzzard_cost = UnitKind::Buzzard.stats().cost;
        let resources = snapshot(
            bombard_cost,
            vec![
                lane(
                    1,
                    BuildingKind::Fabricator,
                    vec![UnitKind::Lancer; QUEUE_CAP],
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Buzzard],
                    ProducerEgress::Unknown,
                ),
            ],
        );
        let demands = [demand(UnitKind::Bombard, 1), demand(UnitKind::Buzzard, 1)];

        let blocked = plan_production(&resources, &demands, 4_000, bombard_cost);

        assert!(blocked.appends.is_empty());
        assert_eq!(blocked.spent, 0);
        assert_eq!(blocked.deferred_scrap, bombard_cost);
        assert_eq!(blocked.unmet, demands);
        assert_eq!(blocked.next_unfunded_cost, Some(buzzard_cost));

        let parallel = snapshot(
            bombard_cost.saturating_add(buzzard_cost),
            vec![
                lane(
                    1,
                    BuildingKind::Fabricator,
                    vec![UnitKind::Lancer; QUEUE_CAP],
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Buzzard],
                    ProducerEgress::Unknown,
                ),
            ],
        );
        let parallel = plan_production(
            &parallel,
            &demands,
            4_000,
            bombard_cost.saturating_add(buzzard_cost),
        );

        assert_eq!(parallel.appends.len(), 1);
        assert_eq!(parallel.appends[0].kind, UnitKind::Buzzard);
        assert_eq!(parallel.spent, buzzard_cost);
        assert_eq!(parallel.deferred_scrap, bombard_cost);

        let released = snapshot(
            bombard_cost.saturating_add(buzzard_cost),
            vec![
                lane(
                    1,
                    BuildingKind::Fabricator,
                    vec![UnitKind::Lancer; QUEUE_CAP - 1],
                    vec![UnitKind::Bombard],
                    ProducerEgress::Open,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    Vec::new(),
                    vec![UnitKind::Buzzard],
                    ProducerEgress::Unknown,
                ),
            ],
        );
        let released = plan_production(
            &released,
            &demands,
            4_000,
            bombard_cost.saturating_add(buzzard_cost),
        );

        assert_eq!(released.appends[0].kind, UnitKind::Bombard);
        assert_eq!(released.appends[0].producer, BuildingId(1));
        assert_eq!(released.appends[1].kind, UnitKind::Buzzard);
        assert_eq!(released.spent, bombard_cost.saturating_add(buzzard_cost));
        assert_eq!(released.deferred_scrap, 0);
    }

    #[test]
    fn current_bank_and_caller_budget_independently_bound_spending() {
        let producer = || {
            lane(
                4,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Kestrel],
                ProducerEgress::Unknown,
            )
        };
        let cost = UnitKind::Kestrel.stats().cost;

        let bank_limited = plan_production(
            &snapshot(cost - 1, vec![producer()]),
            &[demand(UnitKind::Kestrel, 1)],
            Tick::MAX,
            u32::MAX,
        );
        let caller_limited = plan_production(
            &snapshot(u32::MAX, vec![producer()]),
            &[demand(UnitKind::Kestrel, 1)],
            Tick::MAX,
            cost - 1,
        );

        for schedule in [bank_limited, caller_limited] {
            assert!(schedule.appends.is_empty());
            assert_eq!(schedule.spent, 0);
            assert_eq!(schedule.next_unfunded_cost, Some(cost));
            assert_eq!(schedule.unmet, vec![demand(UnitKind::Kestrel, 1)]);
        }
    }

    #[test]
    fn forecast_income_never_becomes_command_credit() {
        let mut resources = snapshot(
            0,
            vec![lane(
                4,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Kestrel],
                ProducerEgress::Unknown,
            )],
        );
        resources.forecast.income.push(RecurringIncomeStream {
            source: BuildingId(12),
            kind: RecurringIncomeKind::Reclaimer,
            amount: 1_000,
            period: 1,
            first_payment_tick: OBSERVED_AT,
        });

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Kestrel, 1)],
            Tick::MAX,
            u32::MAX,
        );

        assert!(schedule.appends.is_empty());
        assert_eq!(schedule.spent, 0);
        assert_eq!(
            schedule.next_unfunded_cost,
            Some(UnitKind::Kestrel.stats().cost)
        );
        assert_eq!(schedule.unmet, vec![demand(UnitKind::Kestrel, 1)]);
    }

    #[test]
    fn exact_demand_is_never_replaced_by_an_available_kind() {
        let resources = snapshot(
            1_000,
            vec![lane(
                4,
                BuildingKind::Fabricator,
                Vec::new(),
                vec![UnitKind::Bombard],
                ProducerEgress::Open,
            )],
        );

        let schedule = plan_production(
            &resources,
            &[demand(UnitKind::Avalanche, 1)],
            Tick::MAX,
            1_000,
        );

        assert!(schedule.appends.is_empty());
        assert!(schedule.satisfied.is_empty());
        assert_eq!(schedule.unmet, vec![demand(UnitKind::Avalanche, 1)]);
        assert_eq!(schedule.spent, 0);
        assert_eq!(schedule.next_unfunded_cost, None);
    }

    #[test]
    fn caller_order_is_priority_even_when_a_later_kind_would_finish_first() {
        let resources = snapshot(
            1_000,
            vec![lane(
                5,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Kestrel, UnitKind::Buzzard],
                ProducerEgress::Unknown,
            )],
        );

        let slower_first = plan_production(
            &resources,
            &[demand(UnitKind::Buzzard, 1), demand(UnitKind::Kestrel, 1)],
            Tick::MAX,
            UnitKind::Buzzard.stats().cost,
        );
        let faster_first = plan_production(
            &resources,
            &[demand(UnitKind::Kestrel, 1), demand(UnitKind::Buzzard, 1)],
            Tick::MAX,
            UnitKind::Buzzard.stats().cost,
        );

        assert_eq!(
            slower_first
                .appends
                .iter()
                .map(|append| append.kind)
                .collect::<Vec<_>>(),
            vec![UnitKind::Buzzard]
        );
        assert_eq!(
            faster_first
                .appends
                .iter()
                .map(|append| append.kind)
                .collect::<Vec<_>>(),
            vec![UnitKind::Kestrel]
        );
        assert_eq!(slower_first.satisfied, vec![demand(UnitKind::Buzzard, 1)]);
        assert_eq!(faster_first.satisfied, vec![demand(UnitKind::Kestrel, 1)]);
    }

    #[test]
    fn repeated_kinds_remain_separate_priority_tranches() {
        let resources = snapshot(
            1_000,
            vec![lane(
                5,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Kestrel, UnitKind::Buzzard],
                ProducerEgress::Unknown,
            )],
        );
        let demands = [
            demand(UnitKind::Buzzard, 1),
            demand(UnitKind::Kestrel, 1),
            demand(UnitKind::Buzzard, 1),
        ];

        let schedule = plan_production(
            &resources,
            &demands,
            Tick::MAX,
            UnitKind::Buzzard.stats().cost + UnitKind::Kestrel.stats().cost,
        );

        assert_eq!(
            schedule
                .appends
                .iter()
                .map(|append| append.kind)
                .collect::<Vec<_>>(),
            vec![UnitKind::Buzzard, UnitKind::Kestrel]
        );
        assert_eq!(schedule.satisfied, demands[..2]);
        assert_eq!(schedule.unmet, vec![demands[2]]);
    }

    #[test]
    fn infeasible_priority_demand_does_not_block_independent_work() {
        let resources = snapshot(
            1_000,
            vec![lane(
                4,
                BuildingKind::Foundry,
                Vec::new(),
                vec![UnitKind::Sentinel],
                ProducerEgress::Open,
            )],
        );

        let schedule = plan_production(
            &resources,
            &[
                demand(UnitKind::Avalanche, 1),
                demand(UnitKind::Sentinel, 1),
            ],
            Tick::MAX,
            1_000,
        );

        assert_eq!(schedule.appends.len(), 1);
        assert_eq!(schedule.appends[0].kind, UnitKind::Sentinel);
        assert_eq!(schedule.satisfied, vec![demand(UnitKind::Sentinel, 1)]);
        assert_eq!(schedule.unmet, vec![demand(UnitKind::Avalanche, 1)]);
    }
}
