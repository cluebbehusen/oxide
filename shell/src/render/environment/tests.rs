use super::*;
use chassis::grid::as_index;

#[test]
fn solid_authored_walls_are_absorbed_but_open_lanes_stay_playable() {
    let closed = ["#####", "#...#", "#...#", "#####"].map(str::to_string);
    assert_eq!(
        BoundaryInsets::from_rows(&closed),
        BoundaryInsets {
            top: true,
            right: true,
            bottom: true,
            left: true,
        }
    );
    let open = ["##.##", "#...#", "....#", "##.##"].map(str::to_string);
    assert_eq!(
        BoundaryInsets::from_rows(&open),
        BoundaryInsets {
            top: false,
            right: true,
            bottom: false,
            left: false,
        }
    );
}

#[test]
fn terrace_material_never_enters_the_battlefield_rect() {
    let field = TerraceField::new(MapFrame {
        rect: Rect::new(32.0, 48.0, 640.0, 384.0),
        tile: 32.0,
    });
    for iy in 0..field.inner_height {
        for ix in 0..field.inner_width {
            assert_eq!(field.material(ix, iy), Material::Void);
        }
    }
    for layer in LAYERS {
        assert!(layer.bench_cells >= 3);
    }
}

#[test]
fn floor_wave_carries_across_the_terrace_benches() {
    let phases: Vec<_> = (0..6)
        .map(|tile_x| WAVE_LIFTS[as_index(tile_x) % 6])
        .collect();
    assert_eq!(phases, WAVE_LIFTS);
    assert!(phases.iter().any(|lift| *lift < 0));
    assert!(phases.iter().any(|lift| *lift > 0));
}
