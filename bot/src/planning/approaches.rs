//! Owned, budgeted coverage fields independent of observational route caches.

use super::{Progress, WorkBudget};
use crate::navigation::{
    BlockedRect, KnownGrid, approaches::ApproachField, distance_work::DistanceWork,
};
use crate::query_work::QueryPurpose;
use chassis::grid::TilePos;
use std::{collections::BTreeMap, sync::Arc};

const PENDING_FIELDS: usize = 16;
const READY_BYTES: usize = 32 * 1024 * 1024;
const IDLE_LIFETIME: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Job {
    query_purpose: QueryPurpose,
    used: u64,
    open: Vec<bool>,
    traversal: Option<DistanceWork>,
    ready: Option<Arc<ApproachField>>,
}

impl Job {
    fn advance(
        &mut self,
        query_purpose: QueryPurpose,
        generation: &Generation,
        goals: &[TilePos],
        overlay: Option<BlockedRect>,
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
                let index = self.open.len();
                let tile = TilePos::new(
                    (index % generation.width as usize) as i32,
                    (index / generation.width as usize) as i32,
                );
                self.open.push(
                    !generation.blocked[index] && !overlay.is_some_and(|rect| rect.contains(tile)),
                );
            }
            self.traversal = Some(DistanceWork::new(
                query_purpose,
                generation.width,
                generation.height,
                std::mem::take(&mut self.open),
                goals.iter().copied(),
            ));
        }
        if self
            .traversal
            .as_mut()
            .unwrap()
            .advance(query_purpose, budget)
            == Progress::Deferred
        {
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Generation {
    width: i32,
    height: i32,
    blocked: Vec<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ApproachPreparation {
    generation: Option<Generation>,
    jobs: BTreeMap<(Option<BlockedRect>, Vec<TilePos>), Job>,
    next_pending: usize,
}

impl ApproachPreparation {
    pub(super) fn valid_checkpoint(&self, width: i32, height: i32, tick: u64) -> bool {
        let cells = width as usize * height as usize;
        self.counts().0 <= PENDING_FIELDS
            && self.ready_bytes() <= READY_BYTES
            && match &self.generation {
                None => self.jobs.is_empty(),
                Some(generation) => {
                    generation.width == width
                        && generation.height == height
                        && generation.blocked.len() == cells
                        && self.jobs.values().all(|job| {
                            job.used <= tick
                                && job.open.len() <= cells
                                && job.traversal.as_ref().is_none_or(|work| {
                                    job.open.is_empty()
                                        && job.ready.is_none()
                                        && work.valid_checkpoint(width, height)
                                })
                                && job.ready.as_ref().is_none_or(|ready| {
                                    job.open.is_empty() && ready.valid_checkpoint(width, height)
                                })
                        })
                }
            }
    }

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
        for ((overlay, goals), job) in pending {
            budget.run_slice(16_000, |slice| {
                job.advance(job.query_purpose, generation, goals, *overlay, slice)
            });
        }
        self.trim_ready();
    }

    fn expire(&mut self, tick: u64) {
        self.jobs
            .retain(|_, job| job.ready.is_some() || tick.saturating_sub(job.used) < IDLE_LIFETIME);
    }

    pub(super) fn advance(
        &mut self,
        query_purpose: QueryPurpose,
        tick: u64,
        grid: KnownGrid<'_>,
        goals: &[TilePos],
        overlay: Option<BlockedRect>,
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
        let key = (overlay, goals);
        if !self.jobs.contains_key(&key) && self.counts().0 == PENDING_FIELDS {
            return Progress::Deferred;
        }
        let job = self.jobs.entry(key.clone()).or_insert_with(|| Job {
            query_purpose,
            used: tick,
            open: Vec::new(),
            traversal: None,
            ready: None,
        });
        job.used = tick;
        let result = job.advance(
            query_purpose,
            self.generation.as_ref().unwrap(),
            &key.1,
            overlay,
            budget,
        );
        self.trim_ready();
        result
    }

    fn field_bytes(&self, goals: &[TilePos]) -> usize {
        let cells = self
            .generation
            .as_ref()
            .map_or(0, |generation| generation.blocked.len());
        cells * size_of::<u32>() + size_of_val(goals) + 512
    }

    fn ready_bytes(&self) -> usize {
        self.jobs
            .iter()
            .filter(|(_, job)| job.ready.is_some())
            .map(|((_, goals), _)| self.field_bytes(goals))
            .sum()
    }

    fn trim_ready(&mut self) {
        let mut retained = self.ready_bytes();
        while retained > READY_BYTES {
            let victim = self
                .jobs
                .iter()
                .filter(|(_, job)| job.ready.is_some())
                .min_by_key(|(key, job)| (job.used, *key))
                .unwrap()
                .0
                .clone();
            retained -= self.field_bytes(&victim.1);
            self.jobs.remove(&victim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::PlanningWork;

    #[test]
    fn checkpoint_accepts_completed_fields_beyond_the_pending_limit() {
        let blocked = vec![false; 8 * 8];
        let grid = KnownGrid::new(8, 8, &blocked).unwrap();
        let mut work = ApproachPreparation::default();
        for index in 0..PENDING_FIELDS + 1 {
            assert!(matches!(
                work.advance(
                    QueryPurpose::NavigationTest,
                    0,
                    grid,
                    &[TilePos::new((index % 8) as i32, (index / 8) as i32)],
                    None,
                    &mut WorkBudget::new(usize::MAX),
                ),
                Progress::Ready(_)
            ));
        }
        assert_eq!(work.counts(), (0, PENDING_FIELDS + 1));
        assert!(work.valid_checkpoint(8, 8, 0));
        let restored = crate::checkpoint::round_trip(&work);
        assert!(restored.valid_checkpoint(8, 8, 0));
        let mut oversized = work.clone();
        let (_, job) = oversized.jobs.pop_first().unwrap();
        oversized.jobs.clear();
        oversized.jobs.insert(
            (
                None,
                vec![TilePos::new(0, 0); READY_BYTES / size_of::<TilePos>()],
            ),
            job,
        );
        assert!(oversized.ready_bytes() > READY_BYTES);
        assert!(!oversized.valid_checkpoint(8, 8, 0));
        for job in work.jobs.values_mut() {
            job.ready = None;
        }
        assert!(!work.valid_checkpoint(8, 8, 0));
    }

    #[test]
    fn candidate_overlays_do_not_replace_each_others_pending_work() {
        let blocked = vec![false; 12 * 8];
        let grid = KnownGrid::new(12, 8, &blocked).unwrap();
        let work = PlanningWork::with_allowance(91);
        let goal = TilePos::new(10, 4);
        let walls = [
            Some(BlockedRect {
                anchor: TilePos::new(6, 0),
                size: (1, 8),
            }),
            Some(BlockedRect {
                anchor: TilePos::new(6, 0),
                size: (1, 7),
            }),
        ];
        for tick in (0..600).step_by(12) {
            let fields: Vec<_> = walls
                .into_iter()
                .map(|wall| {
                    work.candidate_route_field(
                        QueryPurpose::NavigationTest,
                        tick,
                        grid,
                        wall,
                        &[goal],
                    )
                })
                .collect();
            assert!(work.spent() <= 91);
            if let [Progress::Ready(closed), Progress::Ready(open)] = fields.as_slice() {
                assert!(closed.path(&[TilePos::new(1, 4)]).is_none());
                let (_, path) = open.path(&[TilePos::new(1, 4)]).unwrap();
                assert!(path.contains(&TilePos::new(6, 7)));
                assert_eq!(work.stats().retained_approach_fields, 2);
                return;
            }
        }
        panic!("alternating overlays must retain their progress");
    }

    #[test]
    fn excess_admissions_preserve_pending_fields_until_they_complete() {
        let blocked = vec![false; 64 * 64];
        let grid = KnownGrid::new(64, 64, &blocked).unwrap();
        let work = PlanningWork::default();
        for tick in (0..120).step_by(12) {
            let mut completed = 0;
            for x in 0..32 {
                if matches!(
                    work.approach_field(
                        QueryPurpose::NavigationTest,
                        tick,
                        grid,
                        false,
                        &[TilePos::new(x, 2)]
                    ),
                    Progress::Ready(_)
                ) {
                    completed += 1;
                }
            }
            assert!(work.stats().pending_approach_fields <= PENDING_FIELDS);
            if completed == 32 {
                return;
            }
        }
        panic!("later requests must not evict unfinished earlier work");
    }

    #[test]
    fn completed_prefixes_do_not_starve_a_projection_with_many_assets() {
        let blocked = vec![false; 1_600];
        let grid = KnownGrid::new(40, 40, &blocked).unwrap();
        let work = PlanningWork::with_allowance(10_667);
        for tick in (0..120).step_by(12) {
            let mut complete = true;
            for x in 0..20 {
                if !matches!(
                    work.approach_field(
                        QueryPurpose::NavigationTest,
                        tick,
                        grid,
                        false,
                        &[TilePos::new(x, 2)]
                    ),
                    Progress::Ready(_)
                ) {
                    complete = false;
                    break;
                }
            }
            assert!(work.spent() <= 10_667);
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
            work.approach_field(QueryPurpose::NavigationTest, 0, ground, false, &goals),
            Progress::Deferred
        ));
        assert!(matches!(
            work.approach_field(QueryPurpose::NavigationTest, 0, air, true, &goals),
            Progress::Deferred
        ));
        assert_eq!(work.stats().pending_approach_fields, 2);
        let mut bytes = Vec::new();
        ciborium::into_writer(&work, &mut bytes).unwrap();
        let clone: PlanningWork = ciborium::from_reader(bytes.as_slice()).unwrap();
        for tick in (12..120).step_by(12) {
            work.begin(tick);
            clone.begin(tick);
            assert_eq!(work, clone);
            assert!(work.spent() <= 120);
            if work.stats().pending_approach_fields == 0 {
                let Progress::Ready(ground_field) =
                    work.approach_field(QueryPurpose::NavigationTest, tick, ground, false, &goals)
                else {
                    panic!("ground field finished");
                };
                let Progress::Ready(air_field) =
                    work.approach_field(QueryPurpose::NavigationTest, tick, air, true, &goals)
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
        let Progress::Ready(field) = work.approach_field(
            QueryPurpose::NavigationTest,
            0,
            KnownGrid::new(10, 6, &open).unwrap(),
            false,
            &goals,
        ) else {
            panic!("small field finishes");
        };
        assert!(field.path(&[TilePos::new(2, 2)]).is_some());
        let Progress::Ready(field) = work.approach_field(
            QueryPurpose::NavigationTest,
            12,
            KnownGrid::new(10, 6, &blocked).unwrap(),
            false,
            &goals,
        ) else {
            panic!("replacement finishes");
        };
        assert!(field.path(&[TilePos::new(2, 2)]).is_none());
        let empty = PlanningWork::with_allowance(0);
        for x in 0..40 {
            assert!(matches!(
                empty.approach_field(
                    QueryPurpose::NavigationTest,
                    0,
                    KnownGrid::new(10, 6, &open).unwrap(),
                    false,
                    &[TilePos::new(x, 2)]
                ),
                Progress::Deferred
            ));
            assert!(empty.stats().retained_approach_fields <= PENDING_FIELDS);
        }
        assert_eq!(empty.spent(), 0);
        empty.begin(120);
        assert_eq!(empty.stats().pending_approach_fields, 0);
    }
}
