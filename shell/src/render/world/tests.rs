use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn quarry_dressing_is_stable_varied_and_preserves_mirrored_placement() {
    let mut variants = std::collections::BTreeSet::new();
    let mut changed_seed = 0;
    let mut repeated_offset = 0;
    let mut count = 0;
    for y in 5..65 {
        for x in 5..90 {
            let pos = TilePos::new(x, y);
            let placement = quarry_dressing(pos, 100, 80, 719);
            assert_eq!(placement, quarry_dressing(pos, 100, 80, 719));
            changed_seed += usize::from(placement != quarry_dressing(pos, 100, 80, 720));
            let Some(a) = placement else { continue };
            let b = quarry_dressing(TilePos::new(99 - x, 79 - y), 100, 80, 719).unwrap();
            assert_eq!(a.variant, b.variant);
            assert_eq!(a.scale, b.scale);
            assert_eq!(a.offset, -b.offset);
            assert!(((a.rotation - b.rotation).abs() - std::f32::consts::PI).abs() < 1e-5);
            assert!(a.offset.abs().max_element() <= 0.125);
            assert!(a.scale >= 0.72 && a.scale <= 0.965 + f32::EPSILON);
            variants.insert(a.variant);
            repeated_offset +=
                usize::from(quarry_dressing(pos.offset(23, 0), 100, 80, 719) == Some(a));
            count += 1;
        }
    }
    assert_eq!(variants.len(), 12);
    assert!((150..400).contains(&count));
    assert!(changed_seed > count);
    assert_eq!(repeated_offset, 0);
}

const FOG_STATES: [Fog; 3] = [Fog::Visible, Fog::Explored, Fog::Unexplored];

fn fog_composite(neighborhood: &[[Fog; 3]; 3], u: f32, v: f32) -> f32 {
    let current = neighborhood[1][1].alpha();
    current + (1.0 - current) * fog_feather_alpha(neighborhood, u, v)
}

#[test]
fn fog_feather_fades_linearly_into_a_darker_side() {
    for (index, &light) in FOG_STATES.iter().enumerate() {
        for &dark in &FOG_STATES[index + 1..] {
            let mut neighborhood = [[light; 3]; 3];
            neighborhood[1][2] = dark;
            for step in 0..=20 {
                let u = step as f32 / 20.0;
                let fade = (1.0 - (1.0 - u) / FOG_FEATHER).max(0.0);
                let expected = light.alpha() + (dark.alpha() - light.alpha()) * fade;
                let occlusion = fog_composite(&neighborhood, u, 0.5);
                assert!(
                    (occlusion - expected).abs() < 1e-5,
                    "{light:?} {dark:?} {u}"
                );
            }
        }
    }
}

#[test]
fn fog_feather_meets_across_tile_edges_without_overdarkening() {
    let transpose = |n: [[Fog; 3]; 3]| -> [[Fog; 3]; 3] {
        std::array::from_fn(|row| std::array::from_fn(|col| n[col][row]))
    };
    for index in 0..3_usize.pow(9) {
        let left: [[Fog; 3]; 3] = std::array::from_fn(|row| {
            std::array::from_fn(|col| {
                FOG_STATES[index / 3_usize.pow(row.fit::<u32>() * 3 + col.fit::<u32>()) % 3]
            })
        });
        // The right tile's far column is a full tile from the shared edge.
        let right: [[Fog; 3]; 3] = std::array::from_fn(|row| {
            std::array::from_fn(|col| left[row].get(col + 1).copied().unwrap_or(Fog::Visible))
        });
        let (top, bottom) = (transpose(left), transpose(right));
        for step in 0..=20 {
            let t = step as f32 / 20.0;
            let seam = fog_composite(&left, 1.0, t) - fog_composite(&right, 0.0, t);
            assert!(seam.abs() < 1e-5, "{left:?} at {t}");
            let seam = fog_composite(&top, t, 1.0) - fog_composite(&bottom, t, 0.0);
            assert!(seam.abs() < 1e-5, "{left:?} at {t}");
        }
        let darkest = left.iter().flatten().max().copied().unwrap_or(Fog::Visible);
        for (u, v) in (0..=10).flat_map(|u| (0..=10).map(move |v| (u, v))) {
            let occlusion = fog_composite(&left, u as f32 / 10.0, v as f32 / 10.0);
            assert!(occlusion <= darkest.alpha() + 1e-5, "{left:?} at {u}, {v}");
        }
    }
}

const THEMES: [&str; 6] = [
    "rusted-yard",
    "cold-circuitry",
    "quarry-dust",
    "basalt",
    "slag",
    "verdigris",
];

#[test]
fn theme_prop_selection_is_a_180_degree_pair() {
    for theme in THEMES {
        for height in [31, 32] {
            for width in [47, 48] {
                for y in 0..height {
                    for x in 0..width {
                        let pos = TilePos::new(x, y);
                        let mirror = TilePos::new(width - 1 - x, height - 1 - y);
                        let a = symmetric_theme_prop(theme, pos, width, height);
                        let b = symmetric_theme_prop(theme, mirror, width, height);
                        assert_eq!(a.map(|p| p.variant), b.map(|p| p.variant));
                        if pos != mirror
                            && let (Some(a), Some(b)) = (a, b)
                        {
                            assert_eq!((a.quarter_turns + 2) % 4, b.quarter_turns);
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        symmetric_theme_prop("unknown", TilePos::new(4, 4), 9, 9),
        None
    );
    assert_eq!(
        symmetric_theme_prop("basalt", TilePos::new(4, 4), 9, 9),
        None
    );
}

#[test]
fn raised_theme_props_keep_world_space_lighting_upright() {
    for quarter_turns in 0..4 {
        for variant in 0..3 {
            let placement = ThemePropPlacement {
                variant,
                quarter_turns,
            };
            assert_eq!(
                placement.rotation(),
                f32::from(quarter_turns) * std::f32::consts::FRAC_PI_2
            );
        }
        for variant in 3..13 {
            let placement = ThemePropPlacement {
                variant,
                quarter_turns,
            };
            assert_eq!(placement.rotation(), 0.0);
        }
    }
}

#[test]
fn safe_prop_tiles_are_open_and_clear_of_semantic_map_marks() {
    let rows: Vec<String> = [
        "#################",
        "#1..............#",
        "#...............#",
        "#...............#",
        "#...............#",
        "#...............#",
        "#...............#",
        "#...........#...#",
        "#...............#",
        "#...............#",
        "#.......s.......#",
        "#...............#",
        "#...............#",
        "#........,......#",
        "#...............#",
        "#...............#",
        "#################",
    ]
    .map(str::to_string)
    .to_vec();
    assert!(safe_theme_prop_tile(&rows, TilePos::new(7, 7)));
    assert!(!safe_theme_prop_tile(&rows, TilePos::new(1, 1)));
    assert!(!safe_theme_prop_tile(&rows, TilePos::new(4, 4)));
    assert!(!safe_theme_prop_tile(&rows, TilePos::new(12, 7)));
    assert!(!safe_theme_prop_tile(&rows, TilePos::new(11, 7)));
    assert!(!safe_theme_prop_tile(&rows, TilePos::new(8, 10)));
    assert!(
        safe_theme_prop_tile(&rows, TilePos::new(7, 9)),
        "hidden salvage must not suppress a neighboring prop"
    );
    assert!(!safe_theme_prop_tile(&rows, TilePos::new(9, 13)));
}

#[test]
fn every_shipped_map_gets_symmetric_safe_theme_dressing() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let mut paths: Vec<_> = std::fs::read_dir(&root)
        .unwrap_or_else(|err| panic!("reading {}: {err}", root.display()))
        .map(|entry| entry.expect("scenario directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    let mut seen_themes = BTreeSet::new();
    let mut variants_by_theme = BTreeMap::<String, BTreeSet<usize>>::new();
    let mut all_variants = BTreeSet::new();
    let mut total_safe = 0;
    let mut total_props = 0;
    for path in paths {
        let scenario = oxide_sim::Scenario::load(&path)
            .unwrap_or_else(|err| panic!("loading {}: {err}", path.display()));
        let theme = scenario
            .meta
            .as_ref()
            .map_or("", |meta| meta.theme.as_str());
        assert!(
            theme_code(theme).is_some(),
            "{} has no generated theme-prop row",
            path.display()
        );
        seen_themes.insert(theme.to_string());
        let height = scenario.map.len().fit::<i32>();
        let width = scenario.map.first().expect("map row").len().fit::<i32>();
        let mut count = 0;
        let mut safe_count = 0;
        let mut variants = BTreeSet::new();
        for y in 0..height {
            for x in 0..width {
                let pos = TilePos::new(x, y);
                let mirror = TilePos::new(width - 1 - x, height - 1 - y);
                assert_eq!(
                    symmetric_safe_theme_prop_tile(&scenario.map, pos),
                    symmetric_safe_theme_prop_tile(&scenario.map, mirror),
                    "{} safe dressing mask is not symmetric at ({x}, {y})",
                    path.display()
                );
                if !symmetric_safe_theme_prop_tile(&scenario.map, pos) {
                    continue;
                }
                safe_count += 1;
                let Some(prop) = symmetric_theme_prop(theme, pos, width, height) else {
                    continue;
                };
                count += 1;
                variants.insert(prop.variant);
                assert_eq!(authored_tile(&scenario.map, pos), Some(b'.'));
                let partner =
                    symmetric_theme_prop(theme, mirror, width, height).expect("symmetric partner");
                assert_eq!(prop.variant, partner.variant);
                if pos != mirror {
                    assert_eq!((prop.quarter_turns + 2) % 4, partner.quarter_turns);
                }
            }
        }
        assert!(
            count * 20 >= safe_count,
            "{} dresses fewer than one in twenty safe tiles ({count}/{safe_count})",
            path.display()
        );
        assert!(
            count * 5 <= safe_count,
            "{} dresses more than one in five safe tiles ({count}/{safe_count})",
            path.display()
        );
        assert!(
            variants.len() >= 3,
            "{} only uses theme-prop variants {variants:?}",
            path.display()
        );
        if safe_count >= 400 {
            assert!(
                variants.len() >= 8,
                "{} has room for, but uses too little of the environment library: {variants:?}",
                path.display()
            );
        }
        variants_by_theme
            .entry(theme.to_string())
            .or_default()
            .extend(variants.iter().copied());
        all_variants.extend(variants);
        total_safe += safe_count;
        total_props += count;
    }
    assert!(
        total_props * 14 >= total_safe && total_props * 8 <= total_safe,
        "shipped maps should average roughly one prop per eleven safe tiles: \
             {total_props}/{total_safe}"
    );
    assert_eq!(
        seen_themes,
        THEMES.map(str::to_string).into_iter().collect()
    );
    for theme in THEMES {
        assert!(
            variants_by_theme
                .get(theme)
                .is_some_and(|variants| variants.len() >= 8),
            "{theme} exercises too little of the shared environment library: {:?}",
            variants_by_theme.get(theme)
        );
    }
    assert_eq!(
        all_variants,
        (0..13).collect(),
        "the shipped map set does not exercise the complete environment library"
    );
}

#[test]
fn obstacle_groups_never_cover_non_rock_or_overlap_neighbor_groups() {
    let rows = [
        ".........",
        ".#######.",
        ".#######.",
        ".#######.",
        ".#######.",
        ".........",
    ];
    let is_rock = |pos: TilePos| {
        pos.x >= 0
            && pos.y >= 0
            && rows
                .get(as_index(pos.y))
                .and_then(|row| row.as_bytes().get(as_index(pos.x)))
                == Some(&b'#')
    };
    let mut covered = BTreeSet::new();
    for y in (0..rows.len().fit::<i32>()).step_by(2) {
        for x in (0..rows[0].len().fit::<i32>()).step_by(3) {
            let Some(placement) = placement_for_group(TilePos::new(x, y), is_rock) else {
                continue;
            };
            for dy in 0..placement.footprint.1 {
                for dx in 0..placement.footprint.0 {
                    let pos = placement.anchor.offset(dx, dy);
                    assert!(is_rock(pos));
                    assert!(covered.insert((pos.x, pos.y)), "overlap at {pos:?}");
                }
            }
        }
    }
}

#[test]
fn incomplete_fog_knowledge_falls_back_to_individual_rocks() {
    for group_y in (0..20).step_by(2) {
        for group_x in (0..30).step_by(3) {
            let group = TilePos::new(group_x, group_y);
            let Some(_) = placement_for_group(group, |_| true) else {
                continue;
            };
            assert_eq!(
                placement_for_group(group, |pos| pos == group),
                None,
                "a multi-tile silhouette must not disclose its hidden continuation"
            );
            return;
        }
    }
    panic!("test grid did not find a decorated obstacle group");
}

#[test]
fn shipped_maps_exercise_every_approved_multi_tile_obstacle() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let mut paths: Vec<_> = std::fs::read_dir(&root)
        .expect("scenario directory")
        .map(|entry| entry.expect("scenario entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    let mut rocks = BTreeSet::new();
    let mut industrial = BTreeSet::new();
    for path in paths {
        let scenario = oxide_sim::Scenario::load(&path)
            .unwrap_or_else(|error| panic!("loading {}: {error}", path.display()));
        let height = scenario.map.len().fit::<i32>();
        let width = scenario
            .map
            .first()
            .expect("scenario row")
            .len()
            .fit::<i32>();
        let authored_rock =
            |pos: TilePos| authored_tile(&scenario.map, pos).is_some_and(|cell| cell == b'#');
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(3) {
                let Some(placement) = placement_for_group(TilePos::new(x, y), authored_rock) else {
                    continue;
                };
                match placement.art {
                    ObstacleArt::Rock(variant) => {
                        rocks.insert(variant);
                    }
                    ObstacleArt::Industrial(variant) => {
                        industrial.insert(variant);
                    }
                }
            }
        }
    }
    assert_eq!(
        rocks,
        (ONE_TILE_ROCK_COUNT..ONE_TILE_ROCK_COUNT + MULTI_ROCK_FOOTPRINTS.len()).collect()
    );
    assert_eq!(industrial, (0..GROUND_BLOCKER_FOOTPRINTS.len()).collect());
}
