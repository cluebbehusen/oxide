//! Resumable exact production search. A yielded prefix is never a rejection.

use super::*;
use crate::bot::planning::{Progress, WorkBudget};

pub(super) struct Solution {
    pub producers: Vec<ProducerPlanningProjection>,
    pub schedule: Vec<ScheduledProducerJob>,
    pub capital: Vec<CapitalFundingAssignment>,
}

struct Frame {
    producers: Vec<ProducerPlanningProjection>,
    remaining: Vec<bool>,
    schedule: Vec<ScheduledProducerJob>,
    state: Option<ProductionSearchState>,
    placements: std::vec::IntoIter<ProductionPlacement>,
}

pub(super) struct Continuation {
    frames: Vec<Frame>,
}

impl Continuation {
    pub fn new(
        producers: &[ProducerPlanningProjection],
        remaining: &[bool],
        schedule: &[ScheduledProducerJob],
    ) -> Self {
        Self {
            frames: vec![Frame {
                producers: producers.to_vec(),
                remaining: remaining.to_vec(),
                schedule: schedule.to_vec(),
                state: None,
                placements: Vec::new().into_iter(),
            }],
        }
    }

    pub fn advance(
        &mut self,
        search: &mut ProductionPortfolioSearch<'_>,
        budget: &mut WorkBudget,
    ) -> Progress<Solution> {
        while let Some(frame) = self.frames.last_mut() {
            if !budget.charge(1) {
                return Progress::Deferred;
            }
            if frame.state.is_none() {
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
                frame.placements = search
                    .placements(&frame.producers, &frame.remaining, &frame.schedule)
                    .into_iter();
                frame.state = Some(state);
                continue;
            }
            let Some(placement) = frame.placements.next() else {
                search.failed.insert(frame.state.take().unwrap());
                self.frames.pop();
                continue;
            };
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
                placements: Vec::new().into_iter(),
            });
        }
        Progress::ProvenInfeasible
    }
}
