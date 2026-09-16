//! Batch proofs for canonical route safety on an immutable navigation surface.

use super::{distance_work::DistanceWork, flood::tile_index};
use crate::bot::planning::{Progress, WorkBudget};
use crate::bot::query_work::QueryPurpose;
use chassis::grid::{CARDINALS, DIAGONALS, TilePos};
use std::collections::{BTreeMap, VecDeque};

const RETAINED_FIELDS: usize = 16;
const TRACKED_ENDPOINTS: usize = 128;
const UNSAFE: u8 = 1;
const SAFE: u8 = 2;

#[derive(Default)]
pub(super) struct SafetyQueries {
    work: BTreeMap<TilePos, usize>,
    fields: BTreeMap<TilePos, Vec<u8>>,
    order: VecDeque<TilePos>,
}

impl SafetyQueries {
    pub(super) fn record(&mut self, from: TilePos, to: TilePos, expanded: usize) {
        for endpoint in [from, to] {
            if !self.work.contains_key(&endpoint) && self.work.len() == TRACKED_ENDPOINTS {
                self.work.pop_first();
            }
            let work = self.work.entry(endpoint).or_default();
            *work = work.saturating_add(expanded);
        }
    }

    /// `None` means the canonical route still needs inspection, not unsafe.
    pub(super) fn prove(
        &mut self,
        query_purpose: QueryPurpose,
        dimensions: (i32, i32),
        from: TilePos,
        to: TilePos,
        open: impl Fn(TilePos) -> bool,
        safe: impl Fn(TilePos) -> bool,
    ) -> Option<bool> {
        let (width, height) = dimensions;
        let from_index = tile_index(width, height, from)?;
        let to_index = tile_index(width, height, to)?;
        let cells = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        if cells > crate::stats::PATH_EXPANSION_CAP as usize
            || !open(from)
            || !open(to)
            || !safe(from)
            || !safe(to)
        {
            return None;
        }
        let (source, destination) = if self.fields.contains_key(&to) {
            (to, from_index)
        } else if self.fields.contains_key(&from) {
            (from, to_index)
        } else if self.work.get(&to).copied().unwrap_or(0) >= cells {
            (to, from_index)
        } else if self.work.get(&from).copied().unwrap_or(0) >= cells {
            (from, to_index)
        } else {
            return None;
        };
        if !self.fields.contains_key(&source) {
            if self.fields.len() == RETAINED_FIELDS {
                self.fields.remove(&self.order.pop_front().unwrap());
            }
            self.fields
                .insert(source, field(query_purpose, dimensions, source, open, safe));
            self.order.push_back(source);
        }
        match self.fields[&source][destination] {
            0 | UNSAFE => Some(false),
            SAFE => Some(true),
            _ => None,
        }
    }
}

fn field(
    query_purpose: QueryPurpose,
    (width, height): (i32, i32),
    source: TilePos,
    open: impl Fn(TilePos) -> bool,
    safe: impl Fn(TilePos) -> bool,
) -> Vec<u8> {
    let cells = width as usize * height as usize;
    let tile = |index: usize| TilePos::new(index as i32 % width, index as i32 / width);
    let mut distances = DistanceWork::new(
        query_purpose,
        width,
        height,
        (0..cells).map(|index| open(tile(index))).collect(),
        [source],
    );
    assert_eq!(
        distances.advance(query_purpose, &mut WorkBudget::new(usize::MAX)),
        Progress::Ready(())
    );
    let distances = distances.into_distances();
    let mut order = (0..cells)
        .filter(|&index| distances[index] != u32::MAX)
        .collect::<Vec<_>>();
    order.sort_unstable_by_key(|&index| (distances[index], index));
    let mut proof = vec![0; cells];
    for index in order {
        let current = tile(index);
        // Both bits mean tied shortest paths disagree about danger exposure.
        proof[index] = if !safe(current) {
            UNSAFE
        } else if distances[index] == 0 {
            SAFE
        } else {
            CARDINALS
                .iter()
                .chain(DIAGONALS.iter())
                .filter_map(|&(dx, dy)| {
                    let next = current.offset(dx, dy);
                    let neighbor = tile_index(width, height, next)?;
                    let diagonal = dx != 0 && dy != 0;
                    if diagonal && (!open(current.offset(dx, 0)) || !open(current.offset(0, dy))) {
                        return None;
                    }
                    (distances[neighbor].checked_add(if diagonal { 14 } else { 10 })
                        == Some(distances[index]))
                    .then_some(proof[neighbor])
                })
                .fold(0, |alternatives, predecessor| alternatives | predecessor)
        };
    }
    #[cfg(test)]
    super::work::record(|work| {
        work.searches += 1;
        work.fields += 1;
    });
    proof
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_batch_proof_agrees_with_the_canonical_route() {
        let mut proven = 0;
        let mut ambiguous = 0;
        for terrain in 0..512 {
            let open =
                |tile| tile_index(3, 3, tile).is_some_and(|index| terrain & (1 << index) == 0);
            for hazard in 0..9 {
                let safe = |tile: TilePos| tile != TilePos::new(hazard % 3, hazard / 3);
                for goal in 0..9 {
                    let to = TilePos::new(goal % 3, goal / 3);
                    let mut queries = SafetyQueries::default();
                    queries.record(to, to, 9);
                    for start in 0..9 {
                        let from = TilePos::new(start % 3, start / 3);
                        let expected = chassis::path::astar(3, 3, from, to, open, 20_000)
                            .is_some_and(|path| path.into_iter().all(safe));
                        match queries.prove(
                            QueryPurpose::NavigationTest,
                            (3, 3),
                            from,
                            to,
                            open,
                            safe,
                        ) {
                            Some(actual) => {
                                assert_eq!(actual, expected, "{terrain} {hazard} {start} {goal}");
                                proven += 1;
                            }
                            None => ambiguous += 1,
                        }
                    }
                }
            }
        }
        assert!(proven > 10_000 && ambiguous > 10_000);
    }

    #[test]
    fn one_prepared_endpoint_serves_both_directions_and_retention_is_bounded() {
        let mut queries = SafetyQueries::default();
        let goal = TilePos::new(39, 0);
        let source = TilePos::new(0, 0);
        assert_eq!(
            queries.prove(
                QueryPurpose::NavigationTest,
                (40, 1),
                source,
                goal,
                |_| true,
                |_| true
            ),
            None
        );
        queries.record(source, goal, 40);
        let (_, work) = super::super::work::measure(|| {
            for x in 0..39 {
                let from = TilePos::new(x, 0);
                assert_eq!(
                    queries.prove(
                        QueryPurpose::NavigationTest,
                        (40, 1),
                        from,
                        goal,
                        |_| true,
                        |_| true
                    ),
                    Some(true)
                );
                assert_eq!(
                    queries.prove(
                        QueryPurpose::NavigationTest,
                        (40, 1),
                        goal,
                        from,
                        |_| true,
                        |_| true
                    ),
                    Some(true)
                );
            }
        });
        assert_eq!(work.fields, 1);
        assert_eq!(work.paths, 0);
        let mut queries = SafetyQueries::default();
        for x in 0..300 {
            let point = TilePos::new(x, 0);
            queries.record(point, point, 300);
            queries.prove(
                QueryPurpose::NavigationTest,
                (300, 1),
                point,
                point,
                |_| true,
                |_| true,
            );
        }
        assert_eq!(queries.fields.len(), RETAINED_FIELDS);
        assert_eq!(queries.work.len(), TRACKED_ENDPOINTS);
        assert_eq!(
            queries.prove(
                QueryPurpose::NavigationTest,
                (256, 256),
                source,
                goal,
                |_| true,
                |_| true
            ),
            None
        );
    }

    #[test]
    fn a_dangerous_shortest_tie_requires_canonical_inspection() {
        let mut queries = SafetyQueries::default();
        let from = TilePos::new(0, 0);
        let to = TilePos::new(2, 1);
        queries.record(from, to, 6);
        assert_eq!(
            queries.prove(
                QueryPurpose::NavigationTest,
                (3, 2),
                from,
                to,
                |_| true,
                |tile| tile != TilePos::new(1, 0)
            ),
            None
        );
        assert_eq!(queries.fields.len(), 1);
    }

    #[test]
    fn longer_safe_detour_does_not_make_a_shortest_command_route_safe() {
        let mut queries = SafetyQueries::default();
        let from = TilePos::new(0, 1);
        let to = TilePos::new(4, 1);
        let safe = |tile: TilePos| tile != TilePos::new(2, 1);
        assert!(super::super::flood::reaches_any(
            5,
            3,
            [from],
            safe,
            |tile| tile == to
        ));
        queries.record(from, to, 15);
        assert_eq!(
            queries.prove(
                QueryPurpose::NavigationTest,
                (5, 3),
                from,
                to,
                |_| true,
                safe
            ),
            Some(false)
        );
    }
}
