use super::*;

fn frame(home: (i64, i64), map: (i64, i64)) -> HomeFrame {
    HomeFrame {
        home,
        radial: (home.0 - map.0, home.1 - map.1),
    }
}

#[test]
fn home_frame_ranks_nearer_targets_first_and_mirrors_its_ties() {
    let map = (24, 12);
    let rotate = |tile: TilePos| TilePos::new(23 - tile.x, 11 - tile.y);
    let west = frame((8, 12), map);
    let east = frame((40, 12), map);
    let nearest = |frame: HomeFrame, from: TilePos, nodes: &[TilePos]| {
        nodes
            .iter()
            .copied()
            .min_by_key(|node| frame.rank(doubled(from), doubled(*node)))
            .unwrap()
    };
    let harvester = TilePos::new(7, 6);
    let tied = [TilePos::new(7, 3), TilePos::new(7, 9)];

    assert_eq!(
        nearest(west, harvester, &[tied[0], tied[1], TilePos::new(9, 6)]),
        TilePos::new(9, 6)
    );
    let west_choice = nearest(west, harvester, &tied);
    assert_eq!(west_choice, TilePos::new(7, 9), "not broken row-major");
    assert_eq!(
        nearest(east, rotate(harvester), &tied.map(rotate)),
        rotate(west_choice)
    );
    let centred = frame(map, map);
    assert_eq!(nearest(centred, harvester, &tied), TilePos::new(7, 3));
}
