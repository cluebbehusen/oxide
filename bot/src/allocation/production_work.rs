//! Retained production refinements with current-observation witness validation.

use super::*;
use crate::planning::{Progress, WorkBudget};

const RETAINED_TASKS: usize = 16;
const PENDING_LIFETIME: Tick = 120;
const SLICE: usize = 4_096;
const FOREGROUND_SLICE: usize = 16_384;
const TASK_WORK: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Task {
    started_at: Tick,
    used_at: Tick,
    capacity: AllocationCapacity,
    claims: ClaimState,
    bounds: std::sync::Arc<production_bounds::ProductionBounds>,
    continuation: production_search::Continuation,
    failed: BTreeSet<ProductionSearchState>,
    explored: usize,
    memo_hits: usize,
    funding_mode: JointFundingMode,
    constructing: bool,
    spent: usize,
    result: Progress<production_search::Solution>,
}

impl Task {
    fn new(capacity: &AllocationCapacity, claims: &ClaimState) -> Self {
        Self {
            started_at: capacity.resources.observed_at(),
            used_at: capacity.resources.observed_at(),
            capacity: capacity.clone(),
            claims: claims.clone(),
            bounds: production_bounds::ProductionBounds::new(
                &claims.producer_jobs,
                capacity.resources.producers(),
            )
            .into(),
            continuation: production_search::Continuation::constructive(
                capacity.resources.producers(),
                claims.producer_jobs.len(),
            ),
            failed: BTreeSet::new(),
            explored: 0,
            memo_hits: 0,
            funding_mode: JointFundingMode::PreferPriority,
            constructing: true,
            spent: 0,
            result: Progress::Deferred,
        }
    }

    fn same_request(&self, capacity: &AllocationCapacity, claims: &ClaimState) -> bool {
        let Some(elapsed) = capacity
            .resources
            .observed_at()
            .checked_sub(self.started_at)
        else {
            return false;
        };
        let time_matches = |prior: Tick, current: Tick| {
            prior == current || prior.checked_add(elapsed) == Some(current)
        };
        let priority_matches = |prior: FundingPriority, current: FundingPriority| {
            prior.tier == current.tier
                && prior.order == current.order
                && prior.owner == current.owner
                && time_matches(prior.accepted_at, current.accepted_at)
        };
        if matches!(self.result, Progress::Exhausted)
            && !self
                .capacity
                .resources
                .same_production_basis(&capacity.resources)
        {
            return false;
        }
        let prior = &self.claims;
        prior.current_scrap == claims.current_scrap
            && prior.minimum_residual_scrap == claims.minimum_residual_scrap
            && prior.voluntary_scrap_guard == claims.voluntary_scrap_guard
            && prior.actors == claims.actors
            && prior.sites == claims.sites
            && prior.buildings == claims.buildings
            && prior.paid_queue == claims.paid_queue
            && prior.producer_jobs.len() == claims.producer_jobs.len()
            && prior
                .producer_jobs
                .iter()
                .zip(&claims.producer_jobs)
                .all(|(prior, current)| {
                    prior.owner == current.owner
                        && prior.ordinal == current.ordinal
                        && prior.claim.kind == current.claim.kind
                        && prior.claim.access == current.claim.access
                        && prior.claim.funding == current.claim.funding
                        && priority_matches(prior.funding_priority, current.funding_priority)
                        && time_matches(
                            prior.claim.enqueue_not_before,
                            current.claim.enqueue_not_before,
                        )
                        && time_matches(
                            prior.claim.enqueue_not_after,
                            current.claim.enqueue_not_after,
                        )
                        && time_matches(prior.claim.ready_before, current.claim.ready_before)
                })
            && prior.forecast_scrap.len() == claims.forecast_scrap.len()
            && prior
                .forecast_scrap
                .iter()
                .zip(&claims.forecast_scrap)
                .all(|(prior, current)| {
                    prior.amount == current.amount && time_matches(prior.through, current.through)
                })
            && prior.deferrable_capital.len() == claims.deferrable_capital.len()
            && prior
                .deferrable_capital
                .iter()
                .zip(&claims.deferrable_capital)
                .all(|(prior, current)| {
                    prior.owner == current.owner
                        && prior.claim.amount == current.claim.amount
                        && time_matches(prior.claim.through, current.claim.through)
                        && priority_matches(prior.funding_priority, current.funding_priority)
                })
    }

    fn advance(&mut self, budget: &mut WorkBudget) {
        if !matches!(self.result, Progress::Deferred) {
            return;
        }
        loop {
            let phase = (self.constructing, self.funding_mode);
            let before = budget.spent();
            budget.run_slice(TASK_WORK - self.spent, |slice| self.advance_phase(slice));
            self.spent += budget.spent() - before;
            if matches!(self.result, Progress::Deferred)
                && TASK_WORK - self.spent < self.work_cost()
            {
                self.result = Progress::Exhausted;
                self.continuation.discard();
                self.failed.clear();
                return;
            }
            if phase == (self.constructing, self.funding_mode)
                || !matches!(self.result, Progress::Deferred)
            {
                return;
            }
        }
    }

    fn work_cost(&self) -> usize {
        1 + self.claims.producer_jobs.len() * 2
            + self.capacity.resources.producers().len() * oxide_sim::stats::QUEUE_CAP
            + self.claims.deferrable_capital.len() * self.claims.producer_jobs.len().max(1)
    }

    fn pending(&self) -> bool {
        matches!(self.result, Progress::Deferred)
    }

    fn advance_phase(&mut self, budget: &mut WorkBudget) {
        if !matches!(self.result, Progress::Deferred) {
            return;
        }
        let mut search = ProductionPortfolioSearch::for_claims(
            &self.capacity,
            &self.claims,
            self.funding_mode,
            self.bounds.clone(),
        );
        search.failed = core::mem::take(&mut self.failed);
        search.explored_states = self.explored;
        search.memo_hits = self.memo_hits;
        let work_cost = self.work_cost();
        self.result = budget.weighted(work_cost, |units| {
            self.continuation.advance(&mut search, units)
        });
        self.failed = search.failed;
        self.explored = search.explored_states;
        self.memo_hits = search.memo_hits;
        if matches!(self.result, Progress::ProvenInfeasible) && self.constructing {
            self.constructing = false;
            self.failed.clear();
            self.continuation = production_search::Continuation::new(
                self.capacity.resources.producers(),
                &vec![true; self.claims.producer_jobs.len()],
                &[],
            );
            self.result = Progress::Deferred;
        } else if matches!(self.result, Progress::ProvenInfeasible)
            && self.funding_mode == JointFundingMode::PreferPriority
        {
            self.funding_mode = JointFundingMode::PreserveCompatiblePortfolio;
            self.constructing = true;
            self.failed.clear();
            self.continuation = production_search::Continuation::constructive(
                self.capacity.resources.producers(),
                self.claims.producer_jobs.len(),
            );
            self.result = Progress::Deferred;
        }
    }

    fn current_witness(
        &self,
        capacity: &AllocationCapacity,
        claims: &ClaimState,
        solution: &production_search::Solution,
    ) -> Option<ResolvedClaimState> {
        let elapsed = capacity
            .resources
            .observed_at()
            .checked_sub(self.started_at)?;
        let mut witness = witness::validate(capacity, claims, &solution.schedule, elapsed)?;
        witness.search_states = self.explored;
        witness.memo_hits = self.memo_hits;
        Some(witness)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProductionWork {
    tasks: Vec<Task>,
    next_pending: usize,
}

impl ProductionWork {
    pub(crate) fn valid_checkpoint(&self, tick: Tick) -> bool {
        self.tasks.len() <= RETAINED_TASKS
            && self.tasks.iter().all(|task| {
                let producers = task.capacity.resources.producers();
                task.started_at <= task.used_at
                    && task.used_at <= tick
                    && task.spent <= TASK_WORK
                    && (!matches!(task.result, Progress::Exhausted)
                        || TASK_WORK - task.spent < task.work_cost())
                    && task.capacity.resources.valid_checkpoint(tick)
                    && task.claims.producer_jobs.iter().all(|job| {
                        job.claim.access.producers().iter().all(|id| {
                            producers
                                .binary_search_by_key(id, ProducerPlanningProjection::producer)
                                .is_ok()
                        }) && job.claim.fixed_assignment().is_none_or(|assignment| {
                            producers
                                .binary_search_by_key(
                                    &assignment.producer,
                                    ProducerPlanningProjection::producer,
                                )
                                .is_ok()
                        })
                    })
                    && *task.bounds
                        == production_bounds::ProductionBounds::new(
                            &task.claims.producer_jobs,
                            producers,
                        )
                    && task
                        .continuation
                        .valid_checkpoint(&task.capacity, &task.claims, tick)
            })
    }

    pub(crate) fn counts(&self) -> (usize, usize) {
        (
            self.tasks.iter().filter(|task| task.pending()).count(),
            self.tasks.len(),
        )
    }

    pub(crate) fn resume_pending(&mut self, tick: Tick, budget: &mut WorkBudget) {
        self.tasks
            .retain(|task| tick.saturating_sub(task.started_at) < PENDING_LIFETIME);
        if self.tasks.is_empty() {
            return;
        }
        let first = self.next_pending % self.tasks.len();
        self.next_pending = (first + 1) % self.tasks.len();
        for offset in 0..self.tasks.len() {
            let index = (first + offset) % self.tasks.len();
            budget.run_slice(SLICE, |slice| self.tasks[index].advance(slice));
        }
    }

    pub(crate) fn resolve(
        &mut self,
        capacity: &AllocationCapacity,
        claims: &ClaimState,
        budget: &mut WorkBudget,
    ) -> Result<Progress<ResolvedClaimState>, AllocationConflict> {
        claims.validate_production_bounds(capacity)?;
        let tick = capacity.resources.observed_at();
        self.tasks
            .retain(|task| tick.saturating_sub(task.started_at) < PENDING_LIFETIME);
        let index = if let Some(index) = self
            .tasks
            .iter()
            .position(|task| task.same_request(capacity, claims))
        {
            index
        } else {
            for task in &mut self.tasks {
                if let Progress::Ready(solution) = &task.result
                    && solution.schedule.len() == claims.producer_jobs.len()
                    && budget.charge(1 + claims.producer_jobs.len().saturating_pow(2))
                    && let Some(witness) = task.current_witness(capacity, claims, solution)
                {
                    task.used_at = tick;
                    return Ok(Progress::Ready(witness));
                }
            }
            if self.tasks.len() == RETAINED_TASKS {
                let victim = self
                    .tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, task)| !task.pending())
                    .min_by_key(|(index, task)| (task.used_at, *index))
                    .map(|(index, _)| index);
                let Some(victim) = victim else {
                    return Ok(Progress::Deferred);
                };
                self.tasks.remove(victim);
            }
            if !budget.charge(1 + claims.producer_jobs.len() + capacity.resources.producers().len())
            {
                return Ok(Progress::Deferred);
            }
            self.tasks.push(Task::new(capacity, claims));
            self.tasks.len() - 1
        };
        let task = &mut self.tasks[index];
        task.used_at = tick;
        budget.run_slice(FOREGROUND_SLICE, |slice| task.advance(slice));
        match &task.result {
            Progress::Ready(solution) => {
                if let Some(current) = task.current_witness(capacity, claims, solution) {
                    return Ok(Progress::Ready(current));
                }
            }
            Progress::ProvenInfeasible if task.capacity == *capacity && task.claims == *claims => {
                return Err(producer_schedule_conflict(&claims.producer_jobs));
            }
            Progress::Deferred => return Ok(Progress::Deferred),
            Progress::Exhausted => return Ok(Progress::Exhausted),
            Progress::ProvenInfeasible => {}
        }
        if budget.charge(1 + claims.producer_jobs.len() + capacity.resources.producers().len()) {
            *task = Task::new(capacity, claims);
        }
        Ok(Progress::Deferred)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourcePlanningFixture;
    use oxide_sim::stats::QUEUE_CAP;

    fn fixture(tick: Tick, jobs: usize) -> (AllocationCapacity, ClaimState) {
        let producer = BuildingId(7);
        let resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: 10_000,
            producers: vec![
                ProducerPlanningProjection::fixture(
                    producer,
                    tick,
                    1,
                    tick,
                    vec![tick; QUEUE_CAP],
                    vec![UnitKind::Sentinel],
                )
                .unwrap(),
            ],
            ..ResourcePlanningFixture::empty(tick..=tick + 10_000, 1)
        })
        .unwrap();
        let capacity = AllocationCapacity::fixture(resources);
        let owner = ClaimOwner::Proposal(ProposalKey::StandingForce(StandingForceKey::fixture(
            UnitKind::Sentinel,
        )));
        let claims = ClaimState {
            producer_jobs: (0..jobs)
                .map(|ordinal| OwnedProducerJob {
                    claim: ProducerJobClaim::flexible(
                        UnitKind::Sentinel,
                        tick,
                        tick + 10_000,
                        vec![producer],
                    ),
                    owner,
                    ordinal,
                    funding_priority: FundingPriority::fallback(owner),
                })
                .collect(),
            ..ClaimState::default()
        };
        (capacity, claims)
    }

    #[test]
    fn fixed_witnesses_preserve_exact_timing_without_searching_again() {
        for jobs in 0..12 {
            let (capacity, mut claims) = fixture(0, jobs);
            let expected = claims.resolve(&capacity).unwrap();
            for job in &mut claims.producer_jobs {
                let row = expected
                    .producer_schedule
                    .iter()
                    .find(|row| row.request_ordinal == job.ordinal)
                    .unwrap();
                job.claim = ProducerJobClaim::fixed(
                    row.producer,
                    row.kind,
                    row.enqueued_at,
                    row.starts_at,
                    row.ready_at,
                    row.ready_before,
                );
            }
            let before = production_search::SEARCH_CALLS.get();
            let actual = claims.resolve(&capacity).unwrap();
            assert_eq!(production_search::SEARCH_CALLS.get(), before);
            assert_eq!(actual.producer_schedule, expected.producer_schedule);
            assert_eq!(actual.capital_assignments, expected.capital_assignments);
            if jobs >= 2 {
                let mut repeated = actual.producer_schedule.clone();
                repeated[1] = repeated[0];
                assert!(witness::validate(&capacity, &claims, &repeated, 0).is_none());
            }
        }
    }

    #[test]
    fn sliced_production_matches_the_exact_witness_and_retains_its_progress() {
        let (capacity, claims) = fixture(0, 12);
        let expected = claims.resolve(&capacity).unwrap();
        let mut work = ProductionWork::default();
        let mut slices = 0;
        let actual = loop {
            slices += 1;
            let mut budget = WorkBudget::new(1_024);
            let result = work.resolve(&capacity, &claims, &mut budget).unwrap();
            assert!(budget.spent() <= 1_024);
            if let Progress::Ready(actual) = result {
                break actual;
            }
            assert_eq!(result, Progress::Deferred);
            let mut bytes = Vec::new();
            ciborium::into_writer(&work, &mut bytes).unwrap();
            let restored: ProductionWork = ciborium::from_reader(bytes.as_slice()).unwrap();
            assert_eq!(work, restored);
            assert!(restored.valid_checkpoint(0));
            let mut cloned = work.clone();
            let mut resumed = restored.clone();
            let mut clone_budget = WorkBudget::new(1_024);
            let mut restore_budget = WorkBudget::new(1_024);
            assert_eq!(
                cloned.resolve(&capacity, &claims, &mut clone_budget),
                resumed.resolve(&capacity, &claims, &mut restore_budget),
            );
            assert_eq!(cloned, resumed);
            assert_eq!(clone_budget.spent(), restore_budget.spent());
            work = restored;
            assert!(slices < 200);
        };
        assert!(slices > 1);
        assert_eq!(actual, expected);
        let mut cloned = work.clone();
        for copy in [&mut work, &mut cloned] {
            let mut budget = WorkBudget::new(0);
            assert_eq!(
                copy.resolve(&capacity, &claims, &mut budget).unwrap(),
                Progress::Ready(expected.clone())
            );
            assert_eq!(budget.spent(), 0);
        }
    }

    #[test]
    fn pending_work_can_finish_between_decisions_but_current_deadlines_still_apply() {
        let (capacity, claims) = fixture(0, 8);
        let mut work = ProductionWork::default();
        assert_eq!(
            work.resolve(&capacity, &claims, &mut WorkBudget::new(16))
                .unwrap(),
            Progress::Deferred
        );
        let mut cloned = work.clone();
        for copy in [&mut work, &mut cloned] {
            copy.resume_pending(12, &mut WorkBudget::new(4_096));
            let (current, current_claims) = fixture(12, 8);
            let result = copy
                .resolve(&current, &current_claims, &mut WorkBudget::new(0))
                .unwrap();
            let Progress::Ready(result) = result else {
                panic!("{result:?}");
            };
            assert!(
                result
                    .producer_schedule
                    .iter()
                    .all(|job| job.enqueued_at >= 12)
            );
            assert_eq!(
                result.producer_schedule,
                current_claims.resolve(&current).unwrap().producer_schedule
            );

            let mut expired = current_claims.clone();
            for job in &mut expired.producer_jobs {
                job.claim.ready_before = 13;
            }
            assert!(
                copy.resolve(&current, &expired, &mut WorkBudget::new(0))
                    .is_err()
            );
        }
        assert_eq!(work, cloned);
    }

    #[test]
    fn stale_feasibility_does_not_bypass_current_funding_or_lane_access() {
        let (capacity, claims) = fixture(0, 2);
        let mut work = ProductionWork::default();
        assert!(matches!(
            work.resolve(&capacity, &claims, &mut WorkBudget::new(4_096))
                .unwrap(),
            Progress::Ready(_)
        ));
        let mut reserved = claims.clone();
        reserved.current_scrap = 10_000;
        assert!(matches!(
            work.resolve(&capacity, &reserved, &mut WorkBudget::new(0)),
            Err(AllocationConflict::ProductionFunding { .. })
        ));
        let (mut current, current_claims) = fixture(12, 2);
        current.resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: 10_000,
            producers: vec![
                ProducerPlanningProjection::fixture(
                    BuildingId(7),
                    12,
                    1,
                    12,
                    vec![12; QUEUE_CAP],
                    vec![],
                )
                .unwrap(),
            ],
            ..ResourcePlanningFixture::empty(12..=10_012, 1)
        })
        .unwrap();
        assert_eq!(
            work.resolve(&current, &current_claims, &mut WorkBudget::new(0))
                .unwrap(),
            Progress::Deferred
        );
    }

    #[test]
    fn zero_work_and_full_pending_storage_defer_without_a_false_conflict() {
        let (capacity, mut claims) = fixture(0, 12);
        let mut work = ProductionWork::default();
        assert_eq!(
            work.resolve(&capacity, &claims, &mut WorkBudget::new(0))
                .unwrap(),
            Progress::Deferred
        );
        assert_eq!(work.counts(), (0, 0));
        for index in 0..RETAINED_TASKS {
            for (ordinal, job) in claims.producer_jobs.iter_mut().enumerate() {
                job.ordinal = index * 20 + ordinal;
            }
            work.tasks.push(Task::new(&capacity, &claims));
        }
        for (ordinal, job) in claims.producer_jobs.iter_mut().enumerate() {
            job.ordinal = 1_000 + ordinal;
        }
        assert_eq!(
            work.resolve(&capacity, &claims, &mut WorkBudget::new(4_096))
                .unwrap(),
            Progress::Deferred
        );
        assert_eq!(work.counts(), (RETAINED_TASKS, RETAINED_TASKS));
        work.resume_pending(PENDING_LIFETIME, &mut WorkBudget::new(0));
        assert_eq!(work.counts(), (0, 0));
    }

    #[test]
    fn repeated_production_queries_share_the_controllers_remaining_allowance() {
        let (capacity, claims) = fixture(0, 12);
        let work = crate::planning::PlanningWork::with_allowance(1_024);
        for _ in 0..100 {
            assert_eq!(
                work.production(0, &capacity, &claims).unwrap(),
                Progress::Deferred
            );
            assert!(work.stats().spent <= 1_024);
        }
        assert_eq!(work.stats().pending_production, 1);
        assert_eq!(work.stats().retained_production, 1);
        let mut progressed = false;
        for tick in 1..PENDING_LIFETIME {
            let (current, claims) = fixture(tick, 12);
            if matches!(
                work.production(tick, &current, &claims).unwrap(),
                Progress::Ready(_)
            ) {
                progressed = true;
                break;
            }
        }
        assert!(
            progressed,
            "background work must survive changes to the observation tick"
        );
    }

    #[test]
    fn bounded_slices_match_exact_small_deadline_and_funding_cases() {
        for mask in 0..64 {
            let (capacity, mut claims) = fixture(0, 3);
            let duration = Tick::from(UnitKind::Sentinel.stats().train_ticks);
            for (index, job) in claims.producer_jobs.iter_mut().enumerate() {
                job.claim.enqueue_not_before = if mask & (1 << index) == 0 {
                    0
                } else {
                    duration
                };
                job.claim.ready_before = duration * if mask & (8 << index) == 0 { 2 } else { 5 };
            }
            for reserve in [0, 9_900, 9_999] {
                claims.current_scrap = reserve;
                let expected = claims.resolve(&capacity);
                let mut work = ProductionWork::default();
                let actual = (0..200)
                    .find_map(|_| {
                        match work.resolve(&capacity, &claims, &mut WorkBudget::new(1_024)) {
                            Ok(Progress::Ready(result)) => Some(Ok(result)),
                            Err(error) => Some(Err(error)),
                            Ok(Progress::Deferred) => None,
                            Ok(Progress::ProvenInfeasible) => unreachable!(),
                            Ok(Progress::Exhausted) => {
                                panic!("compact fixture exhausted its search allowance")
                            }
                        }
                    })
                    .expect("small searches terminate within the test allowance");
                assert_eq!(
                    actual.is_ok(),
                    expected.is_ok(),
                    "mask={mask}, reserve={reserve}"
                );
                if let Ok(actual) = actual {
                    assert_eq!(
                        actual.producer_schedule,
                        expected.unwrap().producer_schedule
                    );
                }
            }
        }
    }

    #[test]
    fn constructive_funding_seeks_the_income_boundary_instead_of_enumerating_the_horizon() {
        use crate::resources::ForecastAvailability;
        let (basis, claims) = fixture(0, 1);
        let cost = UnitKind::Sentinel.stats().cost;
        for bank in [0, cost - 1, cost] {
            let capacity = AllocationCapacity::fixture(
                ResourcePlanningProjection::fixture(ResourcePlanningFixture {
                    current_scrap: bank,
                    forecast_income: (1..=1_000)
                        .map(|available_at| ForecastAvailability {
                            available_at,
                            amount: 1,
                        })
                        .collect(),
                    producers: basis.resources.producers().to_vec(),
                    ..ResourcePlanningFixture::empty(0..=10_000, 1)
                })
                .unwrap(),
            );
            let expected = claims.resolve(&capacity).unwrap();
            let mut budget = WorkBudget::new(512);
            let actual = ProductionWork::default()
                .resolve(&capacity, &claims, &mut budget)
                .unwrap();
            let Progress::Ready(actual) = actual else {
                panic!("{actual:?}");
            };
            assert_eq!(actual.producer_schedule, expected.producer_schedule);
            assert!(budget.spent() < 512);
            assert_eq!(
                actual.producer_schedule[0].enqueued_at,
                Tick::from(cost - bank)
            );
        }
    }

    #[test]
    fn exhausted_repair_allowance_releases_search_storage_without_proving_infeasibility() {
        let (capacity, claims) = fixture(0, 12);
        assert!(claims.resolve(&capacity).is_ok());
        let mut task = Task::new(&capacity, &claims);
        task.spent = TASK_WORK;
        task.advance(&mut WorkBudget::new(TASK_WORK));
        assert!(!task.pending());
        assert_eq!(task.result, Progress::Exhausted);
        assert!(task.failed.is_empty());
        let mut work = ProductionWork {
            tasks: vec![task],
            next_pending: 0,
        };
        let mut budget = WorkBudget::new(TASK_WORK);
        assert_eq!(
            work.resolve(&capacity, &claims, &mut budget).unwrap(),
            Progress::Exhausted
        );
        assert_eq!(budget.spent(), 0);
        assert!(work.valid_checkpoint(0));
        let (shifted, shifted_claims) = fixture(12, 12);
        assert_eq!(
            work.resolve(&shifted, &shifted_claims, &mut WorkBudget::new(TASK_WORK))
                .unwrap(),
            Progress::Exhausted
        );
        let mut changed = shifted.clone();
        changed.resources = ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: 9_999,
            producers: shifted.resources.producers().to_vec(),
            ..ResourcePlanningFixture::empty(12..=10_012, 1)
        })
        .unwrap();
        let mut retry = WorkBudget::new(TASK_WORK);
        assert!(!matches!(
            work.resolve(&changed, &shifted_claims, &mut retry).unwrap(),
            Progress::Exhausted
        ));
        assert!(retry.spent() > 0);
        assert!(work.valid_checkpoint(12));
        let mut bytes = Vec::new();
        ciborium::into_writer(&work, &mut bytes).unwrap();
        let restored: ProductionWork = ciborium::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(restored, work);
        work.tasks[0].spent = 0;
        assert!(!work.valid_checkpoint(12));
    }
}
