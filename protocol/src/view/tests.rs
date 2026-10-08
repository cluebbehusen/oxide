use super::*;
use oxide_sim::scenario::ScenarioMode;

#[test]
fn capture_respects_filters_and_overlays_entities() {
    let state = oxide_sim::Scenario::skirmish().build().unwrap();
    let full = StateView::capture(
        &state,
        StateFilter {
            map: true,
            ..StateFilter::default()
        },
    );
    assert_eq!(full.players.len(), 2);
    assert_eq!(full.units.len(), 8);
    assert_eq!(full.buildings.len(), 2);
    let map = full.map.as_ref().unwrap();
    let flat: String = map.concat();
    assert!(
        flat.contains('A') && flat.contains('B'),
        "both foundries drawn"
    );
    assert!(
        flat.contains('a') && flat.contains('b'),
        "both armies drawn"
    );

    let slim = StateView::capture(&state, StateFilter::default());
    assert!(slim.map.is_none());
    assert!(!slim.hash.is_empty() && slim.hash.starts_with("0x"));
    assert_eq!(slim.hash, full.hash, "views never perturb state");

    let identity_only = StateView::capture(
        &state,
        StateFilter {
            players: false,
            units: false,
            buildings: false,
            map: false,
        },
    );
    assert!(identity_only.players.is_empty());
    assert!(identity_only.units.is_empty());
    assert!(identity_only.buildings.is_empty());
    assert!(identity_only.map.is_none());
    assert_eq!(identity_only.tick, full.tick);
    assert_eq!(identity_only.hash, full.hash);
}

#[test]
fn the_fog_view_reports_only_what_the_seat_has_seen() {
    let state = oxide_sim::Scenario::skirmish().build().unwrap();
    let fog = FogView::capture(&state, PlayerId(0));
    let omniscient = StateView::capture(&state, StateFilter::default());

    // At tick zero the enemy base is dark: the honest view carries
    // strictly less than the omniscient one.
    assert!(fog.units.len() < omniscient.units.len());
    assert!(fog.buildings.len() < omniscient.buildings.len());
    assert!(fog.ghosts.is_empty(), "nothing hostile has been seen yet");

    let mask_at = |tile: [i32; 2]| {
        fog.mask[as_index(tile[1])]
            .chars()
            .nth(as_index(tile[0]))
            .expect("tile inside the mask")
    };
    let flat: String = fog.mask.concat();
    assert!(
        flat.contains('*') && flat.contains(' '),
        "the opening view has both sight and darkness"
    );
    for unit in fog.units.iter().filter(|u| u.player != 0) {
        assert_eq!(
            mask_at(unit.tile),
            '*',
            "a reported hostile must sit on visible ground"
        );
    }
    for entry in fog.scrap.iter().chain(&fog.wrecks) {
        assert_ne!(
            mask_at(entry.tile),
            ' ',
            "remembered salvage never leaks from unexplored ground"
        );
    }
}

#[test]
fn the_fog_view_carries_only_the_viewing_seats_economy() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[0].scrap = 73;
    scenario.players[1].scrap = 987_654;
    let mut state = scenario.build().unwrap();
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(0),
        command: oxide_sim::Command::Surrender,
    }]);

    let fog = FogView::capture(&state, PlayerId(0));
    assert_eq!(fog.player, 0);
    assert_eq!(fog.own_player.id, 0);
    assert_eq!(fog.own_player.scrap, 73);
    assert!(fog.own_player.resigned);
    assert_eq!(
        fog.own_player.units,
        state
            .units()
            .iter()
            .filter(|unit| unit.player.0 == 0)
            .count()
    );
    assert_eq!(
        fog.own_player.buildings,
        state
            .buildings()
            .iter()
            .filter(|building| building.player.0 == 0)
            .count()
    );

    let encoded = serde_json::to_value(&fog).unwrap();
    assert!(
        encoded.get("players").is_none(),
        "opponent rows stay absent"
    );
    assert_eq!(encoded["own_player"]["scrap"], 73);
    assert_ne!(encoded["own_player"]["scrap"], 987_654);

    let omniscient = StateView::capture(&state, StateFilter::default());
    assert!(omniscient.players[0].resigned);
    assert!(!omniscient.players[1].resigned);
}

#[test]
fn the_fog_view_shows_hostile_bodies_but_never_their_minds() {
    let mut state = oxide_sim::Scenario::skirmish().build().unwrap();
    // March a seat-1 machine into seat 0's sight so a hostile row
    // exists, with a live order program behind it.
    let intruder = state
        .units()
        .iter()
        .find(|u| u.player.0 == 1)
        .expect("seat 1 has starting units")
        .id;
    let own_tile = state.units()[0].tile();
    state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: oxide_sim::Command::Run {
            units: vec![intruder],
            goal: own_tile,
            queue: false,
        },
    }]);
    for _ in 0..2_000 {
        let fog = FogView::capture(&state, oxide_sim::PlayerId(0));
        if let Some(row) = fog.units.iter().find(|u| u.player == 1) {
            assert_eq!(row.order, None, "a hostile program is fog's to hide");
            assert!(row.queue.is_empty());
            assert_eq!(row.patrolling, None);
            let own = fog.units.iter().find(|u| u.player == 0).unwrap();
            assert!(own.order.is_some(), "own intent stays first-class");
            // Hostile buildings under sight carry no production
            // intelligence either.
            for b in fog.buildings.iter().filter(|b| b.player == 1) {
                assert_eq!(b.queue, None);
                assert_eq!(b.ticks_remaining, None);
                assert_eq!(b.rally, None);
                assert_eq!(b.focus, None);
                if b.built {
                    assert_eq!(b.progress, 0, "training progress is fog's to hide");
                }
            }
            return;
        }
        state.tick(&[]);
    }
    panic!("the intruder never reached seat 0's sight");
}

#[test]
fn the_fog_view_keeps_a_hostile_landing_while_hiding_its_program() {
    use oxide_sim::scenario::{PlayerSpec, UnitSpec};
    // An open field: seat 0's unarmed Harvester holds sight on the
    // ground a seat-1 Condor is ordered onto, so nothing shoots the
    // airframe down or draws it back into the air.
    let player = |name: &str, faction| PlayerSpec {
        name: name.into(),
        faction,
        team: None,
        scrap: 100,
        bot: false,
        bot_config: None,
    };
    let scenario = oxide_sim::Scenario {
        mode: ScenarioMode::Match,
        name: "landing-fog".into(),
        seed: 11,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#..2...................#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: vec![
            player("Ferrous", Faction::Ferrous),
            player("Cupric", Faction::Cupric),
        ],
        units: vec![
            // Beyond the Condor's acquisition range from its landing
            // tile at (13, 8) and off its approach, inside its own sight.
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 18,
                y: 6,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Condor,
                x: 6,
                y: 12,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let condor = state
        .units()
        .iter()
        .find(|u| u.kind == UnitKind::Condor)
        .expect("the Condor spawned")
        .id;
    state.tick(&[oxide_sim::PlayerCommand {
        player: PlayerId(1),
        command: oxide_sim::Command::Run {
            units: vec![condor],
            goal: TilePos::new(13, 8),
            queue: false,
        },
    }]);
    for _ in 0..1_500 {
        state.tick(&[]);
        if !state.unit(condor).is_some_and(|u| u.landed) {
            continue;
        }
        let fog = FogView::capture(&state, PlayerId(0));
        let seen = fog
            .units
            .iter()
            .find(|u| u.id == condor.0)
            .expect("the parked Condor sits inside seat 0's sight");
        assert!(seen.landed, "a landing is a physical fact fog never hides");
        assert_eq!(seen.order, None, "the program behind it stays hidden");
        assert!(seen.queue.is_empty());
        assert_eq!(seen.patrolling, None);
        assert_eq!(serde_json::to_value(seen).unwrap()["landed"], true);

        let harvester = fog.units.iter().find(|u| u.player == 0).unwrap();
        assert!(!harvester.landed);
        assert!(
            serde_json::to_value(harvester)
                .unwrap()
                .get("landed")
                .is_none(),
            "a machine that is not parked carries no landed field"
        );

        let omniscient = StateView::capture(&state, StateFilter::default());
        let full = omniscient.units.iter().find(|u| u.id == condor.0).unwrap();
        assert!(full.landed);
        assert!(full.order.is_some(), "omniscient captures keep the program");
        return;
    }
    panic!("the Condor never touched down");
}

#[test]
fn unit_view_exposes_queued_orders() {
    let mut state = oxide_sim::Scenario::skirmish().build().unwrap();
    let mover = state.units()[0].id;
    state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(0),
        command: oxide_sim::Command::Patrol {
            units: vec![mover],
            waypoints: vec![
                chassis::grid::TilePos::new(10, 10),
                chassis::grid::TilePos::new(14, 6),
                chassis::grid::TilePos::new(8, 12),
            ],
        },
    }]);
    let view = StateView::capture(&state, StateFilter::default());
    let u = view.units.iter().find(|u| u.id == mover.0).unwrap();
    assert_eq!(u.patrolling, Some(true));
    assert_eq!(u.queue.len(), 2, "the remaining circuit legs are visible");
}

#[test]
fn building_view_reports_remaining_train_time() {
    let mut state = oxide_sim::Scenario::skirmish().build().unwrap();
    let foundry = state.buildings()[0].id;
    state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(0),
        command: oxide_sim::Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    }]);
    let view = StateView::capture(&state, StateFilter::default());
    let b = view.buildings.iter().find(|b| b.id == foundry.0).unwrap();
    assert_eq!(b.queue.as_deref(), Some([UnitKind::Harvester].as_slice()));
    let remaining = b.ticks_remaining.unwrap();
    assert!(remaining > 0 && remaining <= UnitKind::Harvester.stats().train_ticks);
}

#[test]
fn building_view_exposes_allied_focus_and_redacts_hostile_focus() {
    let target = oxide_sim::Target::Unit(oxide_sim::UnitId(7));
    let building = oxide_sim::Building {
        id: oxide_sim::BuildingId(3),
        player: oxide_sim::PlayerId(0),
        kind: oxide_sim::BuildingKind::Turret,
        anchor: chassis::grid::TilePos::new(4, 5),
        hp: oxide_sim::BuildingKind::Turret.base_stats().max_hp,
        queue: std::collections::VecDeque::new(),
        progress: 0,
        rally: None,
        focus: Some(target.into()),
        built: true,
        provisional: false,
        tier: 0,
        cooldown: 0,
        salvage_drained: 0,
        salvage_credited: 0,
        salvaged: false,
    };

    assert_eq!(building_view(&building).focus, Some(target.into()));
    assert_eq!(building_view_redacted(&building).focus, None);
}

#[test]
fn the_overlaid_debug_map_is_a_picture_not_a_parseable_scenario() {
    let state = oxide_sim::Scenario::skirmish().build().unwrap();
    let view = StateView::capture(
        &state,
        StateFilter {
            map: true,
            ..StateFilter::default()
        },
    );
    let rows = view.map.expect("map requested");
    let flat: String = rows.concat();
    assert!(
        flat.contains('A') || flat.contains('a'),
        "the overlay paints entities as letters"
    );
    // Those entity letters are not in the terrain legend, so the debug
    // map never round-trips back through the scenario parser — the same
    // reason the render-only wreck glyph stays out of authorable maps.
    match oxide_sim::map::Map::parse(&rows) {
        Err(oxide_sim::map::MapError::UnknownChar { c, .. }) => {
            assert!(
                c.is_ascii_alphabetic(),
                "rejected on an entity glyph, got {c:?}"
            );
        }
        other => panic!("expected the overlay to be unparseable terrain, got {other:?}"),
    }
}
