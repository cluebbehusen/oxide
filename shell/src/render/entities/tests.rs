use super::*;
use crate::game::Game;

fn economy_support_game() -> Game {
    let mut scenario = oxide_sim::Scenario::skirmish();
    let frames: Vec<_> = scenario
        .map
        .iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.char_indices()
                .filter(|(_, tile)| *tile == 'E')
                .map(move |(x, _)| (x.fit::<i32>(), y.fit::<i32>()))
        })
        .collect();
    assert!(frames.len() >= 3, "fixture needs home and remote frames");
    let last = frames.len() - 1;
    scenario
        .buildings
        .extend(frames.into_iter().enumerate().map(|(index, (x, y))| {
            oxide_sim::scenario::BuildingSpec {
                player: u8::from(index == last),
                kind: oxide_sim::BuildingKind::Extractor,
                x,
                y,
            }
        }));
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("support fixture builds")
}

#[test]
fn range_rings_have_one_clear_subject() {
    let unit = oxide_sim::UnitId(3);
    let building = oxide_sim::BuildingId(5);
    assert_eq!(
        range_subject(&[unit], &[]),
        Some(oxide_sim::Target::Unit(unit))
    );
    assert_eq!(
        range_subject(&[], &[building]),
        Some(oxide_sim::Target::Building(building))
    );
    assert_eq!(range_subject(&[unit, oxide_sim::UnitId(4)], &[]), None);
    assert_eq!(range_subject(&[unit], &[building]), None);
}

#[test]
fn support_links_exist_only_for_connected_own_endpoints() {
    let mut game = economy_support_game();
    let supported = game
        .state
        .buildings()
        .iter()
        .find(|building| {
            building.player == game.presentation.human
                && building.kind == oxide_sim::BuildingKind::Extractor
                && game.state.extractor_income(building.id)
                    == Some(oxide_sim::ExtractorIncome::Supported)
        })
        .expect("supported own Extractor")
        .id;
    let remote = game
        .state
        .buildings()
        .iter()
        .find(|building| {
            building.player == game.presentation.human
                && building.kind == oxide_sim::BuildingKind::Extractor
                && game.state.extractor_income(building.id)
                    == Some(oxide_sim::ExtractorIncome::Remote)
        })
        .expect("remote own Extractor")
        .id;
    let foreign = game
        .state
        .buildings()
        .iter()
        .find(|building| {
            building.player != game.presentation.human
                && building.kind == oxide_sim::BuildingKind::Extractor
                && game.state.extractor_income(building.id)
                    == Some(oxide_sim::ExtractorIncome::Supported)
        })
        .expect("supported foreign Extractor")
        .id;

    game.presentation.selection.buildings = vec![supported];
    let links = selected_economy_support_links(&game.view());
    assert!(!links.is_empty());
    assert!(links.iter().all(|link| link.extractor == supported));

    let foundry = links[0].foundry;
    game.presentation.selection.buildings = vec![foundry];
    let from_foundry = selected_economy_support_links(&game.view());
    assert!(from_foundry.contains(&EconomySupportLink {
        extractor: supported,
        foundry,
    }));
    assert!(
        from_foundry.iter().all(|link| link.extractor != remote),
        "remote Extractors cannot gain a decorative connection"
    );

    game.presentation.selection.buildings = vec![remote];
    assert!(selected_economy_support_links(&game.view()).is_empty());

    game.presentation.selection.buildings = vec![foreign];
    assert!(
        selected_economy_support_links(&game.view()).is_empty(),
        "foreign support state must not be disclosed by a link"
    );
}

#[test]
fn production_progress_is_private_except_in_an_omniscient_view() {
    let mut game = Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0))
        .expect("embedded skirmish builds");
    let own = game
        .state
        .buildings()
        .iter()
        .find(|building| building.player == game.presentation.human)
        .expect("the human has a Foundry");
    let hostile = game
        .state
        .buildings()
        .iter()
        .find(|building| building.player != game.presentation.human)
        .expect("the opponent has a Foundry");
    assert!(production_progress_visible(&game.view(), own));
    assert!(!production_progress_visible(&game.view(), hostile));

    game.presentation.spectate = true;
    assert!(production_progress_visible(&game.view(), hostile));
}

#[test]
fn repair_bay_uses_the_exact_footprint_offset_aura() {
    let anchor = vec2(10.0, 20.0);
    let mut ranges = Vec::new();
    visit_building_ranges(anchor, oxide_sim::BuildingKind::RepairBay, 0, |range| {
        ranges.push(range);
    });

    let [range] = ranges.as_slice() else {
        panic!("Repair Bay should expose exactly one range: {ranges:?}");
    };
    assert_eq!(range.kind, BuildingRangeKind::Repair);
    let radius = oxide_sim::stats::REPAIR_BAY_RADIUS.to_num::<f32>();
    let size = oxide_sim::BuildingKind::RepairBay.size();
    let footprint_max = anchor + vec2(size.0 as f32, size.1 as f32);
    assert_eq!(
        range.shape,
        BuildingRangeShape::FootprintOffset {
            min: anchor,
            max: footprint_max,
            radius,
        }
    );
    assert_eq!(
        range.shape.outer_bounds(),
        (
            anchor - vec2(radius, radius),
            footprint_max + vec2(radius, radius),
        )
    );
}

#[test]
fn foundries_and_extractors_use_the_exact_square_support_footprint() {
    let anchor = vec2(10.0, 20.0);
    for kind in [
        oxide_sim::BuildingKind::Foundry,
        oxide_sim::BuildingKind::Extractor,
    ] {
        let mut ranges = Vec::new();
        visit_building_ranges(anchor, kind, 0, |range| ranges.push(range));
        let support = ranges
            .iter()
            .find(|range| range.kind == BuildingRangeKind::EconomySupport)
            .expect("economic endpoints expose their support footprint");
        let radius = footprint_distance_aura(oxide_sim::stats::EXTRACTOR_SUPPORT_RADIUS);
        let size = kind.size();
        let footprint_max = anchor + vec2(size.0 as f32, size.1 as f32);
        assert_eq!(
            support.shape,
            BuildingRangeShape::FootprintSquare {
                min: anchor,
                max: footprint_max,
                radius,
            }
        );
        assert_eq!(
            square_footprint_path(anchor, footprint_max, radius),
            [
                anchor - vec2(radius, radius),
                vec2(footprint_max.x + radius, anchor.y - radius),
                footprint_max + vec2(radius, radius),
                vec2(anchor.x - radius, footprint_max.y + radius),
                anchor - vec2(radius, radius),
            ],
            "Chebyshev support has square corners"
        );
    }
}

#[test]
fn weapon_ranges_remain_centered_circles() {
    let anchor = vec2(10.0, 20.0);
    let kind = oxide_sim::BuildingKind::Turret;
    let mut ranges = Vec::new();
    visit_building_ranges(anchor, kind, 0, |range| ranges.push(range));

    let weapon = ranges
        .iter()
        .find(|range| range.kind == BuildingRangeKind::Weapon)
        .expect("a Turret exposes its weapon range");
    let size = kind.size();
    assert_eq!(
        weapon.shape,
        BuildingRangeShape::Circle {
            center: anchor + vec2(size.0 as f32, size.1 as f32) * 0.5,
            radius: kind.base_stats().weapons[0].range.to_num::<f32>(),
        }
    );
    assert!(
        ranges
            .iter()
            .all(|range| range.kind != BuildingRangeKind::DeadZone),
        "zero-minimum-range weapons must not invent a warning circle"
    );
}

#[test]
fn bastion_dead_zone_is_a_shaded_inner_circle() {
    let anchor = vec2(10.0, 20.0);
    let kind = oxide_sim::BuildingKind::Bastion;
    let mut ranges = Vec::new();
    visit_building_ranges(anchor, kind, 0, |range| ranges.push(range));

    let dead_zone = ranges
        .iter()
        .find(|range| range.kind == BuildingRangeKind::DeadZone)
        .expect("a Bastion exposes its close-pressure counter");
    let size = kind.size();
    assert_eq!(
        dead_zone.shape,
        BuildingRangeShape::Circle {
            center: anchor + vec2(size.0 as f32, size.1 as f32) * 0.5,
            radius: kind.base_stats().weapons[0].minimum_range.to_num::<f32>(),
        }
    );
    assert_eq!(
        range_stroke(dead_zone.kind),
        RangeStroke::ShortDash,
        "the inner boundary cannot read as another solid weapon radius"
    );
    let line = Color::new(1.0, 0.68, 0.18, 0.78);
    let fill = dead_zone_fill(line);
    assert_eq!((fill.r, fill.g, fill.b), (line.r, line.g, line.b));
    assert!(
        fill.a > 0.0 && fill.a < 0.1,
        "the static wash must read without obscuring units: {fill:?}"
    );
}

#[test]
fn curved_ranges_have_distinct_textures_and_all_ranges_have_distinct_icons() {
    let kinds = [
        BuildingRangeKind::Weapon,
        BuildingRangeKind::DeadZone,
        BuildingRangeKind::Vision,
        BuildingRangeKind::Radar,
        BuildingRangeKind::Repair,
        BuildingRangeKind::EconomySupport,
    ];
    let strokes: Vec<_> = kinds
        .iter()
        .copied()
        .filter(|kind| *kind != BuildingRangeKind::EconomySupport)
        .map(range_stroke)
        .collect();
    for (index, stroke) in strokes.iter().enumerate() {
        assert!(
            strokes[..index].iter().all(|other| other != stroke),
            "{stroke:?} was reused for two range meanings"
        );
    }

    let icons = kinds.map(range_icon);
    for (index, icon) in icons.iter().enumerate() {
        assert!(
            icons[..index].iter().all(|other| other != icon),
            "{icon:?} was reused for two range meanings"
        );
    }
}

#[test]
fn anti_air_buildings_use_the_weapon_domain_at_every_tier_and_in_placement() {
    let kind = oxide_sim::BuildingKind::FlakTurret;
    for (tier, stats) in kind.tiers().iter().enumerate() {
        let mut ranges = Vec::new();
        visit_building_ranges(vec2(10.0, 10.0), kind, tier.fit::<u8>(), |range| {
            ranges.push(range);
        });
        let weapon = ranges
            .iter()
            .find(|range| range.kind == BuildingRangeKind::AirWeapon)
            .expect("Flak Turret and Burst Flak both expose anti-air reach");
        assert_eq!(
            range_icon(weapon.kind),
            crate::panel::CapabilityIcon::AirWeapon
        );
        assert_ne!(
            range_color(weapon.kind),
            range_color(BuildingRangeKind::Weapon)
        );
        assert_eq!(range_stroke(weapon.kind), RangeStroke::Solid);
        assert!(
            matches!(weapon.shape, BuildingRangeShape::Circle { radius, .. } if radius == stats.weapons[0].range.to_num::<f32>())
        );
    }
    let game = economy_support_game();
    let mut input = InputState::new();
    input.placing = Some(kind);
    let mut indicators = Vec::new();
    visit_active_ranges(&game.view(), &input, |indicator| indicators.push(indicator));
    assert!(
        indicators
            .iter()
            .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::AirWeapon)
    );
    assert!(
        !indicators
            .iter()
            .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::Weapon)
    );
}

#[test]
fn flakhound_and_sentinel_use_consistent_colors_and_marks_for_each_target_domain() {
    let scenario: oxide_sim::Scenario = serde_json::from_value(serde_json::json!({
        "name": "Weapon indicator fixture", "map": ["....................", "....................", "..1.................",
            "....................", "....................", "....................",
            "....................", "....................", "....................",
            "....................", "....................", "...................."],
        "players": [{"name": "You", "faction": "ferrous", "scrap": 0, "bot": false}],
        "units": [{"player": 0, "kind": "flakhound", "x": 5, "y": 7},
            {"player": 0, "kind": "sentinel", "x": 9, "y": 7}]
    }))
    .unwrap();
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    for kind in [
        oxide_sim::UnitKind::Flakhound,
        oxide_sim::UnitKind::Sentinel,
    ] {
        let id = game
            .state
            .units()
            .iter()
            .find(|unit| unit.kind == kind)
            .unwrap()
            .id;
        game.presentation.selection.units = vec![id];
        let mut indicators = Vec::new();
        visit_active_ranges(&game.view(), &InputState::new(), |indicator| {
            indicators.push(indicator);
        });
        let weapons: Vec<_> = indicators
            .iter()
            .filter(|indicator| {
                matches!(
                    indicator.range.kind,
                    BuildingRangeKind::Weapon | BuildingRangeKind::AirWeapon
                )
            })
            .collect();
        assert!(
            weapons
                .iter()
                .all(|indicator| indicator.color == range_color(indicator.range.kind))
        );
        assert!(
            weapons
                .iter()
                .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::AirWeapon)
        );
        if kind == oxide_sim::UnitKind::Sentinel {
            assert!(
                weapons
                    .iter()
                    .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::Weapon)
            );
            assert_eq!(weapons.len(), 2);
            assert_ne!(weapons[0].color, weapons[1].color);
        } else {
            assert_eq!(weapons.len(), 1);
        }
    }
}

#[test]
fn dots_keep_exact_spacing_across_path_vertices_and_ui_scales() {
    for scale in [0.75, 1.0, 2.0] {
        for vertices in [vec![0.0, 30.0], vec![0.0, 1.0, 7.0, 7.0, 19.0, 30.0]] {
            let path: Vec<_> = vertices.into_iter().map(|x| vec2(x * scale, 0.0)).collect();
            let mut spans: Vec<(f32, f32)> = Vec::new();
            visit_stroke_segments(&path, RangeStroke::Dotted, scale, |a, b| {
                if let Some(last) = spans.last_mut()
                    && (last.1 - a.x).abs() < 0.0001
                {
                    last.1 = b.x;
                } else {
                    spans.push((a.x, b.x));
                }
            });
            let expected = [(0.0, 2.5), (9.0, 11.5), (18.0, 20.5), (27.0, 29.5)];
            assert_eq!(spans.len(), expected.len());
            for ((a, b), (start, end)) in spans.into_iter().zip(expected) {
                assert!((a - start * scale).abs() < 0.0001);
                assert!((b - end * scale).abs() < 0.0001);
            }
        }
    }
}

#[test]
fn range_fades_preserve_the_exact_outer_edge_and_stay_inside_small_ranges() {
    for radius in [2.0, 32.0, 640.0] {
        let shape = BuildingRangeShape::Circle {
            center: vec2(10.0, 20.0),
            radius,
        };
        let mesh = range_fade_mesh(shape, 10.0, WHITE);
        let count = mesh.vertices.len() / 3;
        assert_eq!(count, range_path(shape, 0.0).len());
        for (index, vertex) in mesh.vertices.iter().enumerate() {
            let distance = (vertex.position.truncate() - vec2(10.0, 20.0)).length();
            assert!(distance <= radius + 0.001);
            assert!(distance >= radius * 0.5 - 0.001);
            if index < count {
                assert!((distance - radius).abs() < 0.001);
            }
            if index >= count * 2 {
                assert_eq!(vertex.color[3], 0);
            }
        }
        assert!(
            mesh.indices
                .iter()
                .all(|index| (*index as usize) < mesh.vertices.len())
        );
    }
}

#[test]
fn connection_stops_outside_both_footprints() {
    let a = (vec2(0.0, 0.0), vec2(20.0, 20.0));
    let b = (vec2(40.0, 0.0), vec2(60.0, 20.0));
    assert_eq!(
        footprint_link(a, b, 3.0),
        Some([vec2(23.0, 10.0), vec2(37.0, 10.0)])
    );
    let diagonal = (vec2(40.0, 40.0), vec2(60.0, 60.0));
    let [start, end] = footprint_link(a, diagonal, 3.0).unwrap();
    assert!((start - vec2(23.0, 23.0)).length() < 0.0001);
    assert!((end - vec2(37.0, 37.0)).length() < 0.0001);
    assert_eq!(footprint_link(a, a, 3.0), None);
    assert_eq!(
        footprint_link(a, (vec2(20.0, 0.0), vec2(40.0, 20.0)), 3.0),
        None
    );
}

#[test]
fn range_clipping_filters_distant_buildings_and_reuses_interval_storage() {
    let path = circle_path(Vec2::ZERO, 160.0);
    let mut buildings: Vec<_> = (0..1000)
        .map(|i| {
            let min = vec2(2000.0 + i as f32 * 40.0, 2000.0);
            (min, min + Vec2::splat(32.0))
        })
        .collect();
    for i in 0..40 {
        let angle = std::f32::consts::TAU * i as f32 / 40.0;
        let center = vec2(angle.cos(), angle.sin()) * 160.0;
        buildings.push((center - Vec2::splat(10.0), center + Vec2::splat(10.0)));
    }
    let mut clipper = RangeClipper::new(&path, &buildings);
    assert_eq!(clipper.occluders.len(), 40);
    let storage = clipper.intervals.as_ptr();
    let capacity = clipper.intervals.capacity();
    let mut clipped = 0;
    for pair in path.windows(2) {
        clipper.visit_visible(pair[0], pair[1], |_, _| {});
        clipped += usize::from(!clipper.intervals.is_empty());
        assert_eq!(clipper.intervals.as_ptr(), storage);
        assert_eq!(clipper.intervals.capacity(), capacity);
    }
    assert_eq!(clipped, 240);
}

#[test]
fn filtered_range_clipping_preserves_visible_spans_and_pattern_phase() {
    let buildings = [
        (vec2(-30.0, -10.0), vec2(0.0, 25.0)),
        (vec2(-20.0, -5.0), vec2(10.0, 35.0)),
        (vec2(-20.0, -5.0), vec2(10.0, 35.0)),
        (vec2(25.0, -40.0), vec2(40.0, 30.0)),
        (vec2(1000.0, 1000.0), vec2(1032.0, 1032.0)),
    ];
    for path in [
        circle_path(Vec2::ZERO, 30.0),
        rounded_footprint_path(Vec2::ZERO, vec2(20.0, 30.0), 15.0),
        square_footprint_path(Vec2::ZERO, vec2(20.0, 30.0), 15.0).to_vec(),
        vec![vec2(-50.0, 0.0), vec2(50.0, 0.0), vec2(-50.0, 0.0)],
    ] {
        for stroke in [
            RangeStroke::Solid,
            RangeStroke::ShortDash,
            RangeStroke::LongDash,
            RangeStroke::Dotted,
            RangeStroke::DashDot,
        ] {
            let mut clipper = RangeClipper::new(&path, &buildings);
            visit_stroke_segments(&path, stroke, 1.0, |a, b| {
                let mut intervals: Vec<_> = buildings
                    .iter()
                    .filter_map(|&(min, max)| line_footprint_interval(a, b, min, max))
                    .collect();
                intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut expected = Vec::new();
                let mut cursor: f32 = 0.0;
                for (start, end) in intervals.into_iter().chain(std::iter::once((1.0, 1.0))) {
                    if cursor < start {
                        expected.push((a.lerp(b, cursor), a.lerp(b, start)));
                    }
                    cursor = cursor.max(end);
                }
                let mut actual = Vec::new();
                clipper.visit_visible(a, b, |a, b| actual.push((a, b)));
                assert_eq!(actual, expected);
            });
        }
    }
}

#[test]
fn range_occluders_exclude_offscreen_and_unseen_buildings() {
    let mut game = Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(64.0, 64.0)).unwrap();
    let hostile = game
        .state
        .buildings()
        .iter()
        .find(|building| building.player != game.presentation.human)
        .unwrap();
    assert!(!hostile.tiles().any(|tile| game.my_vision().visible(tile)));
    let (width, height) = hostile.kind.size();
    game.presentation.camera.center = vec2(
        hostile.anchor.x as f32 + width as f32 * 0.5,
        hostile.anchor.y as f32 + height as f32 * 0.5,
    );
    assert!(range_occluders(&game.view()).is_empty());
    game.presentation.spectate = true;
    assert!(!range_occluders(&game.view()).is_empty());
    game.presentation.camera.center = vec2(-1000.0, -1000.0);
    assert!(range_occluders(&game.view()).is_empty());
}

#[test]
fn support_line_occlusion_handles_crossings_misses_and_reversed_edges() {
    let min = vec2(4.0, 4.0);
    let max = vec2(6.0, 6.0);
    assert_eq!(
        line_footprint_interval(vec2(0.0, 5.0), vec2(10.0, 5.0), min, max),
        Some((0.4, 0.6))
    );
    assert_eq!(
        line_footprint_interval(vec2(10.0, 5.0), vec2(0.0, 5.0), min, max),
        Some((0.4, 0.6))
    );
    assert_eq!(
        line_footprint_interval(Vec2::ZERO, vec2(10.0, 0.0), min, max),
        None
    );
    assert_eq!(
        line_footprint_interval(Vec2::ZERO, vec2(3.0, 3.0), min, max),
        None
    );
    assert_eq!(
        line_footprint_interval(vec2(5.0, 5.0), vec2(5.0, 10.0), min, max),
        Some((0.0, 0.2))
    );
}

#[test]
fn moth_payloads_begin_in_six_rack_positions_and_keep_their_impact_points() {
    let launch = vec2(5.0, 7.0);
    let heading = vec2(1.0, 0.0);
    let mut starts = Vec::new();
    for slot in 0..6 {
        let impact = vec2(7.0 + slot as f32 * 0.8, 7.1);
        let release = crate::game::LaunchPose {
            heading,
            kind: oxide_sim::UnitKind::Moth,
            slot,
        };
        let (start, direction) = moth_bomb_pose(launch, impact, release, 0.0, 10.0);
        assert!(direction.dot(heading) > 0.99);
        assert!(!starts.contains(&start));
        starts.push(start);
        for progress in [0.0, 0.01, 0.2, 0.5, 0.99, 1.0] {
            let (position, direction) = moth_bomb_pose(launch, impact, release, progress, 10.0);
            assert!(position.is_finite() && direction.is_finite());
        }
        assert!(
            moth_bomb_pose(launch, impact, release, 1.0, 10.0)
                .0
                .distance(impact)
                < 1e-5
        );
    }
}

#[test]
fn condor_payload_clears_the_nose_and_arrives_without_a_loft() {
    let launch = vec2(5.0, 7.0);
    let heading = vec2(1.0, 0.0);
    for impact in [launch, launch + vec2(0.1, 0.0), launch + vec2(3.0, 0.8)] {
        let (start, tangent) = condor_bomb_pose(launch, impact, heading, 0.0);
        assert!(start.is_finite() && tangent.is_finite());
        if impact != launch {
            assert!(start.x > launch.x);
            assert_eq!(start.y, launch.y);
            assert!(tangent.dot(heading) > 0.99);
        }
        let mut previous = start;
        for step in 1..=100 {
            let (position, direction) =
                condor_bomb_pose(launch, impact, heading, step as f32 / 100.0);
            assert!(position.is_finite() && direction.is_finite());
            assert!(position.x >= previous.x);
            assert!(position.y >= launch.y && position.y <= impact.y);
            previous = position;
        }
        assert!(previous.distance(impact) < 1e-5);
    }
}

#[test]
fn artillery_shells_begin_at_the_barrel_and_use_a_low_arc() {
    let launch = vec2(5.0, 7.0);
    let impact = vec2(15.0, 7.0);
    let shooter = oxide_sim::Target::Unit(oxide_sim::UnitId(4));
    assert_eq!(
        shell_visual_origin(launch, impact, shooter, oxide_sim::ProjectileKind::Bomb),
        launch
    );
    assert_eq!(
        shell_visual_origin(launch, impact, shooter, oxide_sim::ProjectileKind::Missile),
        launch + vec2(0.53, 0.0)
    );
    let from = shell_visual_origin(
        launch,
        impact,
        oxide_sim::Target::Building(oxide_sim::BuildingId(4)),
        oxide_sim::ProjectileKind::Shell,
    );
    assert!((from.x - 5.98).abs() < 1.0e-4);
    assert_eq!(from.y, launch.y);
    let bombard_from = shell_visual_origin(
        launch,
        impact,
        oxide_sim::Target::Unit(oxide_sim::UnitId(4)),
        oxide_sim::ProjectileKind::Shell,
    );
    assert!((bombard_from.x - 5.254_297).abs() < 1.0e-4);
    assert_eq!(bombard_from.y, launch.y);
    assert_eq!(
        shell_visual_origin(
            launch,
            launch,
            oxide_sim::Target::Unit(oxide_sim::UnitId(4)),
            oxide_sim::ProjectileKind::Shell,
        ),
        launch
    );

    let zoom = 32.0;
    let bombard = oxide_sim::Target::Unit(oxide_sim::UnitId(4));
    let bastion = oxide_sim::Target::Building(oxide_sim::BuildingId(4));
    assert!((shell_arc_lift(320.0, zoom, bombard) - 19.2).abs() < 1.0e-4);
    assert!((shell_arc_lift(320.0, zoom, bastion) - 12.8).abs() < 1.0e-4);
    assert_eq!(shell_arc_lift(1_000.0, zoom, bombard), zoom * 0.60);
    assert_eq!(shell_arc_lift(1_000.0, zoom, bastion), zoom * 0.40);

    let tail = shell_tail_start(0.5, 10.0);
    assert!((tail - 0.486).abs() < 1.0e-4);
    assert!((0.5 - tail) * 10.0 <= 0.140_001);
}

#[test]
fn bombard_payload_keeps_a_straight_constant_speed_course() {
    let launch = vec2(5.0, 7.0);
    for angle in [0.0_f32, 0.7, 1.57, 2.9, 4.71] {
        let heading = vec2(angle.cos(), angle.sin());
        let impact = launch + vec2((angle + 0.04).cos(), (angle + 0.04).sin()) * 10.0;
        let start = bombard_shell_position(launch, impact, heading, 0.0);
        let next = bombard_shell_position(launch, impact, heading, 0.001);
        assert!((start - launch).normalize().dot(heading) > 0.999);
        assert!((next - start).normalize().dot(heading) > 0.999);
        assert!(bombard_shell_position(launch, impact, heading, 1.0).distance(impact) < 1e-5);
        for step in 0..=100 {
            let t = step as f32 / 100.0;
            let position = bombard_shell_position(launch, impact, heading, t);
            assert!(position.is_finite());
            assert!(position.distance(start.lerp(impact, t)) < 1e-5);
        }
    }
}

#[test]
fn flakhound_reports_four_barrels_as_two_offset_pairs() {
    let delay = crate::game::FlakYokeDelay::OneTick;
    let first = flak_barrel_rounds(0.0, delay, 2);
    assert_eq!(first.iter().flatten().count(), 2);
    assert!(first[..2].iter().all(Option::is_some));
    let both = flak_barrel_rounds(delay.seconds(), delay, 2);
    let rounds: Vec<_> = both.into_iter().flatten().collect();
    assert_eq!(rounds.len(), 4);
    assert!(rounds.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert_eq!(rounds[0].1, rounds[1].1);
    assert_eq!(rounds[2].1, rounds[3].1);
    assert!(rounds[0].1 > rounds[2].1);
    assert_eq!(
        flak_barrel_rounds(0.0, delay, 1).iter().flatten().count(),
        1
    );
}

#[test]
fn flak_turret_rounds_match_both_barrel_banks_and_upgrade() {
    let delay = crate::game::FlakYokeDelay::OneAndHalfTicks;
    for count in [2, 3] {
        let first = flak_barrel_rounds(0.0, delay, count);
        assert_eq!(first.iter().flatten().count(), usize::from(count));
        assert!(first.iter().flatten().all(|round| round.0 < 0.0));
        let both: Vec<_> = flak_barrel_rounds(delay.seconds(), delay, count)
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(both.len(), usize::from(count) * 2);
        assert!(both.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for index in 0..usize::from(count) {
            assert_eq!(both[index].0, -both[both.len() - 1 - index].0);
            assert!(both[index].1 > both[index + usize::from(count)].1);
        }
        assert!(
            flak_barrel_rounds(delay.seconds() + FLAK_ROUND_TRAVEL + 0.01, delay, count)
                .iter()
                .all(Option::is_none)
        );
    }
}

#[test]
fn missiles_eject_then_accelerate_without_changing_arrival() {
    for distance in [0.0, 0.1, 1.0, 6.0, 20.0] {
        let total = (distance / 0.30_f32).ceil().max(1.0);
        assert_eq!(missile_travel_progress(0.0, total, distance), 0.0);
        assert!((missile_travel_progress(1.0, total, distance) - 1.0).abs() < 1.0e-5);
        let mut previous = 0.0;
        for step in 0..=200 {
            let progress = missile_travel_progress(step as f32 / 200.0, total, distance);
            assert!(progress.is_finite() && (0.0..=1.0).contains(&progress));
            assert!(progress >= previous);
            previous = progress;
        }
    }
    let total = 40.0;
    let at = |tick| missile_travel_progress(tick / total, total, 12.0) * 12.0;
    assert!((at(3.0) - 0.45).abs() < 1.0e-5);
    assert_eq!(missile_motor_strength(3.0 / total, total), 0.0);
    assert_eq!(missile_motor_strength(5.0 / total, total), 1.0);
    assert!(at(5.0) - at(4.0) > at(2.0) - at(1.0));
    assert!(at(7.0) - at(6.0) > at(5.0) - at(4.0));
    assert!((at(3.0001) - at(3.0)).abs() < 0.001);
    assert!((at(5.0001) - at(5.0)).abs() < 0.001);
}

#[test]
fn separate_flak_yokes_report_at_their_authored_delays() {
    use crate::game::FlakYokeDelay;

    let unit_delay = FlakYokeDelay::OneTick;
    assert_eq!(flak_round_progress(0.0, unit_delay), [Some(0.0), None]);

    let both = flak_round_progress(unit_delay.seconds(), unit_delay);
    assert!(both[0].is_some());
    assert_eq!(both[1], Some(0.0));

    let after_first = flak_round_progress(FLAK_ROUND_TRAVEL + 1.0e-4, unit_delay);
    assert!(after_first[0].is_none());
    assert!(after_first[1].is_some());

    let turret_delay = FlakYokeDelay::OneAndHalfTicks.seconds();
    assert_eq!(turret_delay, 1.5 * crate::game::TICK_DT);
    assert_eq!(
        flak_round_progress(0.0, FlakYokeDelay::None),
        [Some(0.0), Some(0.0)]
    );
}

#[test]
fn forge_spot_reaches_its_target_before_the_effect_expires() {
    assert_eq!(forge_spot_phases(0.0), (0.0, 0.0));
    assert_eq!(forge_spot_phases(FORGE_SPOT_TRAVEL_FRACTION), (1.0, 0.0));
    let (travel, impact) = forge_spot_phases(0.99);
    assert_eq!(travel, 1.0);
    assert!(impact > 0.9);
}

#[test]
fn kinetic_reports_reach_impact_before_fading_and_respect_fog() {
    use crate::game::ShotStyle;
    for heavy in [false, true] {
        let style = ShotStyle::Kinetic { heavy };
        assert_eq!(shot_impact_progress(style, 0.0), 0.0);
        assert_eq!(shot_impact_progress(style, style.life() * 0.25), 0.0);
        assert!(shot_impact_progress(style, style.life() * 0.9) > 0.5);
        assert_eq!(shot_impact_progress(style, style.life()), 1.0);
        assert_eq!(
            shot_visibility(style, false, true),
            ShotVisibility::ImpactOnly
        );
        assert_eq!(shot_visibility(style, true, false), ShotVisibility::Hidden);
    }
}

#[test]
fn shot_visibility_never_points_into_fog() {
    use crate::game::ShotStyle;

    assert_eq!(
        shot_visibility(ShotStyle::Rail, true, true),
        ShotVisibility::Full
    );
    assert_eq!(
        shot_visibility(ShotStyle::ForgeSpot, false, true),
        ShotVisibility::ImpactOnly
    );
    assert_eq!(
        shot_visibility(ShotStyle::Contact, true, true),
        ShotVisibility::ImpactOnly
    );
    assert_eq!(
        shot_visibility(ShotStyle::ForgeSpot, true, false),
        ShotVisibility::Hidden
    );
}

#[test]
fn sapper_visibility_separates_the_source_body_from_the_impact_bloom() {
    assert_eq!(
        sapper_effect_visibility(false, false, true),
        SapperEffectVisibility {
            body: false,
            bloom: true,
        }
    );
    assert_eq!(
        sapper_effect_visibility(false, true, false),
        SapperEffectVisibility {
            body: true,
            bloom: false,
        }
    );
    assert_eq!(
        sapper_effect_visibility(false, false, false),
        SapperEffectVisibility {
            body: false,
            bloom: false,
        }
    );
    assert_eq!(
        sapper_effect_visibility(true, false, false),
        SapperEffectVisibility {
            body: true,
            bloom: true,
        }
    );
}

#[test]
fn patterned_paths_close_on_their_authored_outer_bounds() {
    let center = vec2(30.0, 40.0);
    let circle = circle_path(center, 12.0);
    assert!((circle[0] - circle[circle.len() - 1]).length() < 1.0e-4);

    let min = vec2(10.0, 20.0);
    let max = vec2(24.0, 31.0);
    let radius = 7.0;
    let footprint = rounded_footprint_path(min, max, radius);
    assert!((footprint[0] - footprint[footprint.len() - 1]).length() < 1.0e-4);
    let low = footprint
        .iter()
        .fold(vec2(f32::INFINITY, f32::INFINITY), |bound, point| {
            bound.min(*point)
        });
    let high = footprint.iter().fold(
        vec2(f32::NEG_INFINITY, f32::NEG_INFINITY),
        |bound, point| bound.max(*point),
    );
    assert!((low - (min - vec2(radius, radius))).length() < 1.0e-4);
    assert!((high - (max + vec2(radius, radius))).length() < 1.0e-4);
}

#[test]
fn unguided_unit_contacts_never_turn_toward_a_nearby_body() {
    let from = vec2(2., 5.);
    let to = vec2(8., 5.);
    assert!(on_payload_course(from, to, vec2(7.7, 5.)));
    assert!(!on_payload_course(from, to, vec2(7.7, 5.2)));
    assert!(!on_payload_course(from, to, vec2(8.2, 5.)));
}
