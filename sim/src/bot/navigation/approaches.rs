//! Shortest representative approaches to a shared destination set.
//!
//! Strategic coverage needs a plausible shortest approach, not the command
//! router's particular choice among tied paths. One reverse field serves every
//! origin and doorstep of an asset.

#[cfg(test)]
use super::{KnownGrid, distance_work::DistanceWork};
#[cfg(test)]
use crate::bot::planning::{Progress, WorkBudget};
#[cfg(test)]
use crate::bot::query_work::QueryPurpose;
use chassis::grid::{CARDINALS, DIAGONALS, TilePos};

#[derive(Debug, PartialEq, Eq)]
pub(in crate::bot) struct ApproachField {
    width: i32,
    height: i32,
    distances: Vec<u32>,
}

impl ApproachField {
    pub(in crate::bot) fn from_distances(width: i32, height: i32, distances: Vec<u32>) -> Self {
        #[cfg(test)]
        super::work::record(|work| work.fields += 1);
        Self {
            width,
            height,
            distances,
        }
    }

    #[cfg(test)]
    fn new(grid: KnownGrid<'_>, goals: &[TilePos]) -> Self {
        let mut work = DistanceWork::new(
            QueryPurpose::NavigationTest,
            grid.width,
            grid.height,
            grid.blocked.iter().map(|blocked| !blocked).collect(),
            goals.iter().copied(),
        );
        assert_eq!(
            work.advance(
                QueryPurpose::NavigationTest,
                &mut WorkBudget::new(usize::MAX)
            ),
            Progress::Ready(())
        );
        #[cfg(test)]
        super::work::record(|work| work.fields += 1);
        Self {
            width: grid.width,
            height: grid.height,
            distances: work.into_distances(),
        }
    }

    pub(in crate::bot) fn distances(&self) -> &[u32] {
        &self.distances
    }

    fn distance(&self, tile: TilePos) -> Option<u32> {
        if tile.x < 0 || tile.y < 0 || tile.x >= self.width || tile.y >= self.height {
            return None;
        }
        let distance = self.distances[(tile.y * self.width + tile.x) as usize];
        (distance != u32::MAX).then_some(distance)
    }

    fn next(&self, from: TilePos) -> Option<(u32, TilePos)> {
        CARDINALS
            .iter()
            .chain(DIAGONALS.iter())
            .filter_map(|&(dx, dy)| {
                let tile = TilePos::new(from.x + dx, from.y + dy);
                let diagonal = dx != 0 && dy != 0;
                if diagonal
                    && (self.distance(TilePos::new(tile.x, from.y)).is_none()
                        || self.distance(TilePos::new(from.x, tile.y)).is_none())
                {
                    return None;
                }
                Some((
                    self.distance(tile)?
                        .checked_add(if diagonal { 14 } else { 10 })?,
                    tile,
                ))
            })
            .min_by_key(|(cost, tile)| (*cost, tile.y, tile.x))
    }

    pub(in crate::bot) fn path(&self, starts: &[TilePos]) -> Option<(TilePos, Vec<TilePos>)> {
        let (_, start) = starts
            .iter()
            .copied()
            .filter_map(|start| {
                if start.x < 0 || start.y < 0 || start.x >= self.width || start.y >= self.height {
                    return None;
                }
                // Like command routing, a unit may leave a blocked starting tile.
                let cost = self
                    .distance(start)
                    .or_else(|| self.next(start).map(|(cost, _)| cost))?;
                Some((cost, start))
            })
            .min_by_key(|(cost, start)| (*cost, start.y, start.x))?;
        let mut path = vec![start];
        let mut tile = start;
        while self.distance(tile) != Some(0) {
            tile = self.next(tile)?.1;
            path.push(tile);
        }
        Some((tile, path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_routes_match_exhaustive_costs_and_never_cut_corners() {
        for mask in 0..512 {
            let blocked: Vec<_> = (0..9).map(|index| mask & (1 << index) != 0).collect();
            let grid = KnownGrid::new(3, 3, &blocked).unwrap();
            let goals = [TilePos::new(2, 2), TilePos::new(1, 0)];
            let field = ApproachField::new(grid, &goals);
            for index in 0..9 {
                let start = TilePos::new(index % 3, index / 3);
                let expected = goals
                    .iter()
                    .filter(|goal| grid.open(**goal, None))
                    .filter_map(|goal| {
                        let mut path = super::super::search::canonical_path(
                            QueryPurpose::NavigationTest,
                            3,
                            3,
                            start,
                            *goal,
                            |tile| grid.open(tile, None),
                        )?;
                        path.insert(0, start);
                        Some(super::super::paths::path_cost(&path))
                    })
                    .min();
                let route = field.path(&[start]);
                assert_eq!(
                    route
                        .as_ref()
                        .map(|(_, path)| super::super::paths::path_cost(path)),
                    expected,
                    "mask {mask}, start {start:?}"
                );
                if let Some((goal, path)) = route {
                    assert!(goals.contains(&goal));
                    assert_eq!(path.first(), Some(&start));
                    for pair in path.windows(2) {
                        assert!(grid.open(pair[1], None));
                        if pair[0].x != pair[1].x && pair[0].y != pair[1].y {
                            assert!(grid.open(TilePos::new(pair[0].x, pair[1].y), None));
                            assert!(grid.open(TilePos::new(pair[1].x, pair[0].y), None));
                        }
                    }
                }
            }
        }
    }
}
