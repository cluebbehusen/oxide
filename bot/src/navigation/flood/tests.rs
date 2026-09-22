use super::*;
use crate::query_work::QueryPurpose;

#[test]
fn connectivity_matches_bounded_astar_on_every_small_layout() {
    for mask in 0..512 {
        let open =
            |tile: TilePos| tile_index(3, 3, tile).is_some_and(|index| mask & (1 << index) == 0);
        let tiles: Vec<_> = (0..9)
            .map(|index| TilePos::new(index % 3, index / 3))
            .collect();
        let passable: Vec<_> = tiles.iter().map(|tile| open(*tile)).collect();
        let components = labels(QueryPurpose::NavigationTest, &passable, (3, 3));
        for start in &tiles {
            let reachable = component(3, 3, *start, open).unwrap();
            let sparse = component_tiles(3, 3, *start, open);
            for goal in &tiles {
                let expected = chassis::path::astar(3, 3, *start, *goal, open, 100).is_some();
                let index = tile_index(3, 3, *goal).unwrap();
                assert_eq!(sparse.contains(goal), expected);
                assert_eq!(reachable[index], expected, "{mask} {start:?} {goal:?}");
                assert_eq!(
                    reaches_any(3, 3, [*start], open, |tile| tile == *goal),
                    expected
                );
                if open(*start) && open(*goal) {
                    assert_eq!(
                        components[tile_index(3, 3, *start).unwrap()] == components[index],
                        expected
                    );
                    let path = cardinal_path(&passable, (3, 3), *start, *goal);
                    assert_eq!(path.is_some(), expected);
                    if let Some(path) = path {
                        assert_eq!(path.first(), Some(start));
                        assert_eq!(path.last(), Some(goal));
                        assert!(path.iter().all(|tile| open(*tile)));
                        assert!(
                            path.windows(2)
                                .all(|p| p[0].x.abs_diff(p[1].x) + p[0].y.abs_diff(p[1].y) == 1)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn cardinal_witness_keeps_original_ties_and_step_bound_keeps_diagonal_rules() {
    assert_eq!(
        cardinal_path(&[true; 9], (3, 3), TilePos::new(0, 0), TilePos::new(1, 1)),
        Some(vec![
            TilePos::new(0, 0),
            TilePos::new(1, 0),
            TilePos::new(1, 1)
        ])
    );
    for mask in 0..512 {
        let open =
            |tile: TilePos| tile_index(3, 3, tile).is_some_and(|index| mask & (1 << index) == 0);
        for limit in 0..4 {
            let starts = [TilePos::new(0, 0), TilePos::new(2, 2)];
            let mut expected = vec![false; 9];
            expected[0] = true;
            expected[8] = true;
            for _ in 0..limit {
                let old = expected.clone();
                for (index, reached) in old.iter().enumerate() {
                    if !reached {
                        continue;
                    }
                    let from = TilePos::new(index as i32 % 3, index as i32 / 3);
                    for (dx, dy) in CARDINALS.into_iter().chain(DIAGONALS) {
                        let to = from.offset(dx, dy);
                        if open(to)
                            && (dx == 0
                                || dy == 0
                                || (open(from.offset(dx, 0)) && open(from.offset(0, dy))))
                        {
                            expected[tile_index(3, 3, to).unwrap()] = true;
                        }
                    }
                }
            }
            assert_eq!(within_steps(3, 3, &starts, open, limit), expected);
        }
    }
}

#[test]
fn pooled_floods_isolate_nested_queries_and_changed_knowledge() {
    let start = TilePos::new(0, 0);
    let goal = TilePos::new(3, 0);
    assert!(reaches_any(
        4,
        1,
        [start],
        |_| {
            assert!(!reaches_any(4, 1, [start], |_| false, |tile| tile == goal));
            true
        },
        |tile| tile == goal
    ));
    assert_eq!(
        component(4, 1, start, |_| false),
        Some(vec![true, false, false, false])
    );
    assert_eq!(component(0, 1, start, |_| true), None);
    assert!(component_tiles(0, 1, start, |_| true).is_empty());
    assert!(!reaches_any(
        4,
        1,
        [TilePos::new(-1, 0)],
        |_| true,
        |_| true
    ));
    assert_eq!(
        cardinal_path(&[true; 4], (4, 1), start, TilePos::new(4, 0)),
        None
    );
    assert_eq!(cardinal_path(&[false; 4], (4, 1), start, goal), None);
}

#[test]
fn monotone_witness_matches_canonical_breadth_first_paths() {
    fn reference(open: &[bool], width: i32, start: TilePos, goal: TilePos) -> Option<Vec<TilePos>> {
        let index = |tile: TilePos| (tile.y * width + tile.x) as usize;
        let mut parent = vec![None; open.len()];
        let mut queue = VecDeque::from([start]);
        parent[index(start)] = Some(start);
        while let Some(tile) = queue.pop_front() {
            if tile == goal {
                break;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = tile.offset(dx, dy);
                let Some(i) = tile_index(width, open.len() as i32 / width, next) else {
                    continue;
                };
                if open[i] && parent[i].is_none() {
                    parent[i] = Some(tile);
                    queue.push_back(next);
                }
            }
        }
        parent[index(goal)]?;
        let mut route = vec![goal];
        while *route.last().unwrap() != start {
            route.push(parent[index(*route.last().unwrap())].unwrap());
        }
        route.reverse();
        Some(route)
    }
    for mask in 0..512 {
        let open: Vec<_> = (0..9).map(|i| mask & (1 << i) == 0).collect();
        for start in 0..9 {
            for goal in 0..9 {
                if !open[start] || !open[goal] {
                    continue;
                }
                let start = TilePos::new(start as i32 % 3, start as i32 / 3);
                let goal = TilePos::new(goal as i32 % 3, goal as i32 / 3);
                if let Some(route) = monotone_cardinal_path(&open, 3, start, goal) {
                    assert_eq!(Some(route), reference(&open, 3, start, goal));
                }
            }
        }
    }
    let start = TilePos::new(0, 0);
    let goal = TilePos::new(159, 159);
    let mut open = vec![true; 160 * 160];
    let (path, work) =
        super::super::work::measure(|| cardinal_path(&open, (160, 160), start, goal));
    assert_eq!(path, reference(&open, 160, start, goal));
    assert!(work.expanded <= 320, "{work:?}");
    for y in 0..159 {
        open[y * 160 + 80] = false;
    }
    assert_eq!(
        cardinal_path(&open, (160, 160), start, goal),
        reference(&open, 160, start, goal)
    );
    open[159 * 160 + 80] = false;
    assert_eq!(cardinal_path(&open, (160, 160), start, goal), None);
}
