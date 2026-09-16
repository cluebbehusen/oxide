//! Owned, resumable distance fields over one immutable passability surface.

use crate::bot::planning::{Progress, WorkBudget};
use chassis::grid::TilePos;
use std::collections::VecDeque;

const BUCKETS: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct DistanceWork {
    width: usize,
    open: Vec<bool>,
    distances: Vec<u32>,
    frontier: [VecDeque<(u32, usize)>; BUCKETS],
    queued: usize,
    current: u32,
}

impl DistanceWork {
    pub(in crate::bot) fn new(
        width: i32,
        height: i32,
        open: Vec<bool>,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> Self {
        assert!(width >= 0 && height >= 0);
        assert_eq!(open.len(), width as usize * height as usize);
        let mut result = Self {
            width: width as usize,
            distances: vec![u32::MAX; open.len()],
            open,
            frontier: std::array::from_fn(|_| VecDeque::new()),
            queued: 0,
            current: 0,
        };
        for source in sources {
            if source.x < 0 || source.y < 0 || source.x >= width || source.y >= height {
                continue;
            }
            let index = source.y as usize * result.width + source.x as usize;
            if result.open[index] && result.distances[index] != 0 {
                result.distances[index] = 0;
                result.frontier[0].push_back((0, index));
                result.queued += 1;
            }
        }
        result
    }

    pub(in crate::bot) fn advance(&mut self, budget: &mut WorkBudget) -> Progress<()> {
        while self.queued > 0 {
            if !budget.charge(1) {
                return Progress::Deferred;
            }
            while self.frontier[self.current as usize % BUCKETS]
                .front()
                .is_none_or(|(distance, _)| *distance > self.current)
            {
                self.current += 1;
            }
            let (distance, index) = self.frontier[self.current as usize % BUCKETS]
                .pop_front()
                .unwrap();
            self.queued -= 1;
            if self.distances[index] != distance {
                continue;
            }
            #[cfg(test)]
            super::work::record(|work| work.expanded += 1);
            let x = index % self.width;
            let west = x > 0 && self.open[index - 1];
            let east = x + 1 < self.width && self.open[index + 1];
            let north = index >= self.width && self.open[index - self.width];
            let south = index + self.width < self.open.len() && self.open[index + self.width];
            for (allowed, dx, dy, cost) in [
                (west, -1, 0, 10),
                (east, 1, 0, 10),
                (north, 0, -1, 10),
                (south, 0, 1, 10),
                (west && north, -1, -1, 14),
                (east && north, 1, -1, 14),
                (west && south, -1, 1, 14),
                (east && south, 1, 1, 14),
            ] {
                if !allowed {
                    continue;
                }
                let next = (index as isize + dx + dy * self.width as isize) as usize;
                let proposed = distance.saturating_add(cost);
                if self.open[next] && proposed < self.distances[next] {
                    self.distances[next] = proposed;
                    self.frontier[proposed as usize % BUCKETS].push_back((proposed, next));
                    self.queued += 1;
                }
            }
        }
        Progress::Ready(())
    }

    pub(in crate::bot) fn into_distances(self) -> Vec<u32> {
        assert_eq!(
            self.queued, 0,
            "unfinished distances are not reachability evidence"
        );
        self.distances
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slice_size_preserves_the_complete_field_and_clone_continuation() {
        let mut open = vec![true; 120];
        for y in 0..9 {
            open[y * 12 + 5] = false;
        }
        let initial = DistanceWork::new(12, 10, open, [TilePos::new(0, 0)]);
        let mut complete = initial.clone();
        assert_eq!(
            complete.advance(&mut WorkBudget::new(1000)),
            Progress::Ready(())
        );
        let expected = complete.into_distances();
        assert_eq!(expected[4], 40);
        assert_eq!(expected[5], u32::MAX);
        for allowance in [1, 2, 7, 19] {
            let mut work = initial.clone();
            let mut clone = initial.clone();
            loop {
                assert_eq!(work.advance(&mut WorkBudget::new(0)), Progress::Deferred);
                let mut budget = WorkBudget::new(allowance);
                let progress = work.advance(&mut budget);
                assert!(budget.spent() <= allowance);
                assert_eq!(progress, clone.advance(&mut WorkBudget::new(allowance)));
                assert_eq!(work, clone);
                if progress == Progress::Ready(()) {
                    break;
                }
            }
            assert_eq!(work.into_distances(), expected);
        }
    }

    #[test]
    fn blocked_sources_and_diagonal_corners_do_not_leak_reachability() {
        let mut work = DistanceWork::new(
            2,
            2,
            vec![true, false, false, true],
            [TilePos::new(0, 0), TilePos::new(1, 0)],
        );
        assert_eq!(work.advance(&mut WorkBudget::new(1)), Progress::Ready(()));
        assert_eq!(work.into_distances(), [0, u32::MAX, u32::MAX, u32::MAX]);
    }
}
