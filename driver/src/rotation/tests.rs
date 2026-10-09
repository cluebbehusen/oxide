use super::*;
use oxide_sim::State;

/// Rotating twice is the identity; rotating once moves Extractor frames by
/// their footprint and preserves open ground, scrap, units and buildings.
#[test]
fn rotation_is_an_involution_and_preserves_the_world() {
    let base = oxide_kit::runner::load_scenario("skirmish").unwrap();
    let once = rotate_180(&base).unwrap();
    assert_ne!(once.map, base.map, "the anchors actually moved");
    assert_eq!(rotate_180(&once).unwrap(), base);

    let (before_map, _) = oxide_sim::map::Map::parse(&base.map).unwrap();
    let (after_map, _) = oxide_sim::map::Map::parse(&once.map).unwrap();
    let (ew, eh) = BuildingKind::Extractor.size();
    let mut expected_frames: Vec<_> = before_map
        .extractor_frames()
        .iter()
        .map(|frame| chassis::grid::TilePos {
            x: before_map.width() - ew - frame.x,
            y: before_map.height() - eh - frame.y,
        })
        .collect();
    expected_frames.sort_by_key(|frame| (frame.y, frame.x));
    assert_eq!(after_map.extractor_frames(), expected_frames);

    let before = base.build().unwrap();
    let after = once.build().unwrap();
    let open = |state: &State| {
        let map = state.map();
        (0..map.height())
            .flat_map(|y| (0..map.width()).map(move |x| chassis::grid::TilePos::new(x, y)))
            .filter(|t| state.passable(*t))
            .count()
    };
    let scrap = |state: &State| {
        let map = state.map();
        (0..map.height())
            .flat_map(|y| (0..map.width()).map(move |x| chassis::grid::TilePos::new(x, y)))
            .filter_map(|t| map.tile(t))
            .map(|tile| u64::from(tile.scrap))
            .sum::<u64>()
    };
    assert_eq!(open(&before), open(&after));
    assert_eq!(scrap(&before), scrap(&after));
    assert_eq!(before.units().len(), after.units().len());
    assert_eq!(before.buildings().len(), after.buildings().len());
}

/// An asymmetric map is refused rather than rotated.
#[test]
fn an_asymmetric_map_refuses_rotation() {
    let mut base = oxide_kit::runner::load_scenario("skirmish").unwrap();
    let mut row: Vec<char> = base.map[10].chars().collect();
    let x = row
        .iter()
        .position(|c| *c == '.')
        .expect("the basin has open ground");
    row[x] = '#';
    base.map[10] = row.into_iter().collect();
    let err = rotate_180(&base).unwrap_err().to_string();
    assert!(err.contains("not 180-symmetric"), "{err}");
}
