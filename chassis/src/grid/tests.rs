use super::*;

#[test]
fn center_and_containing_are_inverse_on_tile_centers() {
    for pos in [TilePos::new(0, 0), TilePos::new(3, 7), TilePos::new(41, 2)] {
        assert_eq!(TilePos::containing(pos.center()), pos);
    }
}

#[test]
fn containing_floors_negative_coordinates() {
    let just_negative = Vec2Fx::new(Fx::lit("-0.25"), Fx::lit("-1.5"));
    assert_eq!(TilePos::containing(just_negative), TilePos::new(-1, -2));
}

#[test]
fn grid_bounds_and_indexing() {
    let mut grid = Grid::new(4, 3, 0u8);
    assert!(grid.in_bounds(TilePos::new(3, 2)));
    assert!(!grid.in_bounds(TilePos::new(4, 0)));
    assert!(!grid.in_bounds(TilePos::new(0, -1)));
    *grid.get_mut(TilePos::new(2, 1)).unwrap() = 9;
    assert_eq!(grid.get(TilePos::new(2, 1)), Some(&9));
    assert_eq!(grid.get(TilePos::new(-1, 0)), None);
}

#[test]
fn iter_is_row_major() {
    let grid = Grid::from_cells(2, 2, vec![10, 11, 12, 13]);
    let order: Vec<_> = grid.iter().map(|(p, &v)| (p.x, p.y, v)).collect();
    assert_eq!(order, vec![(0, 0, 10), (1, 0, 11), (0, 1, 12), (1, 1, 13)]);
}

#[test]
fn bulk_rows_clamp_spans_and_refuse_rows_outside_the_grid() {
    let mut grid = Grid::new(4, 3, 0u8);
    grid.fill_row_span(1, -3, 2, 7);
    assert_eq!(grid.row(1), Some([7, 7, 7, 0].as_slice()));

    grid.fill_row_span(-1, 0, 3, 9);
    grid.fill_row_span(2, 8, 10, 9);
    assert_eq!(grid.row(0), Some([0, 0, 0, 0].as_slice()));
    assert_eq!(grid.row(2), Some([0, 0, 0, 0].as_slice()));
    assert!(grid.row(-1).is_none());
    assert!(grid.row_mut(3).is_none());

    grid.row_mut(2).unwrap().copy_from_slice(&[1, 2, 3, 4]);
    assert_eq!(grid.row(2), Some([1, 2, 3, 4].as_slice()));
}

#[test]
fn copy_from_adopts_the_source_shape_and_cells() {
    let mut destination = Grid::new(1, 1, 0u8);
    let source = Grid::from_cells(3, 2, vec![1, 2, 3, 4, 5, 6]);
    destination.copy_from(&source);

    assert_eq!(destination.width(), 3);
    assert_eq!(destination.height(), 2);
    assert_eq!(destination, source);
    assert!(destination.is_consistent());
}
