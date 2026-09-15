//! Payment and physical occupancy of construction blueprints.

mod common;

use chassis::grid::TilePos;
use common::*;
use oxide_sim::{BuildingKind, Command, Event, Order, PlayerId, State, UnitId, UnitKind};

fn fixture() -> (State, Vec<UnitId>, TilePos) {
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 12, 2),
        unit(0, UnitKind::Harvester, 12, 3),
    ]);
    scenario.players[0].scrap = 100;
    let mut state = scenario.build().unwrap();
    let crew = state.units().iter().map(|u| u.id).collect::<Vec<_>>();
    let anchor = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: crew.clone(),
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), anchor));
    assert!(state.vision(PlayerId(0)).explored(anchor));
    (state, crew, anchor)
}

fn plan(crew: Vec<UnitId>, anchor: TilePos, queue: bool) -> Command {
    Command::Build {
        units: crew,
        kind: BuildingKind::Turret,
        anchor,
        queue,
        defer: true,
    }
}

#[test]
fn one_paid_nonphysical_site_survives_until_the_final_worker_abandons_it() {
    let (mut state, crew, anchor) = fixture();
    let bank = state.player(PlayerId(0)).scrap;
    let cost = BuildingKind::Turret.base_stats().construction.unwrap().cost;
    let mut duplicate_crew = crew.clone();
    duplicate_crew.extend(&crew);
    let report = state.tick(&[cmd(0, plan(duplicate_crew, anchor, true))]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. }))
    );
    let sites: Vec<_> = state
        .buildings()
        .iter()
        .filter(|b| b.anchor == anchor)
        .collect();
    assert_eq!(sites.len(), 1);
    let id = sites[0].id;
    assert!(sites[0].provisional);
    assert!(!state.can_see(PlayerId(0), anchor));
    assert!(state.passable(anchor));
    assert!(!state.building_apparent(PlayerId(1), sites[0]));
    assert_eq!(state.player(PlayerId(0)).scrap, bank - cost);

    let snapshot = serde_json::to_string(&state).unwrap();
    let mut restored: State = serde_json::from_str(&snapshot).unwrap();
    let stop = cmd(
        0,
        Command::Stop {
            units: vec![crew[0]],
        },
    );
    state.tick(std::slice::from_ref(&stop));
    restored.tick(&[stop]);
    assert_eq!(state, restored);
    assert!(state.building(id).is_some());
    assert_eq!(state.player(PlayerId(0)).scrap, bank - cost);
    let report = state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![crew[1], crew[1]],
        },
    )]);
    assert!(state.building(id).is_none());
    assert_eq!(state.player(PlayerId(0)).scrap, bank);
    assert_eq!(
        report
            .events
            .iter()
            .filter(|e| matches!(e, Event::BuildCancelled { refund, .. } if *refund == cost))
            .count(),
        1
    );
}

#[test]
fn replacement_spends_the_refund_and_rejection_preserves_the_paid_program() {
    let (mut state, crew, anchor) = fixture();
    state.tick(&[cmd(0, plan(crew.clone(), anchor, true))]);
    let before = state.clone();
    let rejected = cmd(0, plan(crew.clone(), TilePos::new(-1, -1), false));
    state.inspect_command_phase(&[rejected], |view| {
        assert_eq!(
            view.scrap(PlayerId(0)),
            Some(before.player(PlayerId(0)).scrap)
        );
        assert_eq!(view.units(), before.units());
    });
    let bank = state.player(PlayerId(0)).scrap;
    let next = TilePos::new(13, 1);
    let report = state.tick(&[cmd(0, plan(crew, next, false))]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. }))
    );
    assert_eq!(state.player(PlayerId(0)).scrap, bank);
    assert!(state.buildings().iter().all(|b| b.anchor != anchor));
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.anchor == next && b.provisional)
    );
}

#[test]
fn visibility_activates_the_same_site_without_another_payment() {
    let (mut state, crew, anchor) = fixture();
    state.tick(&[cmd(0, plan(crew.clone(), anchor, false))]);
    let id = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    let bank = state.player(PlayerId(0)).scrap;
    run_until(&mut state, 600, |s, _| {
        s.building(id).is_some_and(|b| !b.provisional)
    });
    assert!(!state.passable(anchor));
    assert!(state.player(PlayerId(0)).scrap >= bank);
    assert!(
        crew.iter()
            .all(|id_| state.unit(*id_).unwrap().order == Order::Build { site: id })
    );
    state.validate_invariants().unwrap();
}

#[test]
fn forged_provisional_scaffolds_cannot_hold_progress_or_physical_state() {
    let (mut state, crew, anchor) = fixture();
    state.tick(&[cmd(0, plan(crew, anchor, true))]);
    let clean = serde_json::to_value(&state).unwrap();
    let i = clean["buildings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|b| b["provisional"] == true)
        .unwrap();
    for (key, value) in [
        ("built", serde_json::json!(true)),
        ("progress", serde_json::json!(1)),
        ("hp", serde_json::json!(1)),
    ] {
        let mut forged = clean.clone();
        forged["buildings"][i][key] = value;
        assert!(
            serde_json::from_value::<State>(forged)
                .unwrap_err()
                .to_string()
                .contains("invalid provisional state")
        );
    }
}
