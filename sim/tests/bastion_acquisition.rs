//! Bastion acquisition against live structure footprints under fog.

mod common;

use chassis::grid::TilePos;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::{BuildingId, BuildingKind, Command, Event, PlayerId, Target, UnitKind};

use common::*;

fn building(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

fn bastion_and_target(
    target: TilePos,
    spotter: Option<TilePos>,
    peak: Option<TilePos>,
) -> oxide_sim::State {
    let mut scenario = open_arena_with(32, 22, Vec::new(), |rows| {
        if let Some(peak) = peak {
            rows[peak.y as usize][peak.x as usize] = '^';
        }
    });
    scenario.buildings = vec![
        building(0, BuildingKind::Bastion, 5, 6),
        building(1, BuildingKind::Reclaimer, target.x, target.y),
    ];
    if let Some(spotter) = spotter {
        scenario
            .buildings
            .push(building(0, BuildingKind::Array, spotter.x, spotter.y));
    }
    scenario.build().unwrap()
}

fn building_id(state: &oxide_sim::State, player: PlayerId, kind: BuildingKind) -> BuildingId {
    state
        .buildings()
        .iter()
        .find(|building| building.player == player && building.kind == kind)
        .unwrap()
        .id
}

fn bastion_launch(events: &[Event], bastion: BuildingId) -> Option<chassis::fx::Vec2Fx> {
    events.iter().find_map(|event| match event {
        Event::ShellLaunched {
            shooter: Target::Building(shooter),
            to,
            ..
        } if *shooter == bastion => Some(*to),
        _ => None,
    })
}

#[test]
fn array_true_sight_lets_a_bastion_shell_a_visible_building_footprint() {
    let target_anchor = TilePos::new(14, 7);
    let hidden = bastion_and_target(target_anchor, None, None);
    assert!(
        !hidden.can_see(PlayerId(0), target_anchor),
        "the target must sit beyond the Bastion's own sight"
    );

    let mut state = bastion_and_target(target_anchor, Some(TilePos::new(10, 10)), None);
    let bastion = building_id(&state, PlayerId(0), BuildingKind::Bastion);
    let target = building_id(&state, PlayerId(1), BuildingKind::Reclaimer);
    assert!(state.can_see(PlayerId(0), target_anchor));

    let expected_aim = state
        .building(target)
        .unwrap()
        .closest_point_to(state.building(bastion).unwrap().center());
    let before = state.building(target).unwrap().hp;
    let report = state.tick(&[]);
    assert_eq!(
        bastion_launch(&report.events, bastion),
        Some(expected_aim),
        "the shell must aim at the closest footprint edge, not the building center"
    );
    run_until(&mut state, 100, |state, _| {
        state
            .building(target)
            .is_some_and(|building| building.hp < before)
    });
}

#[test]
fn bastion_building_acquisition_honors_both_range_edges_and_peak_cover() {
    for (name, target) in [
        ("inside the dead zone", TilePos::new(8, 7)),
        ("beyond maximum range", TilePos::new(16, 7)),
    ] {
        let mut state = bastion_and_target(target, Some(TilePos::new(12, 11)), None);
        let bastion = building_id(&state, PlayerId(0), BuildingKind::Bastion);
        assert!(
            state.can_see(PlayerId(0), target),
            "{name} target is visible"
        );
        for _ in 0..120 {
            let report = state.tick(&[]);
            assert!(
                bastion_launch(&report.events, bastion).is_none(),
                "Bastion fired at a building {name}"
            );
        }
    }

    let target = TilePos::new(14, 7);
    let mut blocked = bastion_and_target(
        target,
        Some(TilePos::new(10, 11)),
        Some(TilePos::new(10, 7)),
    );
    let bastion = building_id(&blocked, PlayerId(0), BuildingKind::Bastion);
    assert!(blocked.can_see(PlayerId(0), target));
    for _ in 0..120 {
        let report = blocked.tick(&[]);
        assert!(
            bastion_launch(&report.events, bastion).is_none(),
            "indirect fire must still stop at a peak"
        );
    }
}

#[test]
fn bastion_fallback_does_not_shell_when_the_footprint_aim_lands_on_a_peak() {
    let mut scenario = open_arena_with(24, 18, Vec::new(), |rows| rows[7][8] = '^');
    scenario.buildings = vec![
        building(0, BuildingKind::Bastion, 11, 6),
        building(1, BuildingKind::Reclaimer, 7, 7),
    ];
    let mut state = scenario.build().unwrap();
    let bastion = building_id(&state, PlayerId(0), BuildingKind::Bastion);
    let target = building_id(&state, PlayerId(1), BuildingKind::Reclaimer);
    let before = state.building(target).unwrap().hp;
    assert!(state.can_see(PlayerId(0), TilePos::new(7, 7)));

    for _ in 0..120 {
        let report = state.tick(&[]);
        assert!(
            bastion_launch(&report.events, bastion).is_none(),
            "the closest-point endpoint cannot tunnel an indirect shell through a peak: {:?}",
            report.events
        );
    }
    assert_eq!(state.building(target).unwrap().hp, before);
}

#[test]
fn an_eligible_visible_unit_keeps_priority_over_a_closer_building() {
    let mut scenario = open_arena(32, 22, vec![unit(1, UnitKind::Harvester, 14, 7)]);
    scenario.buildings = vec![
        building(0, BuildingKind::Bastion, 5, 6),
        building(1, BuildingKind::Reclaimer, 9, 9),
        building(0, BuildingKind::Array, 10, 10),
    ];
    let mut state = scenario.build().unwrap();
    let bastion = building_id(&state, PlayerId(0), BuildingKind::Bastion);
    let unit_position = state.units()[0].pos;

    let report = state.tick(&[]);
    assert_eq!(
        bastion_launch(&report.events, bastion),
        Some(unit_position),
        "the existing visible-unit acquisition tier must remain ahead of buildings"
    );
}

#[test]
fn bastion_does_not_read_a_path_established_later_this_tick() {
    let mut scenario = open_arena(32, 22, vec![unit(1, UnitKind::Harvester, 13, 7)]);
    scenario.buildings = vec![
        building(0, BuildingKind::Bastion, 5, 6),
        building(0, BuildingKind::Array, 10, 10),
    ];
    let mut state = scenario.build().unwrap();
    let bastion = building_id(&state, PlayerId(0), BuildingKind::Bastion);
    let target = state.units()[0].id;
    let current = state.unit(target).unwrap().pos;
    let before = state.unit(target).unwrap().hp;

    let report = state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![target],
            goal: TilePos::new(13, 12),
            queue: false,
        },
    )]);
    let aim = bastion_launch(&report.events, bastion).expect("the visible target is in range");
    assert_eq!(
        aim, current,
        "a new command is not observable motion until the next tick"
    );

    run_until(&mut state, 100, |_, events| {
        events
            .iter()
            .any(|event| matches!(event, Event::ShellLanded { .. }))
    });
    assert_eq!(
        state.unit(target).unwrap().hp,
        before,
        "the same-tick move may dodge the unguided current-position shot"
    );
}

#[test]
fn bastion_keeps_a_legal_aim_when_prediction_enters_its_dead_zone() {
    let mut scenario = open_arena(32, 22, vec![unit(1, UnitKind::Harvester, 9, 7)]);
    scenario.buildings = vec![building(0, BuildingKind::Bastion, 5, 6)];
    let mut state = scenario.build().unwrap();
    let bastion = building_id(&state, PlayerId(0), BuildingKind::Bastion);
    let target = state.units()[0].id;
    let current = state.unit(target).unwrap().pos;
    let center = state.building(bastion).unwrap().center();
    let minimum = BuildingKind::Bastion.base_stats().weapons[0].minimum_range;
    assert!(center.dist_sq(current) >= minimum * minimum);

    let report = state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![target],
            goal: TilePos::new(7, 7),
            queue: false,
        },
    )]);
    let aim = bastion_launch(&report.events, bastion).expect("the legal target is fired upon");
    assert_eq!(
        aim, current,
        "an inward prediction cannot create an impact inside the dead zone"
    );
    assert!(center.dist_sq(aim) >= minimum * minimum);
}

#[test]
fn building_ghosts_and_radar_contacts_do_not_enable_bastion_fire() {
    let target_anchor = TilePos::new(13, 10);
    let mut scenario = open_arena(
        32,
        22,
        vec![
            unit(0, UnitKind::Harvester, 12, 8),
            unit(1, UnitKind::Harvester, 14, 8),
        ],
    );
    scenario.buildings = vec![
        building(0, BuildingKind::Bastion, 5, 6),
        building(1, BuildingKind::Reclaimer, target_anchor.x, target_anchor.y),
        building(0, BuildingKind::Array, 5, 16),
    ];
    let mut state = scenario.build().unwrap();
    let bastion = building_id(&state, PlayerId(0), BuildingKind::Bastion);
    let scout = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0))
        .unwrap()
        .id;
    let radar_target = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(1))
        .unwrap()
        .id;

    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(2, 14),
            queue: false,
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
        let radar_tile = state.unit(radar_target).unwrap().tile();
        if !state.can_see(PlayerId(0), target_anchor) && !state.can_see(PlayerId(0), radar_tile) {
            break;
        }
    }

    let radar_tile = state.unit(radar_target).unwrap().tile();
    assert!(!state.can_see(PlayerId(0), target_anchor));
    assert!(
        state
            .vision(PlayerId(0))
            .ghosts()
            .iter()
            .any(|ghost| ghost.anchor == target_anchor)
    );
    assert!(!state.can_see(PlayerId(0), radar_tile));
    assert!(state.vision(PlayerId(0)).contacts().contains(&radar_tile));

    for _ in 0..(BuildingKind::Bastion.base_stats().weapons[0].cooldown_ticks + 30) {
        let report = state.tick(&[]);
        assert!(
            bastion_launch(&report.events, bastion).is_none(),
            "memory and unidentified radar must not authorize a shot"
        );
    }
}
