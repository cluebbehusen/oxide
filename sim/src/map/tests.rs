use super::*;

#[test]
fn parses_legend_and_anchors() {
    let (map, anchors) = Map::parse(&["#.s", ".1.", "..2"]).unwrap();
    assert_eq!(map.width(), 3);
    assert_eq!(map.height(), 3);
    assert!(!map.terrain_passable(TilePos::new(0, 0)));
    assert!(map.terrain_passable(TilePos::new(1, 0)));
    assert!(!map.terrain_passable(TilePos::new(2, 0)), "scrap blocks");
    assert_eq!(map.scrap_at(TilePos::new(2, 0)), SCRAP_NODE_AMOUNT);
    // Anchor tiles are plain ground.
    assert!(map.terrain_passable(TilePos::new(1, 1)));
    assert_eq!(
        anchors,
        vec![
            (PlayerId(0), TilePos::new(1, 1)),
            (PlayerId(1), TilePos::new(2, 2)),
        ]
    );
}

#[test]
fn depleted_node_becomes_passable() {
    let (mut map, _) = Map::parse(&["s"]).unwrap();
    let pos = TilePos::new(0, 0);
    for left in (0..SCRAP_NODE_AMOUNT).rev() {
        assert_eq!(map.extract_scrap(pos), Some(left));
    }
    assert_eq!(map.extract_scrap(pos), None);
    assert!(map.terrain_passable(pos));
}

#[test]
fn rejects_ragged_and_unknown() {
    assert!(matches!(
        Map::parse(&["..", "..."]),
        Err(MapError::Ragged { row: 1, .. })
    ));
    assert!(matches!(
        Map::parse(&["..", ".x"]),
        Err(MapError::UnknownChar { c: 'x', x: 1, y: 1 })
    ));
    assert!(matches!(
        Map::parse(&["11"]),
        Err(MapError::DuplicateAnchor(PlayerId(0)))
    ));
}

#[test]
fn ascii_roundtrip() {
    let rows = ["#.s", ",S.", "s#."];
    let (map, _) = Map::parse(&rows).unwrap();
    assert_eq!(map.ascii_rows(), rows);
}
