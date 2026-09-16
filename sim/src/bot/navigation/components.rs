//! Exact connectivity over immutable passability snapshots.

use crate::bot::query_work::QueryPurpose;
use chassis::grid::TilePos;
use std::{cell::RefCell, collections::VecDeque, sync::Arc};

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const CACHE_ENTRIES: usize = 16;

struct Entry {
    dimensions: (i32, i32),
    open: Box<[bool]>,
    labels: Arc<[u32]>,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.open.len() * (size_of::<bool>() + size_of::<u32>())
    }
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl Cache {
    fn labels(
        &mut self,
        query_purpose: QueryPurpose,
        dimensions: (i32, i32),
        open: Vec<bool>,
    ) -> Arc<[u32]> {
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.dimensions == dimensions && entry.open.as_ref() == open.as_slice()
        }) {
            let entry = self.entries.remove(index).unwrap();
            let labels = Arc::clone(&entry.labels);
            crate::bot::query_work::record(
                query_purpose,
                crate::bot::query_work::QueryOperation::CacheHit,
                1,
            );
            self.entries.push_back(entry);
            #[cfg(test)]
            super::work::record(|work| work.hits += 1);
            return labels;
        }
        let labels: Arc<[u32]> = super::flood::labels(query_purpose, &open, dimensions).into();
        let entry = Entry {
            dimensions,
            open: open.into_boxed_slice(),
            labels: Arc::clone(&labels),
        };
        let bytes = entry.bytes();
        if bytes <= CACHE_BYTES {
            while self.bytes + bytes > CACHE_BYTES || self.entries.len() == CACHE_ENTRIES {
                self.bytes -= self.entries.pop_front().unwrap().bytes();
            }
            self.bytes += bytes;
            self.entries.push_back(entry);
        }
        labels
    }
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

pub(super) fn labels(
    query_purpose: QueryPurpose,
    dimensions: (i32, i32),
    open: Vec<bool>,
) -> Arc<[u32]> {
    CACHE.with_borrow_mut(|cache| cache.labels(query_purpose, dimensions, open))
}

pub(super) fn connects(dimensions: (i32, i32), labels: &[u32], from: TilePos, to: TilePos) -> bool {
    let (width, height) = dimensions;
    let label = |tile| super::flood::tile_index(width, height, tile).map(|index| labels[index]);
    let Some(target) = label(to).filter(|label| *label != 0) else {
        return false;
    };
    match label(from) {
        None => false,
        Some(0) => [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .any(|(x, y)| label(from.offset(x, y)) == Some(target)),
        Some(source) => source == target,
    }
}

#[cfg(test)]
pub(super) fn clear() {
    CACHE.with_borrow_mut(|cache| *cache = Cache::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_identity_includes_every_tile_and_grid_dimensions() {
        let mut cache = Cache::default();
        for mask in 0..512 {
            let open: Vec<_> = (0..9).map(|bit| mask & (1 << bit) == 0).collect();
            for dimensions in [(3, 3), (9, 1), (1, 9)] {
                let expected =
                    super::super::flood::labels(QueryPurpose::NavigationTest, &open, dimensions);
                let first = cache.labels(QueryPurpose::NavigationTest, dimensions, open.clone());
                assert_eq!(&*first, &expected);
                let (warm, work) = super::super::work::measure(|| {
                    cache.labels(QueryPurpose::NavigationTest, dimensions, open.clone())
                });
                assert!(Arc::ptr_eq(&first, &warm));
                assert_eq!(work.expanded, 0);
                assert_eq!(work.hits, 1);
            }
        }
    }

    #[test]
    fn eviction_and_other_threads_cannot_change_connectivity() {
        let mut cache = Cache::default();
        let open = vec![true; 128 * 128];
        let retained = cache.labels(QueryPurpose::NavigationTest, (128, 128), open.clone());
        for blocked in 0..32 {
            let mut changed = open.clone();
            changed[blocked] = false;
            let labels = cache.labels(QueryPurpose::NavigationTest, (128, 128), changed);
            assert_eq!(labels[blocked], 0);
            assert!(cache.bytes <= CACHE_BYTES);
            assert!(cache.entries.len() <= CACHE_ENTRIES);
        }
        assert_eq!(cache.entries.len(), CACHE_ENTRIES);
        let regenerated = cache.labels(QueryPurpose::NavigationTest, (128, 128), open.clone());
        assert!(!Arc::ptr_eq(&retained, &regenerated));
        assert_eq!(retained, regenerated);
        let other =
            std::thread::spawn(move || labels(QueryPurpose::NavigationTest, (128, 128), open))
                .join()
                .unwrap();
        assert_eq!(retained, other);
        let oversized = vec![true; CACHE_BYTES / 5 + 1];
        let before = cache.bytes;
        let large = cache.labels(
            QueryPurpose::NavigationTest,
            (oversized.len() as i32, 1),
            oversized,
        );
        assert!(large.iter().all(|label| *label == 1));
        assert_eq!(cache.bytes, before);
    }
}
