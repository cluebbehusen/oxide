//! Owned, budgeted coverage fields independent of observational route caches.

use super::{Progress, WorkBudget};
use crate::bot::navigation::{KnownGrid, approaches::ApproachField, distance_work::DistanceWork};
use chassis::grid::TilePos;
use std::{collections::BTreeMap, sync::Arc};

const RETAINED_FIELDS: usize = 16;
const READY_BYTES: usize = 32 * 1024 * 1024;
const LIFETIME: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Job {
    started: u64,
    used: u64,
    open: Vec<bool>,
    traversal: Option<DistanceWork>,
    ready: Option<Arc<ApproachField>>,
}

impl Job {
    fn advance(
        &mut self,
        generation: &Generation,
        goals: &[TilePos],
        budget: &mut WorkBudget,
    ) -> Progress<Arc<ApproachField>> {
        if let Some(field) = &self.ready {
            return Progress::Ready(Arc::clone(field));
        }
        if self.traversal.is_none() {
            while self.open.len() < generation.blocked.len() {
                if !budget.charge(1) {
                    return Progress::Deferred;
                }
                self.open.push(!generation.blocked[self.open.len()]);
            }
            self.traversal = Some(DistanceWork::new(
                generation.width,
                generation.height,
                std::mem::take(&mut self.open),
                goals.iter().copied(),
            ));
        }
        if self.traversal.as_mut().unwrap().advance(budget) == Progress::Deferred {
            return Progress::Deferred;
        }
        let field = Arc::new(ApproachField::from_distances(
            generation.width,
            generation.height,
            self.traversal.take().unwrap().into_distances(),
        ));
        self.ready = Some(Arc::clone(&field));
        Progress::Ready(field)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Generation {
    width: i32,
    height: i32,
    blocked: Vec<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ApproachPreparation {
    generation: Option<Generation>,
    jobs: BTreeMap<Vec<TilePos>, Job>,
    next_pending: usize,
}

impl ApproachPreparation {
    pub(super) fn counts(&self) -> (usize, usize) {
        (
            self.jobs.values().filter(|job| job.ready.is_none()).count(),
            self.jobs.len(),
        )
    }

    pub(super) fn resume_pending(&mut self, tick: u64, budget: &mut WorkBudget) {
        self.expire(tick);
        let Some(generation) = &self.generation else {
            return;
        };
        let mut pending = self
            .jobs
            .iter_mut()
            .filter(|(_, job)| job.ready.is_none())
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return;
        }
        let first = self.next_pending % pending.len();
        self.next_pending = (first + 1) % pending.len();
        pending.rotate_left(first);
        for (goals, job) in pending {
            budget.run_slice(16_000, |slice| job.advance(generation, goals, slice));
        }
        self.trim_ready();
    }

    fn expire(&mut self, tick: u64) {
        self.jobs
            .retain(|_, job| job.ready.is_some() || tick.saturating_sub(job.started) < LIFETIME);
    }

    pub(super) fn advance(
        &mut self,
        tick: u64,
        grid: KnownGrid<'_>,
        goals: &[TilePos],
        budget: &mut WorkBudget,
    ) -> Progress<Arc<ApproachField>> {
        let (width, height) = grid.dimensions();
        if self.generation.as_ref().is_none_or(|generation| {
            generation.width != width
                || generation.height != height
                || generation.blocked != grid.blocked()
        }) {
            self.generation = Some(Generation {
                width,
                height,
                blocked: grid.blocked().to_vec(),
            });
            self.jobs.clear();
            self.next_pending = 0;
        }
        self.expire(tick);
        let mut goals = goals.to_vec();
        goals.sort_unstable_by_key(|tile| (tile.y, tile.x));
        goals.dedup();
        if !self.jobs.contains_key(&goals) && self.counts().0 == RETAINED_FIELDS {
            let victim = self
                .jobs
                .iter()
                .filter(|(_, job)| job.ready.is_none())
                .min_by_key(|(key, job)| (job.used, *key))
                .unwrap()
                .0
                .clone();
            self.jobs.remove(&victim);
        }
        let job = self.jobs.entry(goals.clone()).or_insert_with(|| Job {
            started: tick,
            used: tick,
            open: Vec::new(),
            traversal: None,
            ready: None,
        });
        job.used = tick;
        let result = job.advance(self.generation.as_ref().unwrap(), &goals, budget);
        self.trim_ready();
        result
    }

    fn trim_ready(&mut self) {
        let cells = self
            .generation
            .as_ref()
            .map_or(0, |generation| generation.blocked.len());
        let bytes = |goals: &[TilePos]| cells * size_of::<u32>() + size_of_val(goals) + 512;
        let mut retained: usize = self
            .jobs
            .iter()
            .filter(|(_, job)| job.ready.is_some())
            .map(|(goals, _)| bytes(goals))
            .sum();
        while retained > READY_BYTES {
            let victim = self
                .jobs
                .iter()
                .filter(|(_, job)| job.ready.is_some())
                .min_by_key(|(key, job)| (job.used, *key))
                .unwrap()
                .0
                .clone();
            retained -= bytes(&victim);
            self.jobs.remove(&victim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::planning::PlanningWork;

    #[test]
    fn completed_prefixes_do_not_starve_a_projection_with_many_assets() {
        let blocked = vec![false; 1_600];
        let grid = KnownGrid::new(40, 40, &blocked).unwrap();
        let work = PlanningWork::with_allowance(8_000);
        for tick in (0..120).step_by(12) {
            let mut complete = true;
            for x in 0..20 {
                if !matches!(
                    work.approach_field(tick, grid, false, &[TilePos::new(x, 2)]),
                    Progress::Ready(_)
                ) {
                    complete = false;
                    break;
                }
            }
            assert!(work.spent() <= 8_000);
            if complete {
                assert_eq!(work.stats().retained_approach_fields, 20);
                return;
            }
        }
        panic!("a completed prefix must not be repeatedly evicted to make room for the next asset");
    }

    #[test]
    fn approach_work_resumes_clones_and_keeps_ground_and_air_independent() {
        let blocked = vec![false; 240];
        let mut wall = blocked.clone();
        for y in 0..12 {
            wall[y * 20 + 10] = true;
        }
        let ground = KnownGrid::new(20, 12, &wall).unwrap();
        let air = KnownGrid::new(20, 12, &blocked).unwrap();
        let goals = [TilePos::new(18, 2)];
        let work = PlanningWork::with_allowance(240);
        assert!(matches!(
            work.approach_field(0, ground, false, &goals),
            Progress::Deferred
        ));
        assert!(matches!(
            work.approach_field(0, air, true, &goals),
            Progress::Deferred
        ));
        assert_eq!(work.stats().pending_approach_fields, 2);
        let clone = work.clone();
        for tick in (12..120).step_by(12) {
            work.begin(tick);
            clone.begin(tick);
            assert_eq!(work, clone);
            assert!(work.spent() <= 120);
            if work.stats().pending_approach_fields == 0 {
                let Progress::Ready(ground_field) =
                    work.approach_field(tick, ground, false, &goals)
                else {
                    panic!("ground field finished");
                };
                let Progress::Ready(air_field) = work.approach_field(tick, air, true, &goals)
                else {
                    panic!("air field finished");
                };
                let start = [TilePos::new(2, 2)];
                assert!(ground_field.path(&start).is_none());
                assert!(air_field.path(&start).is_some());
                return;
            }
        }
        panic!("both domains must progress before expiry");
    }

    #[test]
    fn changed_knowledge_invalidates_ready_approaches_and_pending_storage_is_bounded() {
        let open = vec![false; 60];
        let mut blocked = open.clone();
        for y in 0..6 {
            blocked[y * 10 + 5] = true;
        }
        let goals = [TilePos::new(8, 2)];
        let work = PlanningWork::with_allowance(240);
        let Progress::Ready(field) =
            work.approach_field(0, KnownGrid::new(10, 6, &open).unwrap(), false, &goals)
        else {
            panic!("small field finishes");
        };
        assert!(field.path(&[TilePos::new(2, 2)]).is_some());
        let Progress::Ready(field) =
            work.approach_field(12, KnownGrid::new(10, 6, &blocked).unwrap(), false, &goals)
        else {
            panic!("replacement finishes");
        };
        assert!(field.path(&[TilePos::new(2, 2)]).is_none());
        let empty = PlanningWork::with_allowance(0);
        for x in 0..40 {
            assert!(matches!(
                empty.approach_field(
                    0,
                    KnownGrid::new(10, 6, &open).unwrap(),
                    false,
                    &[TilePos::new(x, 2)]
                ),
                Progress::Deferred
            ));
            assert!(empty.stats().retained_approach_fields <= RETAINED_FIELDS);
        }
        assert_eq!(empty.spent(), 0);
        empty.begin(120);
        assert_eq!(empty.stats().pending_approach_fields, 0);
    }
}
