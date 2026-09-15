//! Canonical bounded paths with thread-local reusable search storage.

use chassis::{grid::TilePos, path::AstarScratch};
use std::cell::RefCell;

thread_local! {
    static SCRATCH: RefCell<AstarScratch> = RefCell::default();
}

pub(in crate::bot) struct Search {
    scratch: AstarScratch,
    queried: bool,
}

impl Default for Search {
    fn default() -> Self {
        let mut scratch = SCRATCH.with_borrow_mut(std::mem::take);
        // Reachability evidence belongs to the old passability context.
        scratch.clear_search_evidence();
        Self {
            scratch,
            queried: false,
        }
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        SCRATCH.with_borrow_mut(|scratch| *scratch = std::mem::take(&mut self.scratch));
    }
}

impl Search {
    pub(super) fn clear_search_evidence(&mut self) {
        self.scratch.clear_search_evidence();
        self.queried = false;
    }
    pub(super) fn last_expansions(&self) -> u32 {
        if self.queried {
            self.scratch.last_expansions()
        } else {
            0
        }
    }
    pub(in crate::bot) fn last_search_exhausted(&self) -> bool {
        self.scratch.last_search_exhausted()
    }
    pub(in crate::bot) fn last_search_reached(&self, tile: TilePos) -> bool {
        self.scratch.last_search_reached(tile)
    }

    pub(super) fn path_with_distances(
        &mut self,
        grid: super::KnownGrid<'_>,
        overlay: Option<super::BlockedRect>,
        start: TilePos,
        goal: TilePos,
        distances: &[u32],
    ) -> Option<Vec<TilePos>> {
        self.queried = true;
        let path = chassis::path::astar_with_distances(
            (grid.width, grid.height),
            start,
            goal,
            |tile| grid.open(tile, overlay),
            crate::stats::PATH_EXPANSION_CAP,
            &mut self.scratch,
            distances,
        );
        #[cfg(test)]
        super::work::record(|work| {
            work.searches += 1;
            work.expanded += self.scratch.last_expansions() as usize;
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
        self.queried = true;
        let path = chassis::path::astar_with_scratch(
            width,
            height,
            start,
            goal,
            open,
            crate::stats::PATH_EXPANSION_CAP,
            &mut self.scratch,
        );
        #[cfg(test)]
        super::work::record(|work| {
            work.searches += 1;
            work.expanded += self.scratch.last_expansions() as usize;
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

/// Connectivity without route construction, retaining capped-search behavior.
pub(in crate::bot) fn reachable(
    width: i32,
    height: i32,
    start: TilePos,
    goal: TilePos,
    open: impl Fn(TilePos) -> bool,
) -> bool {
    let Some(cells) = usize::try_from(width).ok().and_then(|width| {
        usize::try_from(height)
            .ok()
            .and_then(|height| width.checked_mul(height))
    }) else {
        return false;
    };
    if super::flood::tile_index(width, height, start).is_none()
        || super::flood::tile_index(width, height, goal).is_none()
    {
        return false;
    }
    if cells > crate::stats::PATH_EXPANSION_CAP as usize {
        return canonical_path(width, height, start, goal, open).is_some();
    }
    if start == goal {
        return true;
    }
    if !open(goal) {
        return false;
    }
    // A legal diagonal has two open side tiles, so it cannot connect separate
    // cardinal components. Costs and tie-breaking are irrelevant to this query.
    super::flood::reaches_any(width, height, [start], open, |tile| tile == goal)
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
    fn connectivity_matches_canonical_routes_without_constructing_them() {
        let (_, work) = super::super::work::measure(|| {
            for mask in 0..512 {
                let open = |tile: TilePos| {
                    super::super::flood::tile_index(3, 3, tile)
                        .is_some_and(|index| mask & (1 << index) == 0)
                };
                for from in 0..9 {
                    for to in 0..9 {
                        let start = TilePos::new(from % 3, from / 3);
                        let goal = TilePos::new(to % 3, to / 3);
                        let expected = chassis::path::astar(
                            3,
                            3,
                            start,
                            goal,
                            open,
                            crate::stats::PATH_EXPANSION_CAP,
                        )
                        .is_some();
                        assert_eq!(
                            reachable(3, 3, start, goal, open),
                            expected,
                            "mask={mask}, start={start:?}, goal={goal:?}"
                        );
                    }
                }
            }
        });
        assert!(work.components > 0);
        assert_eq!(work.paths, 0);
        assert_eq!(work.fields, 0);
        assert!(!reachable(
            3,
            3,
            TilePos::new(-1, 0),
            TilePos::new(0, 0),
            |_| true
        ));
        assert!(!reachable(
            3,
            3,
            TilePos::new(0, 0),
            TilePos::new(3, 0),
            |_| true
        ));
    }

    #[test]
    fn connectivity_retains_large_map_search_limits() {
        let start = TilePos::new(0, 0);
        let goal = TilePos::new(250, 250);
        let open = |tile: TilePos| tile.x != 249 || tile.y == 0;
        let expected = canonical_path(256, 256, start, goal, open).is_some();
        assert!(
            !expected,
            "the long detour must exhaust the canonical allowance"
        );
        assert_eq!(reachable(256, 256, start, goal, open), expected);
        assert!(super::super::flood::reaches_any(
            256,
            256,
            [start],
            open,
            |tile| tile == goal
        ));
    }
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
