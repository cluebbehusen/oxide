//! Automatic static-defense targeting shares visible-hostile legality.

mod common;

use chassis::grid::TilePos;
use oxide_sim::{BuildingId, BuildingKind, Command, Event, PlayerId, State, Target, UnitKind};

use common::{building, cmd, open_arena, open_arena_with, run_until, unit};

fn id(state: &State, kind: BuildingKind) -> BuildingId {
    state
        .buildings()
        .iter()
        .find(|b| b.kind == kind)
        .unwrap()
        .id
}

fn shot(events: &[Event], defense: BuildingId) -> Option<Target> {
    events.iter().find_map(|event| match event {
        Event::TurretFired { turret, target, .. } if *turret == defense => *target,
        Event::ShellLaunched {
            shooter: Target::Building(shooter),
            target,
            ..
        } if *shooter == defense => *target,
        _ => None,
    })
}

fn scene(kind: BuildingKind, target: BuildingKind) -> oxide_sim::Scenario {
    let mut scenario = open_arena(32, 22, Vec::new());
    scenario.buildings = vec![building(0, kind, 5, 6), building(1, target, 9, 7)];
    scenario
}

#[test]
fn ground_defenses_damage_buildings_without_commands_at_every_tier() {
    for kind in [BuildingKind::Turret, BuildingKind::Bastion] {
        for tier in 0..kind.tiers().len() {
            let mut state = scene(kind, BuildingKind::Reclaimer).build().unwrap();
            let defense = id(&state, kind);
            let target = id(&state, BuildingKind::Reclaimer);
            let mut value = serde_json::to_value(&state).unwrap();
            let row = value["buildings"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|row| row["id"] == serde_json::json!(defense))
                .unwrap();
            row["tier"] = serde_json::json!(tier);
            state = serde_json::from_value(value).unwrap();
            let before = state.building(target).unwrap().hp;
            let mut replayed = state.clone();
            let report = state.tick(&[]);
            assert_eq!(
                shot(&report.events, defense),
                Some(Target::Building(target)),
                "{kind:?} tier {tier}"
            );
            for _ in 0..100 {
                state.tick(&[]);
            }
            for _ in 0..101 {
                replayed.tick(&[]);
            }
            assert_eq!(state.hash(), replayed.hash());
            assert!(state.building(target).is_none_or(|b| b.hp < before));
        }
    }
}

#[test]
fn units_take_automatic_priority_but_explicit_building_focus_wins() {
    for kind in [BuildingKind::Turret, BuildingKind::Bastion] {
        let mut scenario = scene(kind, BuildingKind::Reclaimer);
        scenario.units.push(unit(1, UnitKind::Harvester, 10, 6));
        let mut automatic = scenario.build().unwrap();
        let defense = id(&automatic, kind);
        let target = id(&automatic, BuildingKind::Reclaimer);
        let victim = automatic.units()[0].id;
        let mut focused = automatic.clone();
        assert_eq!(
            shot(&automatic.tick(&[]).events, defense),
            Some(Target::Unit(victim))
        );
        let report = focused.tick(&[cmd(
            0,
            Command::FocusFire {
                buildings: vec![defense],
                target: Target::Building(target).into(),
            },
        )]);
        assert_eq!(
            shot(&report.events, defense),
            Some(Target::Building(target))
        );
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. }))
        );
    }
}

#[test]
fn ineligible_air_unit_does_not_block_ground_building_fallback() {
    let mut scenario = scene(BuildingKind::Turret, BuildingKind::Reclaimer);
    scenario.units.push(unit(1, UnitKind::Gnat, 8, 8));
    let mut state = scenario.build().unwrap();
    let defense = id(&state, BuildingKind::Turret);
    let target = id(&state, BuildingKind::Reclaimer);
    assert_eq!(
        shot(&state.tick(&[]).events, defense),
        Some(Target::Building(target))
    );
}

#[test]
fn flak_never_automatically_attacks_buildings() {
    let mut state = scene(BuildingKind::FlakTurret, BuildingKind::Reclaimer)
        .build()
        .unwrap();
    let defense = id(&state, BuildingKind::FlakTurret);
    let target = id(&state, BuildingKind::Reclaimer);
    let before = state.building(target).unwrap().hp;
    for _ in 0..120 {
        assert_eq!(shot(&state.tick(&[]).events, defense), None);
    }
    assert_eq!(state.building(target).unwrap().hp, before);
}

#[test]
fn concealed_mines_are_skipped_until_detected() {
    for kind in [BuildingKind::Bastion, BuildingKind::Turret] {
        for detected in [false, true] {
            let mut scenario = scene(kind, BuildingKind::ScuttleCharge);
            if detected {
                scenario.units.push(unit(0, UnitKind::Kestrel, 9, 9));
            }
            let mut state = scenario.build().unwrap();
            let defense = id(&state, kind);
            let target = id(&state, BuildingKind::ScuttleCharge);
            assert!(state.can_see(PlayerId(0), TilePos::new(9, 7)));
            assert_eq!(
                state.building_apparent(PlayerId(0), state.building(target).unwrap()),
                detected
            );
            let before = state.building(target).unwrap().hp;
            let report = state.tick(&[]);
            assert_eq!(
                shot(&report.events, defense),
                detected.then_some(Target::Building(target)),
                "{kind:?}, detected={detected}"
            );
            if detected {
                if state.building(target).is_some_and(|b| b.hp == before) {
                    run_until(&mut state, 100, |state, _| {
                        state.building(target).is_none_or(|b| b.hp < before)
                    });
                }
            } else {
                for _ in 0..120 {
                    assert_eq!(shot(&state.tick(&[]).events, defense), None);
                }
                assert_eq!(state.building(target).unwrap().hp, before);
            }
        }
    }
}

#[test]
fn turret_building_fallback_obeys_range_terrain_and_allegiance() {
    for case in ["out of range", "rock", "peak", "allied", "own"] {
        let mut scenario = open_arena_with(32, 22, Vec::new(), |rows| {
            if case == "rock" {
                rows[7][7] = '#';
            }
            if case == "peak" {
                rows[7][7] = '^';
            }
            if case == "allied" {
                rows[18][1] = '3';
            }
        });
        scenario.buildings = vec![
            building(0, BuildingKind::Turret, 5, 7),
            building(
                if case == "own" { 0 } else { 1 },
                BuildingKind::Reclaimer,
                if case == "out of range" { 12 } else { 9 },
                7,
            ),
            building(0, BuildingKind::Array, 10, 10),
        ];
        if case == "allied" {
            let mut enemy = scenario.players[1].clone();
            enemy.team = Some(1);
            scenario.players.push(enemy);
            scenario.players[0].team = Some(0);
            scenario.players[1].team = Some(0);
        }
        let mut state = scenario.build().unwrap();
        let defense = id(&state, BuildingKind::Turret);
        let target = id(&state, BuildingKind::Reclaimer);
        assert!(state.can_see(PlayerId(0), state.building(target).unwrap().anchor));
        for _ in 0..120 {
            assert_eq!(shot(&state.tick(&[]).events, defense), None, "{case}");
        }
    }
}
