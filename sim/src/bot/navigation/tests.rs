use super::*;

fn oracle(
    grid: KnownGrid<'_>,
    overlay: Option<BlockedRect>,
    starts: &[TilePos],
    goals: &[TilePos],
    limit: u32,
) -> Option<u32> {
    starts
        .iter()
        .flat_map(|start| {
            goals.iter().filter_map(move |goal| {
                let route = chassis::path::astar(
                    grid.width,
                    grid.height,
                    *start,
                    *goal,
                    |tile| grid.open(tile, overlay),
                    limit,
                )?;
                let mut previous = *start;
                Some(
                    route
                        .into_iter()
                        .map(|tile| {
                            let cost = octile(previous, tile);
                            previous = tile;
                            cost
                        })
                        .sum(),
                )
            })
        })
        .min()
}

#[test]
fn endpoint_sets_match_all_pairs_in_every_small_blocker_layout() {
    let starts = [TilePos::new(0, 0), TilePos::new(0, 1), TilePos::new(0, 2)];
    let goals = [TilePos::new(2, 0), TilePos::new(2, 1), TilePos::new(2, 2)];
    let overlay = BlockedRect {
        anchor: TilePos::new(1, 1),
        size: (1, 1),
    };
    let mut queries = CostQueries::default();
    for mask in 0..512 {
        let blocked: Vec<_> = (0..9).map(|bit| mask & (1 << bit) != 0).collect();
        let grid = KnownGrid::new(3, 3, &blocked).unwrap();
        for overlay in [None, Some(overlay)] {
            let result = queries.between_sets(grid, overlay, &starts, &goals);
            assert_eq!(
                result.available_cost(),
                oracle(grid, overlay, &starts, &goals, 9),
                "layout {mask}, overlay {overlay:?}"
            );
            assert!(!matches!(
                result,
                CostResult::Bounded(_) | CostResult::SearchLimit
            ));
        }
    }
    assert_eq!(queries.work().pair_searches, 0);
}

#[test]
fn blocked_starts_overlap_invalid_endpoints_and_corner_rules_match_astar() {
    let blocked = [true, true, false, true, false, false, false, false, false];
    let grid = KnownGrid::new(3, 3, &blocked).unwrap();
    let mut queries = CostQueries::default();
    for (starts, goals, expected) in [
        (vec![TilePos::new(0, 0)], vec![TilePos::new(0, 0)], Some(0)),
        (vec![TilePos::new(0, 0)], vec![TilePos::new(1, 1)], None),
        (vec![TilePos::new(1, 0)], vec![TilePos::new(1, 1)], Some(10)),
        (vec![TilePos::new(-1, 0)], vec![TilePos::new(1, 1)], None),
        (vec![TilePos::new(1, 1)], vec![TilePos::new(3, 1)], None),
        (vec![], vec![TilePos::new(1, 1)], None),
    ] {
        assert_eq!(
            queries
                .between_sets(grid, None, &starts, &goals)
                .available_cost(),
            expected
        );
        assert_eq!(expected, oracle(grid, None, &starts, &goals, 9));
    }
    assert!(KnownGrid::new(-1, 3, &[]).is_none());
    assert!(KnownGrid::new(0, 3, &[]).is_none());
    assert!(KnownGrid::new(3, 3, &[]).is_none());
}

#[test]
fn search_limit_remains_distinct_from_disconnection_and_partial_success() {
    let blocked = [false; 9];
    let grid = KnownGrid::new(3, 3, &blocked).unwrap();
    let mut queries = CostQueries::default();
    let mut scratch = Scratch::default();
    let query = Query::new(grid, None, &[TilePos::new(0, 0)], &[TilePos::new(2, 2)]);
    assert_eq!(
        queries.search(grid, &query, 0, &mut scratch),
        CostResult::SearchLimit
    );
    assert_eq!(
        queries.search(grid, &query, 9, &mut scratch),
        CostResult::Exact(28)
    );
    assert_eq!(queries.work().set_searches, 1);
    assert_eq!(queries.work().pair_searches, 1);
    let query = Query::new(
        grid,
        None,
        &[TilePos::new(0, 0)],
        &[TilePos::new(2, 2), TilePos::new(0, 0)],
    );
    assert_eq!(
        queries.search(grid, &query, 0, &mut scratch),
        CostResult::Exact(0)
    );

    let blocked = [false, true, false, false, true, false, false, true, false];
    let grid = KnownGrid::new(3, 3, &blocked).unwrap();
    let query = Query::new(grid, None, &[TilePos::new(0, 0)], &[TilePos::new(2, 2)]);
    assert_eq!(
        queries.search(grid, &query, 8, &mut scratch),
        CostResult::Unreachable
    );

    let blocked = [false, true, false, false, false, false, false, false, false];
    let grid = KnownGrid::new(3, 3, &blocked).unwrap();
    let query = Query::new(
        grid,
        None,
        &[TilePos::new(0, 0)],
        &[TilePos::new(2, 0), TilePos::new(0, 2)],
    );
    assert_eq!(
        queries.search(grid, &query, 2, &mut scratch),
        CostResult::Bounded(20)
    );
    assert_eq!(oracle(grid, None, &query.starts, &query.goals, 2), Some(20));
}

#[test]
fn cached_disconnection_is_invalidated_when_a_tile_opens() {
    let mut blocked = [false, false, true, false, false];
    let mut queries = CostQueries::default();
    let starts = [TilePos::new(0, 0)];
    let goals = [TilePos::new(4, 0)];
    let grid = KnownGrid::new(5, 1, &blocked).unwrap();
    assert_eq!(
        queries.between_sets(grid, None, &starts, &goals),
        CostResult::Unreachable
    );
    assert_eq!(
        queries.between_sets(grid, None, &starts, &goals),
        CostResult::Unreachable
    );
    assert_eq!(queries.work().hits, 1);
    blocked[2] = false;
    assert_eq!(
        queries.between_sets(
            KnownGrid::new(5, 1, &blocked).unwrap(),
            None,
            &starts,
            &goals
        ),
        CostResult::Exact(40)
    );
}

#[test]
fn eviction_and_oversized_grids_preserve_answers_without_unbounded_retention() {
    let blocked = [false; 4];
    let grid = KnownGrid::new(2, 2, &blocked).unwrap();
    let starts = [TilePos::new(0, 0)];
    let goals = [TilePos::new(1, 1)];
    let mut queries = CostQueries::default();
    for x in 2..6002 {
        let overlay = Some(BlockedRect {
            anchor: TilePos::new(x, 0),
            size: (1, 1),
        });
        assert_eq!(
            queries.between_sets(grid, overlay, &starts, &goals),
            CostResult::Exact(14)
        );
    }
    let generation = queries.generation.as_ref().unwrap();
    assert!(generation.bytes <= CACHE_BYTES);
    assert!(generation.answers.len() < 6000);
    assert_eq!(
        queries.between_sets(
            grid,
            Some(BlockedRect {
                anchor: TilePos::new(2, 0),
                size: (1, 1)
            }),
            &starts,
            &goals
        ),
        CostResult::Exact(14)
    );
    let oversized = vec![false; CACHE_BYTES + 1];
    let grid = KnownGrid::new(oversized.len() as i32, 1, &oversized).unwrap();
    assert_eq!(
        queries.between_sets(grid, None, &starts, &starts),
        CostResult::Exact(0)
    );
    assert!(queries.generation.is_none());
}

#[test]
fn normalized_sets_reuse_answers_but_opened_tiles_overlays_and_dimensions_do_not() {
    let mut blocked = vec![false; 12];
    blocked[5] = true;
    let mut queries = CostQueries::default();
    let starts = [TilePos::new(0, 0), TilePos::new(0, 1)];
    let goals = [TilePos::new(3, 0), TilePos::new(3, 1)];
    let grid = KnownGrid::new(4, 3, &blocked).unwrap();
    let first = queries.between_sets(grid, None, &starts, &goals);
    assert_eq!(
        queries.between_sets(
            grid,
            None,
            &[starts[1], starts[0], starts[1]],
            &[goals[1], goals[0]]
        ),
        first
    );
    assert_eq!(queries.work().set_searches, 1);
    assert_eq!(queries.work().hits, 1);
    let overlay = Some(BlockedRect {
        anchor: TilePos::new(2, 0),
        size: (1, 3),
    });
    assert_eq!(
        queries.between_sets(grid, overlay, &starts, &goals),
        CostResult::Unreachable
    );
    assert_eq!(queries.work().set_searches, 2);
    blocked[5] = false;
    let grid = KnownGrid::new(4, 3, &blocked).unwrap();
    queries.between_sets(grid, None, &starts, &goals);
    assert_eq!(queries.work().generations, 2);
    let grid = KnownGrid::new(3, 4, &blocked).unwrap();
    queries.between_sets(grid, None, &starts, &goals);
    assert_eq!(queries.work().generations, 3);
}

#[test]
fn a_half_turn_and_endpoint_permutation_preserve_cost() {
    let blocked = [
        false, true, false, false, false, false, false, true, false, false, false, false,
    ];
    let rotated: Vec<_> = blocked.iter().copied().rev().collect();
    let rotate = |p: TilePos| TilePos::new(3 - p.x, 2 - p.y);
    let starts = [TilePos::new(0, 0), TilePos::new(0, 2)];
    let goals = [TilePos::new(3, 0), TilePos::new(3, 2)];
    let mut queries = CostQueries::default();
    let cost = queries.between_sets(
        KnownGrid::new(4, 3, &blocked).unwrap(),
        None,
        &starts,
        &goals,
    );
    assert_eq!(
        queries.between_sets(
            KnownGrid::new(4, 3, &rotated).unwrap(),
            None,
            &starts.map(rotate),
            &goals.map(rotate)
        ),
        cost
    );
}
