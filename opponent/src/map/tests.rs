use super::*;

const SPLIT: [&str; 7] = [
    "##########",
    "#1..#....#",
    "#...#..s.#",
    "#...#s...#",
    "#.s.#....#",
    "#...#..2.#",
    "##########",
];

fn model(rows: &[&str]) -> MapModel {
    let (map, anchors) = Map::parse(rows).unwrap();
    MapModel::new(&map, &anchors, vec![None; 2])
}

#[test]
fn components_split_at_walls_and_scrap_touches_its_side() {
    let model = model(&SPLIT);
    let west = model.component(TilePos::new(1, 4)).unwrap();
    let east = model.component(TilePos::new(8, 4)).unwrap();
    assert_ne!(west, east);
    assert_eq!(model.component(TilePos::new(4, 3)), None, "rock");
    assert_eq!(model.component(TilePos::new(2, 4)), Some(west), "scrap");
    assert_eq!(
        model.component(model.start(PlayerId(0)).unwrap()),
        Some(west),
        "a Foundry footprint keeps its ground component"
    );
    assert!(model.touches(TilePos::new(2, 4), west));
    assert!(!model.touches(TilePos::new(2, 4), east));
    assert!(
        model.touches(TilePos::new(5, 3), east),
        "diagonal doorsteps count"
    );
    assert!(!model.touches(TilePos::new(5, 3), west), "not through rock");
    assert_eq!(model.start(PlayerId(1)), Some(TilePos::new(7, 5)));
    assert_eq!(model.start(PlayerId(2)), None);
}

/// Whether a `kind` at `spot` stands where the layout of `foundries`
/// packs it: a Foundry with its ring clear of lanes, a smaller building
/// beside a lane, and a two-by-two one beside lanes both ways.
fn packed(model: &MapModel, foundries: &[TilePos], kind: BuildingKind, spot: TilePos) -> bool {
    let (width, height) = kind.base_stats().size;
    let laned = |tile: TilePos| model.lane_of(foundries, tile);
    let across = (0..height).any(|dy| laned(spot.offset(-1, dy)) || laned(spot.offset(width, dy)));
    let along = (0..width).any(|dx| laned(spot.offset(dx, -1)) || laned(spot.offset(dx, height)));
    match kind {
        BuildingKind::Foundry => crate::frame::ring(spot, (width, height)).all(|tile| !laned(tile)),
        _ if width == 1 => across || along,
        _ => across && along,
    }
}

#[test]
fn spots_pack_blocks_beside_lanes_clear_of_foundry_rings_and_mirror_between_seats() {
    const ROOM: [&str; 16] = [
        "####################",
        "#..................#",
        "#..................#",
        "#..s...............#",
        "#..................#",
        "#....1.............#",
        "#..................#",
        "#..................#",
        "#..................#",
        "#............2.....#",
        "#..................#",
        "#..................#",
        "#...............s..#",
        "#..................#",
        "#..................#",
        "####################",
    ];
    let model = model(&ROOM);
    let start = model.start(PlayerId(0)).unwrap();
    let spots = |seat: u8, kind: BuildingKind| {
        let start = model.start(PlayerId(seat)).unwrap();
        model
            .spots(PlayerId(seat), vec![start], kind)
            .collect::<Vec<_>>()
    };
    let tiles = |anchor: TilePos, (width, height): (i32, i32)| {
        (0..height).flat_map(move |dy| (0..width).map(move |dx| anchor.offset(dx, dy)))
    };
    let foundry = BuildingKind::Foundry.base_stats().size;
    let laid = |kind: BuildingKind, spot: TilePos| packed(&model, &[start], kind, spot);
    for kind in [
        BuildingKind::Fabricator,
        BuildingKind::Reclaimer,
        BuildingKind::Foundry,
    ] {
        let size = kind.base_stats().size;
        let (west, east) = (spots(0, kind), spots(1, kind));
        let first = west.iter().take_while(|spot| laid(kind, **spot)).count();
        assert!(first > 0, "{kind:?}: {west:?}");
        for spot in &west {
            assert!(
                tiles(*spot, size).all(|tile| !lane(start, tile)),
                "{kind:?} at {spot:?} stands on a lane"
            );
            assert!(
                crate::frame::gap(start, foundry, *spot, size) >= 1,
                "{kind:?} at {spot:?} crowds the Foundry's ring"
            );
        }
        let distances: Vec<i32> = west[..first]
            .iter()
            .map(|spot| chebyshev(*spot, start))
            .collect();
        if size == (2, 2) {
            assert!(distances.is_sorted(), "nearest first: {distances:?}");
        }
        assert!(
            west[first..].iter().all(|spot| !laid(kind, *spot)) || size == (1, 1),
            "the layout's places come first: {west:?}"
        );
        let rotate = |anchor: TilePos| TilePos::new(20 - size.0 - anchor.x, 16 - size.1 - anchor.y);
        assert_eq!(
            west.iter().copied().map(rotate).collect::<Vec<_>>(),
            east,
            "{kind:?}"
        );
    }
    assert_eq!(
        model
            .spots(
                PlayerId(2),
                vec![TilePos::new(9, 7)],
                BuildingKind::Fabricator
            )
            .count(),
        0,
        "a seat without a start lists none"
    );
}

#[test]
fn the_layout_mirrors_under_a_reflection_too() {
    // West and east face each other across a vertical mirror line.
    const MIRROR: [&str; 12] = [
        "######################",
        "#....................#",
        "#....................#",
        "#....................#",
        "#....................#",
        "#..1.............2...#",
        "#....................#",
        "#....................#",
        "#....................#",
        "#....................#",
        "#....................#",
        "######################",
    ];
    let model = model(&MIRROR);
    let width = 22;
    let west = model.start(PlayerId(0)).unwrap();
    let east = model.start(PlayerId(1)).unwrap();
    assert_eq!(east, TilePos::new(width - 2 - west.x, west.y));
    for y in 0..12 {
        for x in 0..width {
            let tile = TilePos::new(x, y);
            let mirrored = TilePos::new(width - 1 - x, y);
            assert_eq!(lane(west, tile), lane(east, mirrored), "{tile:?}");
        }
    }
}

#[test]
fn a_spot_belongs_to_the_nearest_foundry_on_its_own_ground() {
    // Rock splits the field: the west Foundry's spots run up to the
    // wall, nearer the east Foundry across it than the west one.
    const SPLIT: [&str; 10] = [
        "##############################",
        "#.............##.............#",
        "#.............##.............#",
        "#.............##.............#",
        "#..1..........##.............#",
        "#.............##.............#",
        "#.............##.............#",
        "#.............##.............#",
        "#.............##.............#",
        "##############################",
    ];
    let model = model(&SPLIT);
    let west = TilePos::new(5, 4);
    let east = TilePos::new(16, 4);
    assert_ne!(model.component(west), model.component(east));
    let beside_wall = TilePos::new(11, 5);
    assert!(
        chebyshev(beside_wall, east) < chebyshev(beside_wall, west),
        "premise: nearer the east Foundry"
    );
    assert!(
        model
            .spots(PlayerId(0), vec![west, east], BuildingKind::Fabricator)
            .any(|spot| spot == beside_wall)
    );
}

#[test]
fn spots_beside_every_foundry_come_before_further_ones_beside_any() {
    const FIELD: [&str; 12] = [
        "########################################",
        "#......................................#",
        "#......................................#",
        "#......................................#",
        "#......................................#",
        "#...1..............................2...#",
        "#......................................#",
        "#......................................#",
        "#......................................#",
        "#......................................#",
        "#......................................#",
        "########################################",
    ];
    let model = model(&FIELD);
    let start = model.start(PlayerId(0)).unwrap();
    let expansion = TilePos::new(20, 5);
    let distance = |a: TilePos, b: TilePos| (a.x - b.x).abs().max((a.y - b.y).abs());
    let spots: Vec<TilePos> = model
        .spots(
            PlayerId(0),
            vec![start, expansion],
            BuildingKind::Fabricator,
        )
        .collect();
    let nearest = |spot: TilePos| distance(spot, start).min(distance(spot, expansion));
    let first = spots
        .iter()
        .take_while(|spot| {
            packed(
                &model,
                &[start, expansion],
                BuildingKind::Fabricator,
                **spot,
            )
        })
        .count();
    let rings: Vec<i32> = spots[..first].iter().map(|spot| nearest(*spot)).collect();
    assert!(rings.is_sorted(), "{rings:?}");
    assert!(
        spots
            .iter()
            .any(|spot| distance(*spot, expansion) == FIRST_GAP + 2),
        "the expansion has spots at the first gap too"
    );
    assert_eq!(
        spots.first().map(|spot| distance(*spot, start)),
        Some(FIRST_GAP + 2),
        "home goes first at a gap"
    );
}

#[test]
fn a_scrap_choke_joins_what_it_will_open_once_mined() {
    const CHOKE: [&str; 5] = [
        "##########",
        "#1..#....#",
        "#...s..2.#",
        "#...#....#",
        "##########",
    ];
    let model = model(&CHOKE);
    assert_eq!(
        model.component(TilePos::new(1, 3)),
        model.component(TilePos::new(6, 1))
    );
}

#[test]
fn every_shipped_scenario_builds_a_model_with_every_start_on_ground() {
    let scenarios = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        });
    for path in scenarios {
        let scenario = Scenario::load(&path).unwrap();
        let model = MapModel::from_scenario(&scenario).unwrap();
        for seat in 0..scenario.players.len() {
            let start = model.start(PlayerId::from_index(seat)).unwrap();
            assert!(model.component(start).is_some(), "{}", path.display());
            assert!(
                model
                    .spots(
                        PlayerId::from_index(seat),
                        vec![start],
                        BuildingKind::Fabricator
                    )
                    .take(8)
                    .count()
                    == 8,
                "{} seat {seat}",
                path.display()
            );
        }
    }
}

/// Two rooms joined by a corridor three tiles wide.
pub(crate) const CORRIDOR: [&str; 15] = [
    "########################################",
    "#..............##########..............#",
    "#..............##########..............#",
    "#..s...........##########...........s..#",
    "#..............##########..............#",
    "#..............##########..............#",
    "#................................2.....#",
    "#....1.................................#",
    "#......................................#",
    "#..............##########..............#",
    "#..............##########..............#",
    "#..s...........##########...........s..#",
    "#..............##########..............#",
    "#..............##########..............#",
    "########################################",
];

/// [`CORRIDOR`] with the corridor walled up and `rows` opened across the
/// wall instead.
pub(crate) fn ways(rows: &[usize]) -> Vec<String> {
    let mut map = CORRIDOR.map(str::to_owned);
    for row in [6, 7, 8] {
        let mut cells: Vec<char> = map[row].chars().collect();
        for cell in &mut cells[15..=24] {
            *cell = '#';
        }
        map[row] = cells.into_iter().collect();
    }
    for row in rows {
        map[*row] = map[*row].replace("##########", "..........");
    }
    map.to_vec()
}

/// Each seat's narrowest cut across its way to the other, from any
/// distance.
fn cuts(rows: &[String]) -> Vec<Option<(u16, Vec<Vec<TilePos>>)>> {
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let model = model(&rows);
    (0..2)
        .map(|seat| {
            model.cut(PlayerId(seat), PlayerId(1 - seat), 0).map(|cut| {
                (
                    cut.distance,
                    cut.gates.iter().map(|gate| gate.tiles.clone()).collect(),
                )
            })
        })
        .collect()
}

/// `gates` turned a half-turn about the middle of a 40 by 15 map.
fn rotated(gates: &[Vec<TilePos>]) -> Vec<Vec<TilePos>> {
    let mut rotated: Vec<Vec<TilePos>> = gates
        .iter()
        .map(|tiles| {
            let mut tiles: Vec<TilePos> = tiles
                .iter()
                .map(|tile| TilePos::new(39 - tile.x, 14 - tile.y))
                .collect();
            tiles.sort_unstable();
            tiles
        })
        .collect();
    rotated.sort_by_key(|tiles| tiles[0]);
    rotated
}

#[test]
fn a_corridor_between_two_rooms_is_a_cut_mirrored_between_seats() {
    let corridor: Vec<String> = CORRIDOR.map(str::to_owned).to_vec();
    let cuts = cuts(&corridor);
    let (distance, west) = cuts[0].clone().expect("the corridor is a cut");
    assert_eq!(west.len(), 1, "{west:?}");
    assert!(
        west[0]
            .iter()
            .all(|tile| (15..=24).contains(&tile.x) && (6..=8).contains(&tile.y)),
        "{west:?}"
    );
    assert_eq!(west[0].len(), 6, "three across, two deep: {west:?}");
    assert_eq!(cuts[1], Some((distance, rotated(&west))));
    let model = model(&CORRIDOR);
    assert!(model.gated(PlayerId(0), west[0][0]));
    assert!(!model.gated(PlayerId(0), TilePos::new(5, 3)));
    assert!(
        model
            .cut(PlayerId(0), PlayerId(1), distance + 200)
            .is_none(),
        "no cut lies past half the way"
    );
}

#[test]
fn two_ways_through_a_wall_are_one_cut_of_two_gates() {
    let cuts = cuts(&ways(&[3, 4, 10, 11]));
    let (distance, west) = cuts[0].clone().expect("the two ways are a cut");
    assert_eq!(west.len(), 2, "{west:?}");
    for (gate, rows) in west.iter().zip([3..=4, 10..=11]) {
        assert!(
            gate.iter()
                .all(|tile| (15..=24).contains(&tile.x) && rows.contains(&tile.y)),
            "{west:?}"
        );
    }
    assert_eq!(cuts[1], Some((distance, rotated(&west))));
}

#[test]
fn a_corridor_with_a_long_way_round_is_no_cut() {
    // A path from the west room's corner down, along the bottom and up
    // into the east room, far longer than the corridor.
    let mut map: Vec<String> = CORRIDOR.map(str::to_owned).to_vec();
    let mut last: Vec<char> = map[14].chars().collect();
    last[1] = '.';
    last[38] = '.';
    map[14] = last.into_iter().collect();
    for _ in 0..8 {
        map.push(format!("#.{}.#", "#".repeat(36)));
    }
    map.push(format!("#{}#", ".".repeat(38)));
    map.push("#".repeat(40));
    let rows: Vec<&str> = map.iter().map(String::as_str).collect();
    let model = model(&rows);
    assert!(model.cut(PlayerId(0), PlayerId(1), 0).is_none());
}

#[test]
fn no_gate_holds_an_extractor_frame() {
    let mut map: Vec<String> = CORRIDOR.map(str::to_owned).to_vec();
    let mut row: Vec<char> = map[7].chars().collect();
    row[15] = 'E';
    map[7] = row.into_iter().collect();
    let rows: Vec<&str> = map.iter().map(String::as_str).collect();
    let model = model(&rows);
    let cut = model
        .cut(PlayerId(0), PlayerId(1), 0)
        .expect("the corridor further on is still a cut");
    assert!(
        cut.gates
            .iter()
            .flat_map(|gate| &gate.tiles)
            .all(|tile| !(15..=16).contains(&tile.x) || !(7..=8).contains(&tile.y)),
        "{cut:?}"
    );
}

#[test]
fn open_ground_and_many_ways_have_no_cut() {
    let open: Vec<String> = CORRIDOR
        .iter()
        .map(|row| row.replace("##########", ".........."))
        .collect();
    let narrow: Vec<String> = ARENA_ROWS.iter().map(|row| (*row).to_owned()).collect();
    for rows in [open, narrow, ways(&[2, 5, 9, 12])] {
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let model = model(&rows);
        for seat in 0..2 {
            assert!(
                model.cut(PlayerId(seat), PlayerId(1 - seat), 0).is_none(),
                "{rows:#?}"
            );
        }
    }
}

/// A field ten tiles across all the way: narrow, but no narrower anywhere.
const ARENA_ROWS: [&str; 12] = [
    "########################",
    "#......................#",
    "#...............s......#",
    "#......s...............#",
    "#......................#",
    "#..1...............2...#",
    "#......................#",
    "#......................#",
    "#...............s......#",
    "#......s...............#",
    "#......................#",
    "########################",
];
