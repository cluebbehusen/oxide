//! Deterministic, thread-local work accounting for navigation regression tests.

use crate::bot::query_work::QueryPurpose;
use std::cell::Cell;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::bot) struct Counts {
    pub searches: usize,
    pub expanded: usize,
    pub paths: usize,
    pub fields: usize,
    pub components: usize,
    pub egress_checks: usize,
    pub placement_checks: usize,
    pub passability_queries: usize,
    pub generations: usize,
    pub hits: usize,
}

thread_local! {
    static COUNTS: Cell<Counts> = Cell::default();
}

pub(in crate::bot) fn record(update: impl FnOnce(&mut Counts)) {
    COUNTS.with(|cell| {
        let mut counts = cell.get();
        update(&mut counts);
        cell.set(counts);
    });
}

pub(in crate::bot) fn measure<T>(run: impl FnOnce() -> T) -> (T, Counts) {
    let before = COUNTS.get();
    let value = run();
    let after = COUNTS.get();
    (
        value,
        Counts {
            searches: after.searches - before.searches,
            expanded: after.expanded - before.expanded,
            paths: after.paths - before.paths,
            fields: after.fields - before.fields,
            components: after.components - before.components,
            egress_checks: after.egress_checks - before.egress_checks,
            placement_checks: after.placement_checks - before.placement_checks,
            passability_queries: after.passability_queries - before.passability_queries,
            generations: after.generations - before.generations,
            hits: after.hits - before.hits,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::navigation::{
        BlockedRect, CostQueries, KnownGrid,
        paths::{CacheClass, PathBoard, PathQueries},
        search::Search,
    };
    use chassis::grid::TilePos;
    use std::cell::RefCell;

    #[test]
    fn warm_queries_and_hypothetical_layouts_do_not_rebuild_normal_routes() {
        let blocked = vec![false; 120];
        let grid = KnownGrid::new(12, 10, &blocked).unwrap();
        let cache = RefCell::new(PathQueries::default());
        let board = PathBoard {
            query_purpose: QueryPurpose::NavigationTest,
            grid,
            class: CacheClass::Ground,
            cache: &cache,
        };
        let from = TilePos::new(1, 1);
        let to = TilePos::new(10, 8);
        let query = |board: PathBoard<'_>| {
            let path = board.path(from, to, None, &mut Search::default());
            let bound = board.bound(from, to);
            (path, bound)
        };
        let (expected, cold) = measure(|| query(board));
        assert!(expected.0.is_some());
        assert!(cold.searches <= 2 && cold.expanded <= 240, "{cold:?}");
        assert_eq!(cold.generations, 1);
        assert_eq!(cold.fields, 1);
        let (_, warm) = measure(|| {
            for _ in 0..100 {
                assert_eq!(query(board), expected);
            }
        });
        assert_eq!(warm.searches, 0, "{warm:?}");
        assert_eq!(warm.expanded, 0);
        assert_eq!(warm.generations, 0);
        let mut changed = blocked.clone();
        changed[5 * 12 + 6] = true;
        let changed_grid = KnownGrid::new(12, 10, &changed).unwrap();
        let (_, hypothetical) = measure(|| {
            query(PathBoard {
                query_purpose: QueryPurpose::NavigationTest,
                grid: changed_grid,
                class: CacheClass::Hypothetical,
                cache: &cache,
            })
        });
        assert_eq!(hypothetical.generations, 1);
        let (_, restored) = measure(|| assert_eq!(query(board), expected));
        assert_eq!(
            restored.searches, 0,
            "hypothetical work evicted normal routes: {restored:?}"
        );
        let (_, changed) = measure(|| {
            query(PathBoard {
                grid: changed_grid,
                ..board
            })
        });
        assert_eq!(changed.generations, 1);
        assert!(
            changed.searches <= 2 && changed.expanded <= 240,
            "{changed:?}"
        );
    }

    #[test]
    fn endpoint_sets_bound_work_by_grid_size_and_reuse_exact_answers() {
        let blocked = vec![false; 120];
        let grid = KnownGrid::new(12, 10, &blocked).unwrap();
        let starts = (0..10).map(|y| TilePos::new(0, y)).collect::<Vec<_>>();
        let goals = (0..10).map(|y| TilePos::new(11, y)).collect::<Vec<_>>();
        let overlay = Some(BlockedRect {
            anchor: TilePos::new(5, 3),
            size: (2, 3),
        });
        let mut queries = CostQueries::default();
        let (expected, cold) = measure(|| {
            queries.between_sets(QueryPurpose::NavigationTest, grid, overlay, &starts, &goals)
        });
        assert!(expected.available_cost().is_some());
        assert_eq!(
            cold.searches, 1,
            "endpoint pairs must not multiply searches: {cold:?}"
        );
        assert!(cold.expanded <= blocked.len(), "{cold:?}");
        assert_eq!(cold.paths, 0, "cost-only work must not materialize paths");
        let (_, warm) = measure(|| {
            for _ in 0..100 {
                assert_eq!(
                    queries.between_sets(
                        QueryPurpose::NavigationTest,
                        grid,
                        overlay,
                        &starts,
                        &goals
                    ),
                    expected
                );
            }
        });
        assert_eq!(warm.searches, 0);
        assert_eq!(warm.expanded, 0);
        assert_eq!(warm.hits, 100);
    }

    #[test]
    fn nested_measurements_include_inner_work_without_mixing_threads() {
        let (_, outer) = measure(|| {
            record(|work| work.searches += 1);
            let (_, inner) = measure(|| record(|work| work.searches += 2));
            assert_eq!(inner.searches, 2);
            let other = std::thread::spawn(|| measure(|| record(|work| work.searches += 5)).1)
                .join()
                .unwrap();
            assert_eq!(other.searches, 5);
        });
        assert_eq!(outer.searches, 3);
        let (_, next) = measure(|| ());
        assert_eq!(next, Counts::default());
    }
}
