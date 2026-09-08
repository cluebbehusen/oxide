//! Ground chassis and independent mounts share deterministic turn limits.
mod common;
use chassis::grid::TilePos;
use common::*;
use oxide_sim::stats::Domain;
use oxide_sim::{Command, Event, Target, UnitKind};

#[test]
fn every_ground_chassis_pivots_before_moving_and_completes_its_route() {
    for kind in UnitKind::ALL
        .into_iter()
        .filter(|k| k.stats().domain == Domain::Ground)
    {
        let mut state = open_arena(32, 24, vec![unit(0, kind, 6, 10)])
            .build()
            .unwrap();
        let mut document = serde_json::to_value(&state).unwrap();
        document["units"][0]["heading"] = serde_json::json!(128);
        state = serde_json::from_value(document).unwrap();
        let id = state.units()[0].id;
        let start = state.unit(id).unwrap().pos;
        let mut previous = state.unit(id).unwrap().heading;
        assert_eq!(previous, 128);
        let mut moved = false;
        for tick in 0..400 {
            let commands = if tick == 0 {
                vec![cmd(
                    0,
                    Command::Move {
                        units: vec![id],
                        goal: TilePos::new(14, 10),
                        queue: false,
                    },
                )]
            } else {
                vec![]
            };
            state.tick(&commands);
            let u = state.unit(id).unwrap();
            let change = u
                .heading
                .wrapping_sub(previous)
                .cast_signed()
                .unsigned_abs();
            assert!(change <= kind.ground_turn_rate(), "{kind:?}: {change}");
            previous = u.heading;
            if tick < 2 {
                assert_eq!(u.pos, start, "{kind:?} moved before pivoting");
            }
            if u.pos != start && !moved {
                assert!(u.heading.cast_signed().unsigned_abs() <= 8, "{kind:?}");
                moved = true;
            }
            if u.tile() == TilePos::new(14, 10) {
                break;
            }
        }
        assert!(moved, "{kind:?} never moved");
        assert_eq!(
            state.unit(id).unwrap().tile(),
            TilePos::new(14, 10),
            "{kind:?}"
        );
        state.validate_invariants().unwrap();
    }
}

#[test]
fn independent_ground_guns_traverse_without_spinning_the_hull() {
    for kind in [UnitKind::Sentinel, UnitKind::Warden, UnitKind::Lancer] {
        let mut scenario = open_arena(26, 18, vec![unit(0, kind, 6, 8)]);
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 1,
            kind: oxide_sim::BuildingKind::Fabricator,
            x: 9,
            y: 8,
        });
        let mut state = scenario.build().unwrap();
        let mut document = serde_json::to_value(&state).unwrap();
        document["units"][0]["heading"] = serde_json::json!(128);
        state = serde_json::from_value(document).unwrap();
        let id = state.units()[0].id;
        let target = state
            .buildings()
            .iter()
            .find(|b| b.kind == oxide_sim::BuildingKind::Fabricator)
            .unwrap()
            .id;
        let mut previous = 128u8;
        let mut resumed: Option<oxide_sim::State> = None;
        let mut fired = false;
        for tick in 0..60 {
            let commands = if tick == 0 {
                vec![cmd(
                    0,
                    Command::Attack {
                        units: vec![id],
                        target: Target::Building(target),
                        queue: false,
                    },
                )]
            } else {
                vec![]
            };
            let report = state.tick(&commands);
            if let Some(copy) = resumed.as_mut() {
                assert_eq!(copy.tick(&commands).events, report.events);
                assert_eq!(copy.hash(), state.hash());
            }
            if tick == 5 {
                resumed =
                    Some(serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap());
            }
            let u = state.unit(id).unwrap();
            assert_eq!(u.heading, 128, "{kind:?} spun its hull");
            let bearing = u.weapon_heading();
            assert!(
                bearing.wrapping_sub(previous).cast_signed().unsigned_abs()
                    <= kind.turret_turn_rate()
            );
            previous = bearing;
            if report
                .events
                .iter()
                .any(|e| matches!(e, Event::AttackHit {attacker, ..} if *attacker == id))
            {
                assert!(tick > 0);
                assert!(bearing.cast_signed().unsigned_abs() <= 2);
                fired = true;
                break;
            }
        }
        assert!(fired, "{kind:?} never fired");
    }
}

#[test]
fn mirrored_ground_spawns_have_opposite_initial_bearings() {
    for kind in UnitKind::ALL
        .into_iter()
        .filter(|kind| kind.stats().domain == Domain::Ground)
    {
        let state = open_arena(32, 24, vec![unit(0, kind, 6, 8), unit(1, kind, 25, 15)])
            .build()
            .unwrap();
        assert_eq!(
            state.units()[0].heading.wrapping_add(128),
            state.units()[1].heading,
            "{kind:?}"
        );
    }
}

#[test]
fn a_ground_sidearm_cannot_fire_across_its_main_guns_bearing() {
    let mut state = open_arena(
        20,
        16,
        vec![
            unit(0, UnitKind::Sentinel, 5, 8),
            unit(1, UnitKind::Scuttler, 7, 8),
            unit(1, UnitKind::Darter, 5, 6),
        ],
    )
    .build()
    .unwrap();
    let (gun, ground, air) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    face_target(&mut state, gun, Target::Unit(ground));
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![gun],
            target: Target::Unit(ground),
            queue: false,
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(event,Event::AttackHit{attacker,target:Target::Unit(id),..} if *attacker==gun && *id==ground)));
    assert!(!report.events.iter().any(|event| matches!(event,Event::AttackHit{attacker,target:Target::Unit(id),..} if *attacker==gun && *id==air)));
    assert_eq!(state.unit(air).unwrap().hp, UnitKind::Darter.stats().max_hp);
    assert_eq!(state.unit(gun).unwrap().cooldowns[1], 0);
}
