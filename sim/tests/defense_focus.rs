//! Explicit focus-fire orders for static defenses.

mod common;

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::{
    BuildingId, BuildingKind, Command, Event, PlayerId, State, Target, UnitId, UnitKind,
};

use common::{cmd, open_arena, open_arena_with, run_until, unit};

fn building(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

fn building_id(state: &State, player: u8, kind: BuildingKind, nth: usize) -> BuildingId {
    state
        .buildings()
        .iter()
        .filter(|building| building.player == PlayerId(player) && building.kind == kind)
        .nth(nth)
        .unwrap()
        .id
}

fn unit_ids(state: &State, player: u8) -> Vec<UnitId> {
    state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(player))
        .map(|unit| unit.id)
        .collect()
}

fn two_turret_state() -> State {
    let mut scenario = open_arena(
        30,
        18,
        vec![
            unit(1, UnitKind::Harvester, 10, 6),
            unit(1, UnitKind::Harvester, 11, 8),
        ],
    );
    scenario.buildings = vec![
        building(0, BuildingKind::Turret, 5, 5),
        building(0, BuildingKind::Turret, 5, 8),
    ];
    scenario.build().unwrap()
}

#[test]
fn focus_fire_canonicalizes_a_multi_defense_selection_deterministically() {
    let base = two_turret_state();
    let defenses = [
        building_id(&base, 0, BuildingKind::Turret, 0),
        building_id(&base, 0, BuildingKind::Turret, 1),
    ];
    let target = Target::Unit(unit_ids(&base, 1)[0]);

    let mut canonical = base.clone();
    let canonical_report = canonical.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: defenses.to_vec(),
            target,
        },
    )]);
    assert!(
        !canonical_report
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. }))
    );

    let mut repeated = base;
    repeated.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![defenses[1], defenses[0], defenses[1]],
            target,
        },
    )]);

    assert_eq!(repeated.hash(), canonical.hash());
    for defense in defenses {
        assert_eq!(canonical.building(defense).unwrap().focus, Some(target));
    }
}

#[test]
fn focus_fire_validation_is_atomic_for_mixed_or_foreign_buildings() {
    let mut state = two_turret_state();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let own_foundry = building_id(&state, 0, BuildingKind::Foundry, 0);
    let foreign_foundry = building_id(&state, 1, BuildingKind::Foundry, 0);
    let targets = unit_ids(&state, 1);
    let first = Target::Unit(targets[0]);
    let replacement = Target::Unit(targets[1]);

    state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![turret],
            target: first,
        },
    )]);
    assert_eq!(state.building(turret).unwrap().focus, Some(first));

    for (invalid, reason) in [
        (own_foundry, RejectReason::InvalidTarget),
        (foreign_foundry, RejectReason::NotYourBuilding),
    ] {
        let report = state.tick(&[cmd(
            0,
            Command::FocusFire {
                buildings: vec![turret, invalid],
                target: replacement,
            },
        )]);
        assert!(report.events.contains(&Event::CommandRejected {
            player: PlayerId(0),
            reason,
        }));
        assert_eq!(
            state.building(turret).unwrap().focus,
            Some(first),
            "a rejected mixed selection partially changed its valid member"
        );
    }
}

#[test]
fn focus_fire_rejects_hidden_and_wrong_domain_targets() {
    let mut scenario = open_arena(
        40,
        20,
        vec![
            unit(1, UnitKind::Harvester, 25, 10),
            unit(1, UnitKind::Harvester, 9, 7),
        ],
    );
    scenario.buildings = vec![
        building(0, BuildingKind::Turret, 5, 6),
        building(0, BuildingKind::FlakTurret, 6, 8),
    ];
    let mut state = scenario.build().unwrap();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let flak = building_id(&state, 0, BuildingKind::FlakTurret, 0);
    let targets = unit_ids(&state, 1);

    for (defense, target) in [(turret, targets[0]), (flak, targets[1])] {
        let report = state.tick(&[cmd(
            0,
            Command::FocusFire {
                buildings: vec![defense],
                target: Target::Unit(target),
            },
        )]);
        assert!(report.events.contains(&Event::CommandRejected {
            player: PlayerId(0),
            reason: RejectReason::InvalidTarget,
        }));
        assert_eq!(state.building(defense).unwrap().focus, None);
    }
}

#[test]
fn a_reachable_focus_preempts_the_nearest_ordinary_target() {
    let mut scenario = open_arena(
        24,
        16,
        vec![
            unit(1, UnitKind::Harvester, 9, 6),
            unit(1, UnitKind::Harvester, 11, 6),
        ],
    );
    scenario.buildings = vec![building(0, BuildingKind::Turret, 6, 6)];
    let mut state = scenario.build().unwrap();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let targets = unit_ids(&state, 1);
    let near_hp = state.unit(targets[0]).unwrap().hp;
    let focus_hp = state.unit(targets[1]).unwrap().hp;

    let report = state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![turret],
            target: Target::Unit(targets[1]),
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::TurretFired {
            turret: fired,
            target: Target::Unit(target),
            ..
        } if *fired == turret && *target == targets[1]
    )));
    assert_eq!(state.unit(targets[0]).unwrap().hp, near_hp);
    assert!(state.unit(targets[1]).unwrap().hp < focus_hp);
}

#[test]
fn an_out_of_range_focus_is_retained_while_the_defense_fires_normally() {
    let mut scenario = open_arena(
        32,
        18,
        vec![
            unit(1, UnitKind::Harvester, 9, 6),
            unit(1, UnitKind::Harvester, 15, 6),
        ],
    );
    scenario.buildings = vec![
        building(0, BuildingKind::Turret, 6, 6),
        building(0, BuildingKind::Array, 11, 8),
    ];
    let mut state = scenario.build().unwrap();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let targets = unit_ids(&state, 1);

    let report = state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![turret],
            target: Target::Unit(targets[1]),
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::TurretFired {
            turret: fired,
            target: Target::Unit(target),
            ..
        } if *fired == turret && *target == targets[0]
    )));
    assert_eq!(
        state.building(turret).unwrap().focus,
        Some(Target::Unit(targets[1]))
    );
}

#[test]
fn focus_clears_as_soon_as_fresh_true_sight_loses_the_target() {
    let mut scenario = open_arena(
        40,
        20,
        vec![
            unit(0, UnitKind::Harvester, 18, 10),
            unit(1, UnitKind::Harvester, 21, 10),
        ],
    );
    scenario.buildings = vec![building(0, BuildingKind::Turret, 5, 6)];
    let mut state = scenario.build().unwrap();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let spotter = unit_ids(&state, 0)[0];
    let target = unit_ids(&state, 1)[0];
    assert!(state.can_see(PlayerId(0), state.unit(target).unwrap().tile()));

    state.tick(&[
        cmd(
            0,
            Command::FocusFire {
                buildings: vec![turret],
                target: Target::Unit(target),
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![spotter],
                goal: TilePos::new(5, 14),
                queue: false,
            },
        ),
    ]);
    run_until(&mut state, 500, |state, _| {
        state.building(turret).unwrap().focus.is_none()
    });
    assert!(!state.can_see(PlayerId(0), state.unit(target).unwrap().tile()));
}

#[test]
fn lethal_focus_fire_does_not_leave_a_dangling_preference() {
    let mut scenario = open_arena(24, 16, vec![unit(1, UnitKind::Harvester, 9, 6)]);
    scenario.buildings = vec![building(0, BuildingKind::Turret, 6, 6)];
    let mut state = scenario.build().unwrap();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let target = unit_ids(&state, 1)[0];
    state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![turret],
            target: Target::Unit(target),
        },
    )]);

    run_until(&mut state, 500, |state, _| state.unit(target).is_none());
    assert_eq!(state.building(turret).unwrap().focus, None);
}

#[test]
fn a_bastion_focuses_a_visible_building_ahead_of_an_ordinary_unit() {
    let mut scenario = open_arena(32, 20, vec![unit(1, UnitKind::Harvester, 10, 7)]);
    scenario.buildings = vec![
        building(0, BuildingKind::Bastion, 5, 6),
        building(0, BuildingKind::Array, 10, 10),
        building(1, BuildingKind::Reclaimer, 14, 7),
    ];
    let mut state = scenario.build().unwrap();
    let bastion = building_id(&state, 0, BuildingKind::Bastion, 0);
    let target = building_id(&state, 1, BuildingKind::Reclaimer, 0);
    let aim = state
        .building(target)
        .unwrap()
        .closest_point_to(state.building(bastion).unwrap().center());

    let report = state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![bastion],
            target: Target::Building(target),
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::ShellLaunched {
            shooter: Target::Building(shooter),
            to,
            ..
        } if *shooter == bastion && *to == aim
    )));
    assert_eq!(
        state.building(bastion).unwrap().focus,
        Some(Target::Building(target))
    );
}

#[test]
fn a_direct_fire_turret_can_focus_a_hostile_building() {
    let mut scenario = open_arena(24, 16, Vec::new());
    scenario.buildings = vec![
        building(0, BuildingKind::Turret, 6, 6),
        building(1, BuildingKind::Reclaimer, 10, 6),
    ];
    let mut state = scenario.build().unwrap();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let target = building_id(&state, 1, BuildingKind::Reclaimer, 0);
    let before = state.building(target).unwrap().hp;

    let report = state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![turret],
            target: Target::Building(target),
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::TurretFired {
            turret: fired,
            target: Target::Building(hit),
            ..
        } if *fired == turret && *hit == target
    )));
    assert!(state.building(target).unwrap().hp < before);
}

#[test]
fn missing_focus_field_deserializes_as_no_preference() {
    let mut state = two_turret_state();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let target = Target::Unit(unit_ids(&state, 1)[0]);
    state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![turret],
            target,
        },
    )]);
    let mut document = serde_json::to_value(&state).unwrap();
    for building in document["buildings"].as_array_mut().unwrap() {
        building.as_object_mut().unwrap().remove("focus");
    }

    let restored: State = serde_json::from_value(document).unwrap();
    assert!(
        restored
            .buildings()
            .iter()
            .all(|building| building.focus.is_none())
    );
    restored.validate_invariants().unwrap();
}

#[test]
fn peak_obstruction_keeps_focus_but_allows_a_clear_fallback() {
    let mut scenario = open_arena_with(
        28,
        18,
        vec![
            unit(1, UnitKind::Harvester, 9, 9),
            unit(1, UnitKind::Harvester, 11, 6),
        ],
        |rows| rows[6][9] = '^',
    );
    scenario.buildings = vec![
        building(0, BuildingKind::Turret, 6, 6),
        building(0, BuildingKind::Array, 10, 10),
    ];
    let mut state = scenario.build().unwrap();
    let turret = building_id(&state, 0, BuildingKind::Turret, 0);
    let targets = unit_ids(&state, 1);

    let report = state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![turret],
            target: Target::Unit(targets[1]),
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::TurretFired {
            target: Target::Unit(target),
            ..
        } if *target == targets[0]
    )));
    assert_eq!(
        state.building(turret).unwrap().focus,
        Some(Target::Unit(targets[1]))
    );
}
