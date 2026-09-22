//! Necessary capacity bounds prepared once for the exact FIFO scheduler.

use super::{
    OwnedProducerJob, ProducerPlanningProjection, ScheduledProducerJob, optimistic_funding_schedule,
};
use crate::ids::BuildingId;
use chassis::Tick;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProductionBounds {
    latest: Option<Vec<ScheduledProducerJob>>,
    windows: Vec<WindowBound>,
    fixed: Vec<FixedWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowBound {
    jobs: Vec<(usize, u128)>,
    producers: Vec<usize>,
    release: Tick,
    deadline: Tick,
    quantum: Tick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FixedWindow {
    job: usize,
    producer: usize,
    start: Tick,
    end: Tick,
}

impl ProductionBounds {
    pub(super) fn new(jobs: &[OwnedProducerJob], producers: &[ProducerPlanningProjection]) -> Self {
        let producer_index = |producer: &BuildingId| {
            producers
                .binary_search_by_key(producer, ProducerPlanningProjection::producer)
                .expect("producer claims were validated")
        };
        let mut fixed: Vec<_> = jobs
            .iter()
            .enumerate()
            .filter_map(|(index, job)| {
                let fixed = job.claim.fixed_assignment()?;
                Some(FixedWindow {
                    job: index,
                    producer: producer_index(&fixed.producer),
                    start: fixed.starts_at,
                    end: fixed.ready_at.saturating_add(1),
                })
            })
            .collect();
        fixed.sort_unstable_by_key(|window| (window.producer, window.start, window.job));
        let flexible: Vec<_> = jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| job.claim.fixed_assignment().is_none())
            .collect();
        let mut kinds: Vec<_> = flexible.iter().map(|(_, job)| job.claim.kind).collect();
        kinds.sort_unstable();
        kinds.dedup();
        let mut families: Vec<Vec<_>> = kinds
            .iter()
            .map(|kind| {
                flexible
                    .iter()
                    .copied()
                    .filter(|(_, job)| job.claim.kind == *kind)
                    .collect()
            })
            .collect();
        if kinds.len() > 1 {
            families.push(flexible);
        }
        let mut windows = Vec::new();
        for demands in families {
            let mut groups: Vec<_> = demands
                .iter()
                .map(|(_, job)| job.claim.access.producers())
                .collect();
            groups.sort_unstable();
            groups.dedup();
            let mut all: Vec<_> = groups
                .iter()
                .flat_map(|group| group.iter().copied())
                .collect();
            all.sort_unstable();
            all.dedup();
            groups.push(&all);
            groups.sort_unstable();
            groups.dedup();
            let mut releases: Vec<_> = demands
                .iter()
                .map(|(_, job)| job.claim.enqueue_not_before)
                .collect();
            releases.sort_unstable();
            releases.dedup();
            let mut deadlines: Vec<_> = demands
                .iter()
                .map(|(_, job)| job.claim.ready_before)
                .collect();
            deadlines.sort_unstable();
            deadlines.dedup();
            for group in groups {
                let mut seen = std::collections::BTreeSet::new();
                for &release in &releases {
                    for &deadline in &deadlines {
                        let selected: Vec<_> = demands
                            .iter()
                            .filter(|(_, job)| {
                                job.claim.enqueue_not_before >= release
                                    && job.claim.ready_before <= deadline
                                    && job
                                        .claim
                                        .access
                                        .producers()
                                        .iter()
                                        .all(|producer| group.binary_search(producer).is_ok())
                            })
                            .map(|(index, _)| *index)
                            .collect();
                        if selected.len() <= 1 || !seen.insert(selected.clone()) {
                            continue;
                        }
                        let release = selected
                            .iter()
                            .map(|&index| jobs[index].claim.enqueue_not_before)
                            .min()
                            .unwrap();
                        let deadline = selected
                            .iter()
                            .map(|&index| jobs[index].claim.ready_before)
                            .max()
                            .unwrap();
                        // Every selected job occupies a multiple of this time unit.
                        let quantum = selected
                            .iter()
                            .map(|&index| Tick::from(jobs[index].claim.kind.stats().train_ticks))
                            .reduce(gcd)
                            .unwrap();
                        windows.push(WindowBound {
                            jobs: selected
                                .into_iter()
                                .map(|index| {
                                    (
                                        index,
                                        u128::from(
                                            Tick::from(jobs[index].claim.kind.stats().train_ticks)
                                                / quantum,
                                        ),
                                    )
                                })
                                .collect(),
                            producers: group.iter().map(&producer_index).collect(),
                            release,
                            deadline,
                            quantum,
                        });
                    }
                }
            }
        }
        Self {
            latest: optimistic_funding_schedule(jobs),
            windows,
            fixed,
        }
    }

    pub(super) fn latest(&self) -> Option<&[ScheduledProducerJob]> {
        self.latest.as_deref()
    }

    pub(super) fn remaining_work_fits(
        &self,
        remaining: &[bool],
        producers: &[ProducerPlanningProjection],
    ) -> bool {
        self.windows.iter().all(|window| {
            let count = window
                .jobs
                .iter()
                .filter(|&&(index, _)| remaining[index])
                .map(|&(_, work)| work)
                .sum::<u128>();
            count <= 1
                || count
                    <= window
                        .producers
                        .iter()
                        .map(|&producer| {
                            self.free_capacity(producer, &producers[producer], remaining, window)
                        })
                        .sum::<u128>()
        })
    }

    fn free_capacity(
        &self,
        producer: usize,
        lane: &ProducerPlanningProjection,
        remaining: &[bool],
        window: &WindowBound,
    ) -> u128 {
        let mut start = window.release.max(lane.production_available_at());
        let mut slots = 0_u128;
        for fixed in self
            .fixed
            .iter()
            .filter(|fixed| fixed.producer == producer && remaining[fixed.job])
        {
            slots +=
                u128::from(fixed.start.min(window.deadline).saturating_sub(start) / window.quantum);
            start = start.max(fixed.end);
            if start >= window.deadline {
                return slots;
            }
        }
        slots + u128::from(window.deadline.saturating_sub(start) / window.quantum)
    }
}

fn gcd(mut left: Tick, mut right: Tick) -> Tick {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}
