//! Building frontage, rerouting, and deterministic Scuttler group contact.
use crate::common;
use chassis::fx::Fx;
use common::{building, cmd, players, unit};
use oxide_sim::scenario::ScenarioMode;
use oxide_sim::{
    BuildingKind, Command, Event, PlayerId, Scenario, State, Target, UnitId, UnitKind,
};
use std::collections::BTreeSet;

fn swarm(paired: bool) -> Scenario {
    let mut units = Vec::new();
    for i in 0..24 {
        units.push(unit(0, UnitKind::Scuttler, 14 + i % 6, 11 + i / 6));
        if paired {
            units.push(unit(1, UnitKind::Scuttler, 65 - i % 6, 48 - i / 6));
        }
    }
    let mut buildings = vec![building(1, BuildingKind::Foundry, 24, 13)];
    if paired {
        buildings.push(building(0, BuildingKind::Foundry, 54, 45));
    }
    Scenario {
        name: "scuttler-frontage".into(),
        mode: ScenarioMode::Sandbox,
        map: vec![".".repeat(80); 60],
        players: players(0),
        units,
        buildings,
        meta: None,
    }
}

fn attack(state: &State, player: u8, target: usize) -> oxide_sim::PlayerCommand {
    cmd(
        player,
        Command::Attack {
            units: state
                .units()
                .iter()
                .filter(|u| u.player == PlayerId(player))
                .map(|u| u.id)
                .collect(),
            target: Target::Building(state.buildings()[target].id).into(),
            queue: false,
        },
    )
}

fn hits(events: &[Event]) -> BTreeSet<UnitId> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::AttackHit { attacker, .. } => Some(*attacker),
            _ => None,
        })
        .collect()
}

#[test]
fn group_uses_every_face_and_settles_instead_of_pushing_through_attackers() {
    let mut state = swarm(false).build().unwrap();
    state.tick(&[attack(&state, 0, 0)]);
    for _ in 1..150 {
        state.tick(&[]);
    }
    let positions: Vec<_> = state.units().iter().map(|u| u.pos).collect();
    let mut active = BTreeSet::new();
    for _ in 0..30 {
        active.extend(hits(&state.tick(&[]).events));
    }
    let center = state.buildings()[0].center();
    let mut faces = [0; 4];
    for &id in &active {
        let delta = state.unit(id).unwrap().pos - center;
        let face = if delta.x.abs() >= delta.y.abs() {
            usize::from(delta.x > Fx::ZERO)
        } else {
            2 + usize::from(delta.y > Fx::ZERO)
        };
        faces[face] += 1;
    }
    assert!(
        faces.into_iter().all(|count| count >= 2),
        "unoccupied frontage: {faces:?}"
    );
    for (unit, before) in state.units().iter().zip(positions) {
        if active.contains(&unit.id) {
            assert!(
                unit.pos.dist_sq(before) <= Fx::lit("0.0001"),
                "productive attacker moved more than 0.01 tile: {:?}",
                unit.id
            );
            assert!(unit.path.is_none());
        }
    }
    state.validate_invariants().unwrap();
}

#[test]
fn waiting_scuttlers_fill_frontage_released_by_a_move_order() {
    let mut scenario = swarm(false);
    scenario
        .units
        .extend((24..48).map(|i| unit(0, UnitKind::Scuttler, 14 + i % 6, 11 + i / 6)));
    let mut state = scenario.build().unwrap();
    state.tick(&[attack(&state, 0, 0)]);
    for _ in 1..150 {
        state.tick(&[]);
    }
    let mut active = BTreeSet::new();
    for _ in 0..6 {
        active.extend(hits(&state.tick(&[]).events));
    }
    let moved = *active
        .iter()
        .find(|id| state.unit(**id).unwrap().pos.x < Fx::from_num(24))
        .unwrap();
    state.tick(&[cmd(
        0,
        Command::Run {
            units: vec![moved],
            goal: chassis::grid::TilePos::new(18, 20),
            queue: false,
        },
    )]);
    let mut replacements = BTreeSet::new();
    for _ in 0..60 {
        replacements.extend(hits(&state.tick(&[]).events));
    }
    assert!(
        replacements.iter().any(|id| !active.contains(id)),
        "nobody filled the released position"
    );
}

#[test]
fn group_contact_survives_serialized_continuation_on_both_approach_sides() {
    let mut state = swarm(true).build().unwrap();
    state.tick(&[attack(&state, 0, 0), attack(&state, 1, 1)]);
    let mut restored = None;
    for tick in 1..170 {
        state.tick(&[]);
        if tick == 70 {
            restored = Some(
                serde_json::from_slice::<State>(&serde_json::to_vec(&state).unwrap()).unwrap(),
            );
        } else if let Some(other) = &mut restored {
            other.tick(&[]);
            assert_eq!(state.hash(), other.hash(), "continuation drift at {tick}");
        }
        state.validate_invariants().unwrap();
    }
}
