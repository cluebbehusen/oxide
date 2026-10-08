use super::*;
use crate::scenario::Scenario;

/// Every window the index serves must equal a brute-force filter of
/// the unit list, entry for entry — the order is load-bearing (the
/// collision resolver applies corrections in visit order).
fn assert_row_spans_match_a_full_filter(state: &State, index: &UnitIndex) {
    let reference = |y: i32, x_min: i32, x_max: i32| -> Vec<(TilePos, usize)> {
        let mut hits: Vec<(TilePos, usize)> = state
            .units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.hp > 0)
            .map(|(slot, u)| (u.tile(), slot))
            .filter(|(t, _)| t.y == y && t.x >= x_min && t.x <= x_max)
            .collect();
        hits.sort_unstable_by_key(|&(t, slot)| (t.x, slot));
        hits
    };
    let width = state.map.width();
    let height = state.map.height();
    let mut nonempty = 0;
    for y in -1..=height {
        for x in -1..=width {
            let got = index.row_span(y, x - 1, x + 1);
            assert_eq!(got, &reference(y, x - 1, x + 1)[..], "window ({y}, {x})");
            nonempty += usize::from(!got.is_empty());
        }
        let whole = index.row_span(y, i32::MIN, i32::MAX);
        assert_eq!(whole, &reference(y, i32::MIN, i32::MAX)[..], "row {y}");
    }
    assert!(nonempty > 0, "the fixture exercised no occupied windows");
}

/// A state whose bodies sit on both sides of the map, border rows
/// included, so the occupied rectangle is the whole map and then some.
fn spread_skirmish() -> State {
    let mut state = Scenario::skirmish().build().expect("skirmish builds");
    for _ in 0..90 {
        state.tick(&[]);
    }
    let width = state.map.width();
    let height = state.map.height();
    for (slot, tile) in [
        TilePos::new(-1, -1),
        TilePos::new(width, height),
        TilePos::new(-1, 2),
        TilePos::new(width, 3),
    ]
    .into_iter()
    .enumerate()
    {
        state.units[slot].pos = tile.center();
    }
    state
        .validate_invariants()
        .expect("the accepted coordinate envelope includes border rows");
    state
}

#[test]
fn row_spans_match_a_full_filter_in_either_layout() {
    let mut state = spread_skirmish();
    let mut index = UnitIndex::new();
    index.rebuild(&state.units);
    assert_eq!(index.layout, Layout::Rows, "a sparse map keeps row buckets");
    assert_row_spans_match_a_full_filter(&state, &index);

    // Pack the same bodies into a block across the corner, stacking
    // several per tile, until the rectangle is small enough for tiles.
    for (slot, unit) in state.units.iter_mut().enumerate() {
        let slot = i32::try_from(slot).unwrap() / 2;
        unit.pos = TilePos::new(slot % 3 - 1, slot / 3 - 1).center();
    }
    index.rebuild(&state.units);
    assert_eq!(index.layout, Layout::Tiles, "a crowd gets tile buckets");
    assert_row_spans_match_a_full_filter(&state, &index);
}

/// The accepted envelope reaches far past any map; a body out there
/// must not make the index pay for the empty rectangle between.
#[test]
fn bodies_at_the_envelope_corners_keep_the_index_small() {
    let mut state = Scenario::skirmish().build().expect("skirmish builds");
    let near = TilePos::new(-2048, -2048);
    let far = TilePos::new(2047, 2047);
    state.units[0].pos = near.center();
    state.units[1].pos = far.center();
    state
        .validate_invariants()
        .expect("the envelope accepts both corners");

    let mut index = UnitIndex::new();
    index.rebuild(&state.units);
    assert_eq!(index.layout, Layout::Rows);
    assert_eq!(index.starts.len(), 4096 + 1);
    for (slot, tile) in [(0, near), (1, far)] {
        assert_eq!(index.row_span(tile.y, tile.x, tile.x), &[(tile, slot)]);
    }
}

/// The survey may over-report but never miss: every hostile body or
/// building within reach of a tile must raise its flag, or acquisition
/// would skip a target the full scan finds.
#[test]
fn the_survey_never_misses_a_hostile_near_a_tile() {
    let mut state = spread_skirmish();
    let width = state.map.width();
    let height = state.map.height();
    // A footprint across a cell corner must mark every cell it covers.
    let corner = TilePos::new(
        CELL * (width / CELL / 2) - 1,
        CELL * (height / CELL / 2) - 1,
    );
    state.place_site(
        crate::ids::PlayerId(1),
        crate::stats::BuildingKind::Foundry,
        corner,
    );

    let mut index = UnitIndex::new();
    index.rebuild(&state.units);
    index.survey(&state);
    let mut ruled_out = 0;
    for team in [0, 1] {
        for y in -1..=height {
            for x in -1..=width {
                for reach in [0, 3, 9] {
                    let home = TilePos::new(x, y);
                    let near = |tile: TilePos| {
                        (tile.x - home.x).abs() <= reach && (tile.y - home.y).abs() <= reach
                    };
                    let foreign = |player| state.player(player).team != team;
                    let bodies = state
                        .units
                        .iter()
                        .any(|u| u.hp > 0 && foreign(u.player) && near(u.tile()));
                    let buildings = state
                        .buildings()
                        .iter()
                        .any(|b| foreign(b.player) && b.tiles().any(near));
                    let flagged = index.hostiles_near(team, home, reach);
                    assert!(flagged.bodies || !bodies, "body near {home:?}");
                    assert!(flagged.buildings || !buildings, "building near {home:?}");
                    ruled_out += usize::from(!flagged.bodies && !flagged.buildings);
                }
            }
        }
    }
    assert!(ruled_out > 0, "the survey never ruled anything out");
}

#[test]
fn buildings_placed_after_the_survey_are_left_to_a_full_scan() {
    let mut state = Scenario::skirmish().build().expect("skirmish builds");
    let mut index = UnitIndex::new();
    index.rebuild(&state.units);
    let all = state.buildings().len();
    assert_eq!(index.buildings_since_survey(state.buildings()).len(), all);
    index.survey(&state);
    assert!(index.buildings_since_survey(state.buildings()).is_empty());

    let anchor = TilePos::new(state.map.width() / 2, state.map.height() / 2);
    let site = state.place_site(
        crate::ids::PlayerId(1),
        crate::stats::BuildingKind::Turret,
        anchor,
    );
    let since: Vec<_> = index
        .buildings_since_survey(state.buildings())
        .iter()
        .map(|b| b.id)
        .collect();
    assert_eq!(since, [site]);

    index.rebuild(&state.units);
    assert_eq!(
        index.buildings_since_survey(state.buildings()).len(),
        all + 1
    );
    let unsurveyed = index.hostiles_near(0, anchor, 0);
    assert!(unsurveyed.bodies && unsurveyed.buildings);
}
