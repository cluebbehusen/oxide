use super::*;
use chassis::fx::Fx;

#[test]
fn parked_bodies_mark_exactly_the_resting_friendly_ground_tiles() {
    let mut state = crate::Scenario::skirmish()
        .build()
        .expect("skirmish builds");
    for _ in 0..90 {
        state.tick(&[]);
    }
    let (width, height) = (state.map.width(), state.map.height());
    state.units[0].pos = TilePos::new(-1, 2).center();
    state.units[0].path = None;
    state.units[0].drive_speed = Fx::ZERO;
    let mut marked = 0;
    for player in 0..state.players.len() {
        let viewer = PlayerId::from_index(player);
        let parked = state.parked_bodies(viewer);
        for y in -1..=height {
            for x in -1..=width {
                let tile = TilePos::new(x, y);
                let resting = state.map.grid().get(tile).is_some()
                    && state.units.iter().any(|u| {
                        u.hp > 0
                            && u.domain() == crate::stats::Domain::Ground
                            && u.drive_speed == Fx::ZERO
                            && u.path.is_none()
                            && !state.hostile(viewer, u.player)
                            && u.tile() == tile
                    });
                assert_eq!(parked.blocks(tile), resting, "{viewer:?} {tile:?}");
                marked += usize::from(resting);
            }
        }
    }
    assert!(marked > 0, "no body was resting");
}

fn tiny_state() -> State {
    let (map, _) = Map::parse(&["....", "....", "....", "...."]).unwrap();
    State::assemble(
        map,
        vec![Player {
            name: "p".into(),
            faction: Faction::Ferrous,
            team: 0,
            scrap: 0,
            recovery_allowance: 0,
            recovery_target: 0,
            recovery_ready: true,
            resigned: false,
            eliminated_at: None,
        }],
        7,
    )
}

#[test]
fn unit_lookup_by_id_uses_sorted_order() {
    let mut state = tiny_state();
    let a = state.spawn_unit(
        PlayerId(0),
        UnitKind::Harvester,
        TilePos::new(0, 0).center(),
    );
    let b = state.spawn_unit(PlayerId(0), UnitKind::Sentinel, TilePos::new(1, 1).center());
    assert_eq!(state.unit(a).unwrap().kind, UnitKind::Harvester);
    assert_eq!(state.unit(b).unwrap().kind, UnitKind::Sentinel);
    assert_eq!(state.unit(UnitId(99)), None);
}

#[test]
fn mirrored_spawns_face_mirrored_headings() {
    let mut state = tiny_state();
    let (width, height) = (state.map().width(), state.map().height());
    for kind in UnitKind::ALL {
        for (x, y) in [(0, 0), (0, 1), (1, 3), (2, 0)] {
            let tile = TilePos::new(x, y);
            let turned = TilePos::new(width - 1 - x, height - 1 - y);
            let a = state.spawn_unit(PlayerId(0), kind, tile.center());
            let b = state.spawn_unit(PlayerId(0), kind, turned.center());
            let heading = |id| state.unit(id).unwrap().heading;
            assert_eq!(
                heading(b),
                heading(a).wrapping_add(128),
                "{kind:?} at {tile:?}"
            );
        }
    }
}

#[test]
fn building_blocks_passability() {
    let mut state = tiny_state();
    state.place_building(PlayerId(0), BuildingKind::Foundry, TilePos::new(1, 1));
    assert!(state.passable(TilePos::new(0, 0)));
    for pos in [(1, 1), (2, 1), (1, 2), (2, 2)] {
        assert!(!state.passable(TilePos::new(pos.0, pos.1)));
    }
    assert!(state.passable(TilePos::new(3, 3)));
}

#[test]
fn building_geometry() {
    let mut state = tiny_state();
    let id = state.place_building(PlayerId(0), BuildingKind::Foundry, TilePos::new(1, 1));
    let b = state.building(id).unwrap();
    assert_eq!(b.center(), Vec2Fx::new(Fx::from_num(2), Fx::from_num(2)));
    // A point due west clamps to the footprint's west face.
    let probe = Vec2Fx::new(Fx::ZERO, Fx::from_num(2));
    assert_eq!(
        b.closest_point_to(probe),
        Vec2Fx::new(Fx::from_num(1), Fx::from_num(2))
    );
    assert_eq!(b.tiles().count(), 4);
}

#[test]
fn the_progress_ceiling_keeps_the_construction_ramp_in_u32() {
    // Unit welds ramp over the full max_hp — same product, same
    // ceiling, same obligation for every machine on the roster.
    const UNIT_KINDS: [UnitKind; 11] = [
        UnitKind::Harvester,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Bombard,
        UnitKind::Flakhound,
        UnitKind::Stinger,
        UnitKind::Buzzard,
        UnitKind::Darter,
        UnitKind::Talon,
        UnitKind::Wisp,
    ];
    // Construction, repair, and salvage all price one tick of work as
    // `ramp * (meter + 1) / ramp_ticks` in u32. The ceiling is only
    // worth anything if that product still fits at the ceiling.
    const KINDS: [BuildingKind; 7] = [
        BuildingKind::Foundry,
        BuildingKind::Turret,
        BuildingKind::Fabricator,
        BuildingKind::FlakTurret,
        BuildingKind::Bastion,
        BuildingKind::Array,
        BuildingKind::Reclaimer,
    ];
    for kind in KINDS {
        let stats = kind.base_stats();
        let ramp = u64::from(stats.max_hp - stats.max_hp / 5);
        assert!(
            u32::try_from(ramp * (u64::from(PROGRESS_ENVELOPE) + 1)).is_ok(),
            "{}: the ramp math must stay inside u32 at the ceiling",
            kind.name()
        );
    }
    for kind in UNIT_KINDS {
        let ramp = u64::from(kind.stats().max_hp);
        assert!(
            u32::try_from(ramp * (u64::from(PROGRESS_ENVELOPE) + 1)).is_ok(),
            "{}: the unit weld ramp must stay inside u32 at the ceiling",
            kind.name()
        );
    }
}

#[test]
fn the_coordinate_envelope_admits_every_legal_map_and_refuses_the_extremes() {
    assert!(tile_inside_envelope(TilePos::new(
        i32::from(MAX_MAP_EDGE),
        i32::from(MAX_MAP_EDGE)
    )));
    assert!(tile_inside_envelope(TilePos::new(-1, -1)));
    assert!(!tile_inside_envelope(TilePos::new(i32::MAX, 0)));
    assert!(!tile_inside_envelope(TilePos::new(0, i32::MIN)));
    assert!(point_inside_envelope(
        TilePos::new(i32::from(MAX_MAP_EDGE), 0).center()
    ));
    assert!(!point_inside_envelope(Vec2Fx::new(
        Fx::from_bits(i64::MAX),
        Fx::ZERO
    )));
}

#[test]
fn state_hash_changes_with_content() {
    let mut a = tiny_state();
    let b = tiny_state();
    assert_eq!(a.hash(), b.hash());
    a.spawn_unit(PlayerId(0), UnitKind::Harvester, Vec2Fx::ZERO);
    assert_ne!(a.hash(), b.hash());
}
