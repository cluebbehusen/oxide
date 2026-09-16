//! Resumable exact production search. A yielded prefix is never a rejection.

use super::*;
use crate::bot::planning::{Progress, WorkBudget};
use std::collections::BTreeMap;

#[cfg(test)]
thread_local! {
    pub(super) static SEARCH_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Solution {
    pub producers: Vec<ProducerPlanningProjection>,
    pub schedule: Vec<ScheduledProducerJob>,
    pub capital: Vec<CapitalFundingAssignment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    producers: Vec<ProducerPlanningProjection>,
    remaining: Vec<bool>,
    schedule: Vec<ScheduledProducerJob>,
    state: Option<ProductionSearchState>,
    preparation: Option<PlacementPreparation>,
    placements: BTreeMap<PlacementKey, ProductionPlacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Continuation {
    frames: Vec<Frame>,
    constructive: bool,
}

impl Continuation {
    pub fn new(
        producers: &[ProducerPlanningProjection],
        remaining: &[bool],
        schedule: &[ScheduledProducerJob],
    ) -> Self {
        Self {
            constructive: false,
            frames: vec![Frame {
                producers: producers.to_vec(),
                remaining: remaining.to_vec(),
                schedule: schedule.to_vec(),
                state: None,
                preparation: None,
                placements: BTreeMap::new(),
            }],
        }
    }

    pub fn constructive(producers: &[ProducerPlanningProjection], jobs: usize) -> Self {
        Self {
            constructive: true,
            ..Self::new(producers, &vec![true; jobs], &[])
        }
    }

    pub fn discard(&mut self) {
        self.frames.clear();
    }

    pub fn advance(
        &mut self,
        search: &mut ProductionPortfolioSearch<'_>,
        budget: &mut WorkBudget,
    ) -> Progress<Solution> {
        while let Some(frame) = self.frames.last_mut() {
            if let Some(preparation) = frame.preparation.as_mut() {
                match preparation.advance(
                    search,
                    &frame.producers,
                    &frame.remaining,
                    &frame.schedule,
                    budget,
                    self.constructive,
                ) {
                    Progress::Ready(placements) => {
                        frame.placements = placements;
                        frame.preparation = None;
                    }
                    Progress::Deferred => return Progress::Deferred,
                    Progress::ProvenInfeasible => {
                        unreachable!("placement enumeration returns an empty set")
                    }
                }
            }
            if !budget.charge(1) {
                return Progress::Deferred;
            }
            if frame.state.is_none() {
                if frame.schedule.is_empty() && !search.optimistic_funding_fits() {
                    self.frames.pop();
                    continue;
                }
                if frame.remaining.iter().all(|remaining| !remaining) {
                    let mut funded = frame.schedule.clone();
                    let minimum_residual_scrap = if search
                        .voluntary_scrap_guard
                        .satisfier
                        .is_some_and(|satisfier| {
                            schedule_satisfies_voluntary_scrap_guard(
                                search.capacity,
                                &funded,
                                satisfier,
                            )
                        }) {
                        search.minimum_residual_scrap
                    } else {
                        search.guarded_minimum_residual_scrap
                    };
                    if let Some(capital) = assign_joint_funding(
                        JointFundingBasis {
                            capacity: search.capacity,
                            current_capital: search.current_capital,
                            minimum_residual_scrap,
                            forecast_capital: search.forecast_capital,
                            deferrable_capital: search.deferrable_capital,
                            jobs: search.jobs,
                        },
                        &mut funded,
                        search.funding_mode,
                    ) {
                        return Progress::Ready(Solution {
                            producers: frame.producers.clone(),
                            schedule: funded,
                            capital,
                        });
                    }
                    self.frames.pop();
                    continue;
                }
                if !search.remaining_fixed_jobs_fit(
                    &frame.producers,
                    &frame.remaining,
                    &frame.schedule,
                ) || !search.remaining_jobs_can_fit(
                    &frame.producers,
                    &frame.remaining,
                    &frame.schedule,
                ) {
                    self.frames.pop();
                    continue;
                }
                let state = ProductionSearchState {
                    remaining: frame.remaining.clone(),
                    producers: frame.producers.clone(),
                    cash_spend: cash_spend_timeline(&frame.schedule),
                    owner_enqueue_floors: owner_enqueue_floors(&frame.schedule),
                    voluntary_scrap_guard_satisfied: search
                        .voluntary_scrap_guard
                        .satisfier
                        .is_some_and(|satisfier| {
                            schedule_satisfies_voluntary_scrap_guard(
                                search.capacity,
                                &frame.schedule,
                                satisfier,
                            )
                        }),
                };
                if search.failed.contains(&state) {
                    search.memo_hits = search.memo_hits.saturating_add(1);
                    self.frames.pop();
                    continue;
                }
                search.explored_states = search.explored_states.saturating_add(1);
                frame.preparation = Some(PlacementPreparation::default());
                frame.state = Some(state);
                continue;
            }
            let Some((_, placement)) = frame.placements.pop_first() else {
                search.failed.insert(frame.state.take().unwrap());
                self.frames.pop();
                continue;
            };
            if self.constructive {
                frame.placements.clear();
            }
            let mut schedule = frame.schedule.clone();
            schedule.push(placement.row);
            if !combined_cash_timeline_fits(
                search.capacity,
                search.current_capital,
                search.minimum_residual_scrap,
                search.forecast_capital,
                search.deferrable_capital,
                &schedule,
            ) {
                continue;
            }
            if search.funding_mode == JointFundingMode::PreferPriority {
                let mut funded = schedule.clone();
                if assign_joint_funding(
                    JointFundingBasis {
                        capacity: search.capacity,
                        current_capital: search.current_capital,
                        minimum_residual_scrap: search.minimum_residual_scrap,
                        forecast_capital: search.forecast_capital,
                        deferrable_capital: search.deferrable_capital,
                        jobs: search.jobs,
                    },
                    &mut funded,
                    search.funding_mode,
                )
                .is_none()
                {
                    continue;
                }
            }
            let mut producers = frame.producers.clone();
            producers[placement.lane_index] = placement.lane_after;
            let mut remaining = frame.remaining.clone();
            remaining[placement.job_index] = false;
            self.frames.push(Frame {
                producers,
                remaining,
                schedule,
                state: None,
                preparation: None,
                placements: BTreeMap::new(),
            });
        }
        Progress::ProvenInfeasible
    }
}

type PlacementKey = (
    Tick,
    FundingPriority,
    ClaimOwner,
    usize,
    Tick,
    BuildingId,
    usize,
);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PlacementPreparation {
    job: usize,
    producer: usize,
    lane: Option<LaneCandidates>,
    placements: BTreeMap<PlacementKey, ProductionPlacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaneCandidates {
    index: usize,
    earliest: Tick,
    initial: bool,
    income: usize,
    income_end: usize,
    best: Option<ProductionPlacement>,
    single: bool,
}

impl PlacementPreparation {
    fn advance(
        &mut self,
        search: &ProductionPortfolioSearch<'_>,
        producers: &[ProducerPlanningProjection],
        remaining: &[bool],
        schedule: &[ScheduledProducerJob],
        budget: &mut WorkBudget,
        constructive: bool,
    ) -> Progress<BTreeMap<PlacementKey, ProductionPlacement>> {
        while self.job < search.jobs.len() {
            if !budget.charge(1) {
                return Progress::Deferred;
            }
            let job = &search.jobs[self.job];
            if let Some(lane) = self.lane.as_mut() {
                let (enqueued_at, probe) = if lane.initial {
                    lane.initial = false;
                    (lane.earliest, None)
                } else if !lane.single {
                    if lane.income == lane.income_end {
                        let best = lane.best.take();
                        self.lane = None;
                        self.producer += 1;
                        if let Some(best) = best {
                            self.insert(search, best);
                        }
                        continue;
                    }
                    let index = if constructive {
                        lane.income + (lane.income_end - lane.income) / 2
                    } else {
                        let index = lane.income;
                        lane.income += 1;
                        index
                    };
                    (
                        search.capacity.resources.forecast_income()[index].available_at,
                        Some(index),
                    )
                } else {
                    self.lane = None;
                    self.producer += 1;
                    continue;
                };
                let mut lane_after = producers[lane.index].clone();
                let Some(projected) = lane_after.append(job.claim.kind, enqueued_at) else {
                    if constructive && let Some(probe) = probe {
                        lane.income_end = probe;
                    } else {
                        self.lane = None;
                        self.producer += 1;
                    }
                    continue;
                };
                if projected.ready_at >= job.claim.ready_before
                    || job.claim.fixed_assignment().is_some_and(|fixed| {
                        fixed.enqueued_at != enqueued_at
                            || fixed.starts_at != projected.starts_at
                            || fixed.ready_at != projected.ready_at
                    })
                {
                    if constructive && let Some(probe) = probe {
                        lane.income_end = probe;
                    } else {
                        self.lane = None;
                        self.producer += 1;
                    }
                    continue;
                }
                let producer = lane_after.producer();
                let placement = ProductionPlacement {
                    job_index: self.job,
                    lane_index: lane.index,
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
                };
                if constructive {
                    let mut funded = schedule.to_vec();
                    funded.push(placement.row);
                    if !combined_cash_timeline_fits(
                        search.capacity,
                        search.current_capital,
                        search.minimum_residual_scrap,
                        search.forecast_capital,
                        search.deferrable_capital,
                        &funded,
                    ) || (search.funding_mode == JointFundingMode::PreferPriority
                        && assign_joint_funding(
                            JointFundingBasis {
                                capacity: search.capacity,
                                current_capital: search.current_capital,
                                minimum_residual_scrap: search.minimum_residual_scrap,
                                forecast_capital: search.forecast_capital,
                                deferrable_capital: search.deferrable_capital,
                                jobs: search.jobs,
                            },
                            &mut funded,
                            search.funding_mode,
                        )
                        .is_none())
                    {
                        if let Some(probe) = probe {
                            lane.income = probe + 1;
                        }
                        continue;
                    }
                    if let Some(probe) = probe {
                        lane.best = Some(placement);
                        lane.income_end = probe;
                        continue;
                    }
                    self.lane = None;
                    self.producer += 1;
                }
                self.insert(search, placement);
                continue;
            }
            if !remaining[self.job] || !search.is_frontier(self.job, remaining) {
                self.job += 1;
                self.producer = 0;
                continue;
            }
            let Some(&producer) = job.claim.access.producers().get(self.producer) else {
                self.job += 1;
                self.producer = 0;
                continue;
            };
            let index = producers
                .binary_search_by_key(&producer, ProducerPlanningProjection::producer)
                .expect("producer access was validated against capacity");
            let Some(slot) = producers[index].earliest_enqueue_tick(job.claim.kind) else {
                self.producer += 1;
                continue;
            };
            let owner_enqueue = schedule
                .iter()
                .filter(|row| row.owner == job.owner && row.request_ordinal < job.ordinal)
                .map(|row| row.enqueued_at)
                .max()
                .unwrap_or(0);
            let earliest = slot.max(job.claim.enqueue_not_before).max(owner_enqueue);
            let latest = search
                .capacity
                .resources
                .horizon()
                .min(job.claim.enqueue_not_after)
                .min(job.claim.ready_before.saturating_sub(1))
                .min(
                    search
                        .bounds
                        .latest()
                        .map_or(Tick::MAX, |bounds| bounds[self.job].enqueued_at),
                );
            let fixed = job.claim.fixed_assignment();
            let candidate = fixed
                .map(|fixed| fixed.enqueued_at)
                .or_else(|| search.capacity.resources.decision_at_or_after(earliest));
            match candidate.filter(|&tick| tick >= earliest && tick <= latest) {
                Some(earliest) => {
                    self.lane = Some(LaneCandidates {
                        index,
                        earliest,
                        initial: true,
                        income: search
                            .capacity
                            .resources
                            .forecast_income()
                            .partition_point(|income| income.available_at <= earliest),
                        income_end: search
                            .capacity
                            .resources
                            .forecast_income()
                            .partition_point(|income| income.available_at <= latest),
                        best: None,
                        single: fixed.is_some() || search.earliest_enqueue_dominates,
                    })
                }
                None => self.producer += 1,
            }
        }
        Progress::Ready(core::mem::take(&mut self.placements))
    }

    fn insert(&mut self, search: &ProductionPortfolioSearch<'_>, placement: ProductionPlacement) {
        let job = &search.jobs[placement.job_index];
        self.placements.insert(
            (
                placement.row.enqueued_at,
                job.funding_priority,
                job.owner,
                job.ordinal,
                placement.row.starts_at,
                placement.row.producer,
                placement.job_index,
            ),
            placement,
        );
    }
}
