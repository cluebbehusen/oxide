//! Canonical bounded paths with thread-local reusable search storage.

use chassis::{grid::TilePos, path::AstarScratch};
use std::cell::RefCell;

thread_local! {
    static SCRATCH: RefCell<AstarScratch> = RefCell::default();
}

pub(in crate::bot) struct Search(AstarScratch);

impl Default for Search {
    fn default() -> Self {
        let mut scratch = SCRATCH.with_borrow_mut(std::mem::take);
        // Reachability evidence belongs to the old passability context.
        scratch.clear_search_evidence();
        Self(scratch)
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        SCRATCH.with_borrow_mut(|scratch| *scratch = std::mem::take(&mut self.0));
    }
}

impl Search {
    pub(super) fn clear_search_evidence(&mut self) {
        self.0.clear_search_evidence();
    }
    pub(in crate::bot) fn last_search_exhausted(&self) -> bool {
        self.0.last_search_exhausted()
    }
    pub(in crate::bot) fn last_search_reached(&self, tile: TilePos) -> bool {
        self.0.last_search_reached(tile)
    }

    pub(super) fn path_with_distances(
        &mut self,
        grid: super::KnownGrid<'_>,
        overlay: Option<super::BlockedRect>,
        start: TilePos,
        goal: TilePos,
        distances: &[u32],
    ) -> Option<Vec<TilePos>> {
        let path = chassis::path::astar_with_distances(
            (grid.width, grid.height),
            start,
            goal,
            |tile| grid.open(tile, overlay),
            crate::stats::PATH_EXPANSION_CAP,
            &mut self.0,
            distances,
        );
        #[cfg(test)]
        super::work::record(|work| {
            work.searches += 1;
            work.expanded += self.0.last_expansions() as usize;
            work.paths += usize::from(path.is_some());
        });
        path
    }

    pub(in crate::bot) fn path(
        &mut self,
        width: i32,
        height: i32,
        start: TilePos,
        goal: TilePos,
        open: impl Fn(TilePos) -> bool,
    ) -> Option<Vec<TilePos>> {
        let path = chassis::path::astar_with_scratch(
            width,
            height,
            start,
            goal,
            open,
            crate::stats::PATH_EXPANSION_CAP,
            &mut self.0,
        );
        #[cfg(test)]
        super::work::record(|work| {
            work.searches += 1;
            work.expanded += self.0.last_expansions() as usize;
            work.paths += usize::from(path.is_some());
        });
        path
    }
}

pub(in crate::bot) fn canonical_path(
    width: i32,
    height: i32,
    start: TilePos,
    goal: TilePos,
    open: impl Fn(TilePos) -> bool,
) -> Option<Vec<TilePos>> {
    Search::default().path(width, height, start, goal, open)
}

/// First candidate in caller preference order, retaining capped-search behavior.
pub(in crate::bot) fn first_reachable_goal(
    width: i32,
    height: i32,
    start: TilePos,
    goals: &[TilePos],
    open: impl Fn(TilePos) -> bool,
) -> Option<TilePos> {
    let mut search = Search::default();
    for (index, goal) in goals.iter().copied().enumerate() {
        if search.path(width, height, start, goal, &open).is_some() {
            return Some(goal);
        }
        if search.last_search_exhausted() {
            return goals[index + 1..]
                .iter()
                .copied()
                .find(|tile| search.last_search_reached(*tile));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferred_goal_queries_match_independent_canonical_searches() {
        for mask in 0..512 {
            let open = |tile: TilePos| {
                super::super::flood::tile_index(3, 3, tile)
                    .is_some_and(|index| mask & (1 << index) == 0)
            };
            for index in 0..9 {
                let start = TilePos::new(index % 3, index / 3);
                let goals = [TilePos::new(2, 2), TilePos::new(0, 0), TilePos::new(1, 1)];
                let expected = goals.iter().copied().find(|goal| {
                    chassis::path::astar(3, 3, start, *goal, open, crate::stats::PATH_EXPANSION_CAP)
                        .is_some()
                });
                assert_eq!(first_reachable_goal(3, 3, start, &goals, open), expected);
            }
        }
    }
    #[test]
    fn canonical_query_is_reentrant_and_clears_retained_failure_evidence() {
        let start = TilePos::new(0, 0);
        let goal = TilePos::new(4, 0);
        let mut outer = Search::default();
        assert!(outer.path(5, 1, start, goal, |tile| tile.x != 2).is_none());
        assert!(outer.last_search_exhausted());
        let expected = chassis::path::astar(
            5,
            1,
            start,
            goal,
            |_| true,
            crate::stats::PATH_EXPANSION_CAP,
        );
        assert_eq!(
            canonical_path(5, 1, start, goal, |_| {
                assert!(canonical_path(1, 1, start, start, |_| true).is_some());
                true
            }),
            expected
        );
        assert!(outer.last_search_exhausted());
        drop(outer);
        let new = Search::default();
        assert!(!new.last_search_exhausted());
        assert!(!new.last_search_reached(start));
    }
    #[test]
    fn capped_preferred_goal_does_not_discard_a_later_reachable_goal() {
        let start = TilePos::new(0, 0);
        let sealed = TilePos::new(240, 240);
        let reachable = TilePos::new(4, 0);
        let open = |tile: TilePos| {
            !(tile != sealed && (239..=241).contains(&tile.x) && (239..=241).contains(&tile.y))
        };
        let mut search = Search::default();
        assert!(search.path(256, 256, start, sealed, open).is_none());
        assert!(!search.last_search_exhausted());
        assert_eq!(
            first_reachable_goal(256, 256, start, &[sealed, reachable], open),
            Some(reachable)
        );
    }
}
