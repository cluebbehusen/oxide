use super::*;

const BASE: [i32; 3] = [2, 2, 2];

#[test]
fn depth_walks_lip_bench_vignette_void_in_order() {
    let mut seen = Vec::new();
    for depth in 1..=(MAX_DEPTH.fit::<u8>() + 1) {
        let material = material(depth, BASE).expect("pit cell");
        if seen.last() != Some(&material) {
            seen.push(material);
        }
    }
    assert_eq!(
        seen,
        [
            Material::Riser(0),
            Material::Bench(0),
            Material::Riser(1),
            Material::Bench(1),
            Material::Riser(2),
            Material::Bench(2),
            Material::Vignette(1),
            Material::Vignette(2),
            Material::Vignette(3),
            Material::Vignette(4),
            Material::Void,
        ]
    );
    assert_eq!(material(0, BASE), None);
}

#[test]
fn each_level_descends_toward_the_void() {
    let mut previous: Option<Layer> = None;
    for layer in DROP {
        assert!(layer.riser.r < layer.top.r);
        assert!(layer.top.r < layer.lip.r);
        if let Some(above) = previous {
            assert!(layer.top.r < above.top.r);
            assert!(layer.lip.r < above.lip.r);
        }
        previous = Some(layer);
    }
    assert!(VOID.r < DROP[DROP.len() - 1].top.r);
}

#[test]
fn bench_widths_jog_within_bounds_and_never_exceed_the_padding() {
    let mut distinct = std::collections::BTreeSet::new();
    for by in -8..8 {
        for bx in -8..8 {
            let widths = bench_widths(bx, by);
            assert!(widths.iter().all(|w| (1..=MAX_BENCH_CELLS).contains(w)));
            let depth: i32 = widths.iter().map(|w| 1 + w).sum::<i32>() + VIGNETTE_CELLS;
            assert!(depth <= MAX_DEPTH);
            distinct.insert(widths);
        }
    }
    assert!(distinct.len() > 1, "the rim never jogs");
}

fn flat_field(cells: i32) -> PitField {
    PitField {
        origin: TilePos::new(0, 0),
        width: cells / CELLS,
        height: cells / CELLS,
        depth: vec![u8::MAX; as_index(cells * cells)],
        known: vec![true; as_index((cells / CELLS) * (cells / CELLS))],
    }
}

#[test]
fn chessboard_distance_is_exact_from_a_flat_field() {
    let mut field = flat_field(12);
    // A single standing cell in the middle; everything else is pit.
    field.depth[6 * 12 + 6] = 0;
    field.relax(12, 12, false);
    field.relax(12, 12, true);
    for y in 0..12i32 {
        for x in 0..12i32 {
            let expected = (x - 6).abs().max((y - 6).abs()).fit::<u8>();
            assert_eq!(field.cell(x, y), expected, "cell {x},{y}");
        }
    }
}

#[test]
fn only_explored_standing_ground_seeds_a_rim() {
    assert_eq!(seed(Some(Terrain::Ground), true), None);
    assert_eq!(seed(Some(Terrain::Rock), true), None);
    assert_eq!(seed(Some(Terrain::Peak), true), None);
    assert_eq!(seed(Some(Terrain::Ground), false), Some(false));
    assert_eq!(seed(Some(Terrain::Pit), false), Some(false));
    assert_eq!(seed(Some(Terrain::Pit), true), Some(true));
    assert_eq!(
        seed(None, true),
        Some(false),
        "off-map is highwall, not floor"
    );
}

#[test]
fn unknown_ground_never_becomes_a_rim() {
    // Unexplored tiles enter the field exactly like pit: with no explored
    // standing cell anywhere, every cell stays void.
    let mut field = flat_field(12);
    field.relax(12, 12, false);
    field.relax(12, 12, true);
    assert!(field.depth.iter().all(|depth| *depth == u8::MAX));
}
