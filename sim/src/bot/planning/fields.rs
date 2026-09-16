//! Controller-owned field preparation. Unfinished work is never a route verdict.

use super::{Progress, WorkBudget};
use crate::bot::query_work::QueryPurpose;
use crate::bot::{
    PublicMapBriefing,
    navigation::public_fields::{BlockedGroundLayout, PublicFieldWork, PublicGroundDistances},
};
use chassis::grid::TilePos;
use std::{collections::BTreeMap, sync::Arc};

const RETAINED_JOBS: usize = 4;
const PENDING_IDLE_LIFETIME: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Job {
    query_purpose: QueryPurpose,
    used_at: u64,
    work: PublicFieldWork,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::bot) struct FieldPreparation {
    generation: Option<(PublicMapBriefing, BlockedGroundLayout)>,
    jobs: BTreeMap<Vec<TilePos>, Job>,
    next_pending: usize,
}

impl FieldPreparation {
    pub(super) fn resume_pending(&mut self, tick: u64, budget: &mut WorkBudget) {
        self.jobs.retain(|_, job| {
            job.work.is_ready() || tick.saturating_sub(job.used_at) < PENDING_IDLE_LIFETIME
        });
        let Some((map, blocked)) = &self.generation else {
            return;
        };
        let mut pending = self
            .jobs
            .values_mut()
            .filter(|job| !job.work.is_ready())
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return;
        }
        let first = self.next_pending % pending.len();
        self.next_pending = (first + 1) % pending.len();
        pending.rotate_left(first);
        for job in pending {
            budget.run_slice(16_000, |slice| {
                job.work.advance(job.query_purpose, map, blocked, slice)
            });
        }
    }

    pub(super) fn counts(&self) -> (usize, usize) {
        (
            self.jobs
                .values()
                .filter(|job| !job.work.is_ready())
                .count(),
            self.jobs.len(),
        )
    }

    pub(in crate::bot) fn advance(
        &mut self,
        query_purpose: QueryPurpose,
        tick: u64,
        map: &PublicMapBriefing,
        blocked: &BlockedGroundLayout,
        sources: impl IntoIterator<Item = TilePos>,
        budget: &mut WorkBudget,
    ) -> Progress<Arc<PublicGroundDistances>> {
        if self
            .generation
            .as_ref()
            .is_none_or(|(prior_map, prior_blocked)| prior_map != map || prior_blocked != blocked)
        {
            self.generation = Some((map.clone(), blocked.clone()));
            self.jobs.clear();
            self.next_pending = 0;
        }
        self.jobs.retain(|_, job| {
            job.work.is_ready() || tick.saturating_sub(job.used_at) < PENDING_IDLE_LIFETIME
        });
        let mut sources: Vec<_> = sources.into_iter().collect();
        sources.sort_unstable_by_key(|tile| (tile.y, tile.x));
        sources.dedup();
        if !self.jobs.contains_key(&sources) && self.jobs.len() == RETAINED_JOBS {
            let victim = self
                .jobs
                .iter()
                .filter(|(_, job)| job.work.is_ready())
                .min_by_key(|(key, job)| (job.used_at, *key))
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else {
                return Progress::Deferred;
            };
            self.jobs.remove(&victim);
        }
        let job = self.jobs.entry(sources.clone()).or_insert_with(|| Job {
            query_purpose,
            used_at: tick,
            work: PublicFieldWork::new(map, sources),
        });
        job.used_at = tick;
        job.work.advance(query_purpose, map, blocked, budget)
    }
}
