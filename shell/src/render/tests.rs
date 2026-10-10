use macroquad::prelude::vec2;

#[test]
fn articulated_mount_tracks_interpolated_positions_without_reading_hidden_targets() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 1,
        kind: oxide_sim::BuildingKind::Reclaimer,
        x: 9,
        y: 3,
    });
    let mut game = crate::game::Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let unit = game
        .state
        .units()
        .iter()
        .find(|unit| unit.player == game.presentation.human)
        .unwrap()
        .clone();
    let own = game.home_foundry().unwrap().id;
    let enemy = game
        .state
        .buildings()
        .iter()
        .find(|building| building.player != game.presentation.human)
        .unwrap();
    assert!(!enemy.tiles().any(|tile| game.my_vision().visible(tile)));
    let hidden = enemy.id;
    game.presentation
        .aim_unit_targets
        .insert(unit.id.0, oxide_sim::Target::Building(own));
    game.presentation.prev_pos.insert(
        unit.id.0,
        vec2(
            unit.pos.x.to_num::<f32>() + 1.0,
            unit.pos.y.to_num::<f32>() + 1.0,
        ),
    );
    let before = super::tracked_mount_angle(&game.view(), &unit, 0.0).unwrap();
    let after = super::tracked_mount_angle(&game.view(), &unit, 1.0).unwrap();
    assert!((before - after).abs() > 0.01);
    let hit = game.state.building(own).unwrap().closest_point_to(unit.pos);
    let direction = hit - unit.pos;
    let expected = direction
        .y
        .to_num::<f32>()
        .atan2(direction.x.to_num::<f32>())
        + std::f32::consts::FRAC_PI_2;
    assert!(
        (after - expected).abs() < 1e-5,
        "mount aims at the same near edge as combat"
    );
    game.presentation
        .aim_unit_targets
        .insert(unit.id.0, oxide_sim::Target::Building(hidden));
    assert_eq!(super::tracked_mount_angle(&game.view(), &unit, 1.0), None);
    game.presentation.aim_unit_targets.insert(
        unit.id.0,
        oxide_sim::Target::Unit(oxide_sim::UnitId(u32::MAX)),
    );
    assert_eq!(super::tracked_mount_angle(&game.view(), &unit, 1.0), None);
    game.presentation.aim_unit_targets.clear();
    let mut sapper = unit;
    sapper.kind = oxide_sim::UnitKind::Sapper;
    let known = game
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::BuildingKind::Reclaimer)
        .unwrap();
    let direction = known.closest_point_to(sapper.pos) - sapper.pos;
    let expected = direction
        .y
        .to_num::<f32>()
        .atan2(direction.x.to_num::<f32>())
        + std::f32::consts::FRAC_PI_2;
    sapper.order = oxide_sim::Order::Attack {
        pursue: false,
        target: oxide_sim::Target::Building(known.id).into(),
        resume: None,
    };
    assert!(
        (super::tracked_mount_angle(&game.view(), &sapper, 1.0).unwrap() - expected).abs() < 1e-5
    );
    sapper.order = oxide_sim::Order::Attack {
        pursue: false,
        target: oxide_sim::Target::Building(hidden).into(),
        resume: None,
    };
    assert_eq!(super::tracked_mount_angle(&game.view(), &sapper, 1.0), None);
}

#[test]
fn active_work_faces_its_physical_target() {
    use crate::presentation_animation::UnitWorkState;
    use chassis::grid::TilePos;

    let from = TilePos::new(4, 4).center();
    let east = TilePos::new(5, 4).center();
    let north = TilePos::new(4, 3).center();
    let east_angle = super::unit_work_facing(
        from,
        UnitWorkState::Harvesting {
            target: east,
            cycle: 0.0,
        },
    )
    .expect("different target has a heading");
    assert!((east_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert_eq!(
        super::unit_work_facing(
            from,
            UnitWorkState::Repairing {
                target: north,
                cycle: 0.0,
            }
        ),
        Some(0.0)
    );
    assert_eq!(
        super::unit_work_facing(
            from,
            UnitWorkState::Salvaging {
                target: from,
                cycle: 0.0,
            }
        ),
        None
    );
}

#[test]
fn harvester_selection_ring_clears_the_rendered_sprite() {
    let zoom = 32.0;
    let padding = 4.0;
    let radius = super::unit_selection_radius(oxide_sim::UnitKind::Harvester, zoom, padding);
    let visual_radius = super::unit_draw_scale(oxide_sim::UnitKind::Harvester) * zoom * 0.5;
    let collision_radius = oxide_sim::UnitKind::Harvester
        .stats()
        .radius
        .to_num::<f32>()
        * zoom;

    assert!((radius - (visual_radius + padding)).abs() < 1e-6);
    assert!(radius > collision_radius + padding);
}

#[test]
fn condor_uses_its_reviewed_two_tile_canvas_and_large_shadow() {
    let zoom = 64.0;
    assert_eq!(super::unit_draw_scale(oxide_sim::UnitKind::Condor), 2.0);
    let (shadow, offset, lift) = super::air_presentation(oxide_sim::UnitKind::Condor, zoom);
    assert_eq!(shadow, vec2(112.0, 76.0));
    assert_eq!(offset, vec2(8.0, 12.0));
    assert_eq!(lift, 4.0);
    assert!(
        super::unit_selection_radius(oxide_sim::UnitKind::Condor, zoom, 4.0)
            > super::unit_selection_radius(oxide_sim::UnitKind::Harvester, zoom, 4.0)
    );
}

#[test]
fn advanced_ground_units_use_two_tile_canvases() {
    let zoom = 64.0;
    for kind in [oxide_sim::UnitKind::Breaker, oxide_sim::UnitKind::Avalanche] {
        assert_eq!(super::unit_draw_scale(kind), 2.0);
        assert!(
            super::unit_selection_radius(kind, zoom, 4.0)
                > super::unit_selection_radius(oxide_sim::UnitKind::Sentinel, zoom, 4.0,)
        );
    }
}

#[test]
fn skyhook_uses_its_two_tile_transport_canvas_and_large_shadow() {
    let zoom = 64.0;
    assert_eq!(super::unit_draw_scale(oxide_sim::UnitKind::Skyhook), 2.0);
    let (shadow, offset, lift) = super::air_presentation(oxide_sim::UnitKind::Skyhook, zoom);
    assert_eq!(shadow, vec2(113.92, 97.28));
    assert_eq!(offset, vec2(8.32, 12.8));
    assert_eq!(lift, 4.48);
    assert!(
        super::unit_selection_radius(oxide_sim::UnitKind::Skyhook, zoom, 4.0)
            > super::unit_selection_radius(oxide_sim::UnitKind::Harvester, zoom, 4.0)
    );
}

#[test]
fn warden_is_larger_than_standard_armor_but_smaller_than_crucible_heavies() {
    let zoom = 64.0;
    assert_eq!(super::unit_draw_scale(oxide_sim::UnitKind::Warden), 1.4);
    assert!(
        super::unit_selection_radius(oxide_sim::UnitKind::Warden, zoom, 4.0)
            > super::unit_selection_radius(oxide_sim::UnitKind::Sentinel, zoom, 4.0)
    );
    assert!(
        super::unit_selection_radius(oxide_sim::UnitKind::Warden, zoom, 4.0)
            < super::unit_selection_radius(oxide_sim::UnitKind::Breaker, zoom, 4.0)
    );
}

#[test]
fn ui_scale_respects_both_small_window_axes() {
    assert_eq!(super::effective_ui_scale(1.5, vec2(640.0, 800.0)), 1.0);
    assert_eq!(super::effective_ui_scale(1.5, vec2(960.0, 400.0)), 1.0);
    assert_eq!(super::effective_ui_scale(1.5, vec2(960.0, 600.0)), 1.5);
    assert_eq!(super::effective_ui_scale(0.75, vec2(640.0, 400.0)), 0.75);
}

#[test]
fn ground_and_air_weapon_marks_have_distinct_warm_colors() {
    let ground = super::capability_icon_color(crate::panel::CapabilityIcon::Weapon);
    let air = super::capability_icon_color(crate::panel::CapabilityIcon::AirWeapon);

    assert!(ground.r > ground.b, "ground range stays warm");
    assert!(air.r > air.b, "air attack stays in the weapon color family");
    assert!(ground.g > ground.b, "ground attack stays orange-rust");
    assert!(air.b > air.g, "air attack stays crimson-rose");
    assert_ne!(ground, air);
}

#[test]
fn the_staleness_ramp_is_fresh_then_caps() {
    assert_eq!(super::staleness_fade(0.0), 0.0);
    assert!(super::staleness_fade(45.0) > 0.2);
    let capped = super::staleness_fade(600.0);
    assert!(
        (capped - 0.55).abs() < 1e-6,
        "old memories fade to a floor, never vanish"
    );
}
