use super::*;
use crate::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
use crate::{Faction, Scenario, UnitKind};

fn player(name: &str, faction: Faction, team: Option<u8>) -> PlayerSpec {
    PlayerSpec {
        name: name.into(),
        faction,
        team,
        scrap: 0,
        bot: false,
        bot_config: None,
    }
}

fn allied_incident_state() -> State {
    Scenario {
        mode: ScenarioMode::Match,
        name: "allied-incidents".into(),
        map: vec![
            "########################".into(),
            "#1.........2........3..#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: vec![
            player("West", Faction::Ferrous, Some(0)),
            player("Center", Faction::Cupric, Some(0)),
            player("East", Faction::Ferrous, Some(1)),
        ],
        units: Vec::new(),
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .unwrap()
}

fn screened_source(extra_hostile: bool) -> (State, TilePos) {
    let source = TilePos::new(10, 5);
    let mut units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 9,
            y: 5,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 8,
            y: 5,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 12,
            y: 5,
        },
    ];
    if extra_hostile {
        units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 12,
            y: 6,
        });
    }
    let state = Scenario {
        mode: ScenarioMode::Match,
        name: "screened-salvage".into(),
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#................2.#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .unwrap();
    (state, source)
}

#[test]
fn equal_local_ground_value_screens_a_work_zone() {
    let (state, source) = screened_source(false);
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    assert!(!danger.contains(source));
}

#[test]
fn outmatched_local_ground_value_retires_a_work_zone() {
    let (state, source) = screened_source(true);
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    assert!(danger.contains(source));
}

#[test]
fn cached_danger_queries_match_the_snapshot_predicate() {
    let (state, _) = screened_source(true);
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    for y in -1..=state.map.height() {
        for x in -1..=state.map.width() {
            let tile = TilePos::new(x, y);
            assert_eq!(
                danger.contains(tile),
                danger.compute_contains(tile),
                "{tile}"
            );
            assert_eq!(
                danger.contains(tile),
                danger.compute_contains(tile),
                "cached {tile}"
            );
        }
    }
}

#[test]
fn exhausted_route_cache_preserves_a_goal_only_danger_exception() {
    let (state, _) = screened_source(false);
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    let start = TilePos::new(2, 3);
    let unreachable = TilePos::new(15, 3);
    let exceptional_goal = TilePos::new(10, 3);

    assert!(
        danger
            .find_route(start, unreachable, |tile| tile.x != 10)
            .is_none()
    );
    assert_eq!(
        danger.last_route_reachability(&[exceptional_goal, unreachable], false),
        Some(vec![false, false])
    );
    assert_eq!(
        danger.last_route_reachability(&[exceptional_goal, unreachable], true),
        Some(vec![true, false]),
        "a goal-specific exception is reachable through its explored cardinal neighbor"
    );
    assert!(
        danger
            .find_route(start, exceptional_goal, |tile| {
                tile == exceptional_goal || tile.x != 10
            })
            .is_some(),
        "the cached exception agrees with a real goal-specific A*"
    );
}

#[test]
fn allied_impact_memory_is_shared_bounded_and_cools_down() {
    let mut state = allied_incident_state();
    let source = TilePos::new(10, 4);
    state.record_salvage_incident(PlayerId(1), source);

    let west = state.vision(PlayerId(0)).salvage_incidents();
    let center = state.vision(PlayerId(1)).salvage_incidents();
    assert_eq!(west, center, "teammates receive one shared memory");
    assert_eq!(west.len(), 1);
    assert_eq!(west[0].tile, source);
    assert_eq!(
        west[0].expires_at,
        state.current_tick() + crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1
    );
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    assert!(
        danger.contains(source),
        "the incident source stays ineligible"
    );
    assert!(
        danger.route_safe_from(source, source.offset(-1, 0)),
        "a worker inside the ring may step outward"
    );
    assert!(
        danger.route_safe_from(source.offset(-2, 0), source.offset(-3, 0)),
        "an outward route may keep leaving the ring"
    );
    assert!(
        !danger.route_safe_from(source.offset(-2, 0), source.offset(-1, 0)),
        "an escape cannot turn back toward the impact"
    );
    assert!(
        !danger.route_safe_from(source.offset(-5, 0), source.offset(-4, 0)),
        "a route originating outside cannot enter the incident ring"
    );
    assert!(
        state.vision(PlayerId(2)).salvage_incidents().is_empty(),
        "the hostile team learns nothing from its victim's memory"
    );

    state.tick = west[0].expires_at;
    assert!(
        !GroundSalvageDanger::capture(&state, PlayerId(0)).contains(source),
        "the incident stops affecting routes exactly at expiry"
    );
    state.refresh_vision();
    assert!(
        state.vision(PlayerId(0)).salvage_incidents().is_empty(),
        "refresh prunes expired state instead of accumulating history"
    );
    assert_eq!(state.vision(PlayerId(0)), state.vision(PlayerId(1)));
}

#[test]
fn incident_memory_coalesces_and_evicts_deterministically() {
    let mut state = allied_incident_state();
    let repeated = TilePos::new(8, 3);
    state.record_salvage_incident(PlayerId(0), repeated);
    let first_expiry = state.vision(PlayerId(0)).salvage_incidents()[0].expires_at;
    state.tick += 7;
    state.record_salvage_incident(PlayerId(1), repeated);
    let incidents = state.vision(PlayerId(0)).salvage_incidents();
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].expires_at, first_expiry + 7);

    let mut state = allied_incident_state();
    for x in 0..=crate::stats::HARVEST_INCIDENT_CAP {
        state.record_salvage_incident(PlayerId(0), TilePos::new(i32::try_from(x).unwrap(), 3));
    }
    let incidents = state.vision(PlayerId(0)).salvage_incidents();
    assert_eq!(incidents.len(), crate::stats::HARVEST_INCIDENT_CAP);
    assert_eq!(
        incidents[0].tile,
        TilePos::new(1, 3),
        "equal-expiry overflow evicts the row-major first site"
    );
    assert!(
        incidents
            .windows(2)
            .all(|pair| { (pair[0].tile.y, pair[0].tile.x) < (pair[1].tile.y, pair[1].tile.x) })
    );
}

#[test]
fn indexed_building_knowledge_matches_the_fog_reference() {
    fn reference(state: &State, viewer: PlayerId, tile: TilePos) -> bool {
        let vision = state.vision(viewer);
        if vision.visible(tile) {
            return state
                .buildings_at(tile)
                .any(|building| !building.kind.is_stealthy() && !building.provisional());
        }
        let team = state.player(viewer).team;
        state.buildings.iter().any(|building| {
            !building.kind.is_stealthy()
                && !building.provisional()
                && state.player(building.player).team == team
                && building.contains(tile)
        }) || vision
            .ghosts()
            .iter()
            .any(|ghost| !ghost.kind.is_stealthy() && ghost.footprint().any(|t| t == tile))
    }

    let (mut state, _) = screened_source(false);
    let visible_site = TilePos::new(10, 4);
    assert!(state.vision(PlayerId(0)).visible(visible_site));
    state.place_building(PlayerId(1), BuildingKind::Turret, visible_site);

    let check = |state: &State| {
        let knowledge = GroundSalvageDanger::capture(state, PlayerId(0));
        for y in 0..state.map.height() {
            for x in 0..state.map.width() {
                let tile = TilePos::new(x, y);
                assert_eq!(
                    knowledge.known_building_blocked(tile),
                    reference(state, PlayerId(0), tile),
                    "{tile}"
                );
            }
        }
    };
    check(&state);

    state.refresh_vision();
    for unit in state
        .units
        .iter_mut()
        .filter(|unit| unit.player == PlayerId(0))
    {
        unit.pos = TilePos::new(2, 2).center();
    }
    state.refresh_vision();
    assert!(!state.vision(PlayerId(0)).visible(visible_site));
    assert!(
        state
            .vision(PlayerId(0))
            .ghosts()
            .iter()
            .any(|ghost| ghost.anchor == visible_site)
    );
    check(&state);
}

#[test]
fn known_building_projection_only_blocks_ground_claiming_footprints() {
    let mut state = allied_incident_state();
    let own_charge = TilePos::new(3, 4);
    let allied_charge = TilePos::new(6, 4);
    let allied_wall = TilePos::new(9, 4);
    let hostile_wall = TilePos::new(13, 4);
    let hostile_charge = TilePos::new(16, 4);
    state.place_building(PlayerId(0), BuildingKind::ScuttleCharge, own_charge);
    state.place_building(PlayerId(1), BuildingKind::ScuttleCharge, allied_charge);
    state.place_building(PlayerId(1), BuildingKind::Barricade, allied_wall);
    state.vision[0].ghosts.extend([
        GhostBuilding {
            kind: BuildingKind::Barricade,
            owner: PlayerId(2),
            anchor: hostile_wall,
            hp: BuildingKind::Barricade.base_stats().max_hp,
            built: true,
        },
        GhostBuilding {
            kind: BuildingKind::ScuttleCharge,
            owner: PlayerId(2),
            anchor: hostile_charge,
            hp: BuildingKind::ScuttleCharge.base_stats().max_hp,
            built: true,
        },
    ]);

    assert!(state.passable(own_charge));
    assert!(state.passable(allied_charge));
    assert!(!state.passable(allied_wall));
    let projection = GroundSalvageDanger::capture(&state, PlayerId(0));
    assert!(!projection.known_building_blocked(own_charge));
    assert!(!projection.known_building_blocked(allied_charge));
    assert!(projection.known_building_blocked(allied_wall));
    assert!(projection.known_building_blocked(hostile_wall));
    assert!(!projection.known_building_blocked(hostile_charge));
}

fn static_egress_danger() -> GroundSalvageDanger {
    let (state, _) = screened_source(false);
    let mut danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    danger.mobile.clear();
    danger.statics = vec![StaticGroundPressure {
        anchor: TilePos::new(10, 3),
        size: (2, 2),
        reach_sq: Fx::from_num(64),
    }];
    danger
}

#[test]
fn static_pressure_allows_egress_but_never_approach_or_new_entry() {
    let danger = static_egress_danger();
    let from = TilePos::new(7, 4);
    assert!(
        danger.contains(from),
        "the unsafe source remains ineligible for work"
    );
    assert!(danger.route_safe_from(from, TilePos::new(6, 4)));
    assert!(danger.route_safe_from(from, TilePos::new(7, 3)));
    assert!(!danger.route_safe_from(from, TilePos::new(8, 4)));
    assert!(!danger.route_safe_from(TilePos::new(0, 4), TilePos::new(3, 4)));
    assert!(danger.route_safe_from(from, TilePos::new(0, 4)));
}

#[test]
fn static_egress_cannot_approach_an_overlapping_gun_or_cross_radar_or_mobile_fire() {
    let from = TilePos::new(7, 4);
    let to = TilePos::new(6, 4);
    let mut overlap = static_egress_danger();
    overlap.statics.push(StaticGroundPressure {
        anchor: TilePos::new(0, 3),
        size: (2, 2),
        reach_sq: Fx::from_num(64),
    });
    assert!(!overlap.route_safe_from(from, to));
    let mut radar = static_egress_danger();
    radar.contacts.push(to);
    assert!(!radar.route_safe_from(from, to));
    let mut mobile = static_egress_danger();
    mobile.mobile.push(MobileGroundPressure {
        pos: to.center(),
        reach_sq: Fx::from_num(16),
        strength: 100,
        hostile: true,
    });
    assert!(!mobile.route_safe_from(from, to));
}

/// A walk over every threat record, kept as the reference the coarse
/// cells must reproduce: `(mobile or radar, observed)`.
fn linear_threats(danger: &GroundSalvageDanger, source: TilePos) -> (bool, bool) {
    let point = source.center();
    let radar = danger
        .contacts
        .iter()
        .any(|contact| contact.chebyshev(source) <= crate::stats::HARVEST_RADAR_DANGER_RADIUS);
    let (mut hostile, mut screen) = (0u64, 0u64);
    for pressure in &danger.mobile {
        if pressure.pos.dist_sq(point) <= pressure.reach_sq {
            if pressure.hostile {
                hostile = hostile.saturating_add(pressure.strength);
            } else {
                screen = screen.saturating_add(pressure.strength);
            }
        }
    }
    let mobile = radar || hostile > screen;
    let statics = danger.statics.iter().any(|pressure| {
        rect_closest_point(pressure.anchor, pressure.size, point).dist_sq(point)
            <= pressure.reach_sq
    });
    (mobile, mobile || statics)
}

#[test]
fn threat_cells_agree_with_a_walk_over_every_record() {
    let mut state = Scenario::skirmish().build().expect("skirmish builds");
    for _ in 0..90 {
        state.tick(&[]);
    }
    let mut danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    assert!(danger.incidents.is_empty());
    let (width, height) = (state.map.width(), state.map.height());
    let mut rng = chassis::rng::Pcg32::new(17, 3);
    let mut coordinate =
        |span: i32| i32::try_from(rng.next_u32() % u32::try_from(span + 8).unwrap()).unwrap() - 4;
    for _ in 0..160 {
        let (x, y) = (coordinate(width), coordinate(height));
        let reach = Fx::from_num(x.rem_euclid(9) + 1) + Fx::lit("0.3");
        danger.mobile.push(MobileGroundPressure {
            pos: TilePos::new(x, y).center() + Vec2Fx::new(Fx::lit("0.2"), -Fx::lit("0.4")),
            reach_sq: reach * reach,
            strength: (y.rem_euclid(7) as u64 + 1) * 40,
            hostile: x % 2 == 0,
        });
    }
    for _ in 0..40 {
        let (x, y) = (coordinate(width), coordinate(height));
        let reach = Fx::from_num(y.rem_euclid(3) + 1);
        danger.statics.push(StaticGroundPressure {
            anchor: TilePos::new(x, y),
            size: (x.rem_euclid(3) + 1, y.rem_euclid(2) + 1),
            reach_sq: reach * reach,
        });
    }
    for _ in 0..6 {
        let (x, y) = (coordinate(width), coordinate(height));
        danger.contacts.push(TilePos::new(x, y));
    }

    let froms = [
        TilePos::new(0, 0),
        TilePos::new(width / 2, height / 2),
        TilePos::new(width - 1, 3),
    ];
    let (mut dangerous, mut safe) = (0, 0);
    for y in -2..=height + 1 {
        for x in -2..=width + 1 {
            let tile = TilePos::new(x, y);
            let (mobile, observed) = linear_threats(&danger, tile);
            assert_eq!(danger.mobile_or_radar_contains(tile), mobile, "{tile:?}");
            assert_eq!(danger.compute_observed_contains(tile), observed, "{tile:?}");
            for from in froms {
                let approaches = danger.statics.iter().any(|pressure| {
                    let next = rect_closest_point(pressure.anchor, pressure.size, tile.center())
                        .dist_sq(tile.center());
                    next <= pressure.reach_sq
                        && next
                            < rect_closest_point(pressure.anchor, pressure.size, from.center())
                                .dist_sq(from.center())
                });
                assert_eq!(
                    danger.route_safe_from(from, tile),
                    !(observed && (mobile || approaches)),
                    "{from:?} -> {tile:?}"
                );
            }
            dangerous += usize::from(observed);
            safe += usize::from(!observed);
        }
    }
    assert!(
        dangerous > 0 && safe > 0,
        "{dangerous} dangerous, {safe} safe"
    );
}
