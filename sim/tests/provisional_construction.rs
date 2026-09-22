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
    let mut snapshot = serde_json::to_value(&state).unwrap();
    let tile = (anchor.y * state.map().width() + anchor.x) as usize;
    snapshot["map"]["grid"]["cells"][tile]["wreck"] = serde_json::json!(100);
    state = serde_json::from_value(snapshot).unwrap();
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
    assert_eq!(state.building(id).unwrap().progress, 0);
    assert!(state.map().wreck_at(anchor) > 0);
    assert!(state.player(PlayerId(0)).scrap >= bank);
    assert!(
        crew.iter()
            .all(|id_| state.unit(*id_).unwrap().order == Order::Build { site: id })
    );
    state.validate_invariants().unwrap();
    state.tick(&[cmd(0, Command::Stop { units: crew })]);
    assert!(state.building(id).is_none());
    assert!(state.map().wreck_at(anchor) > 0);
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

#[test]
fn a_provisional_site_cannot_shield_a_physical_building_from_blind_fire() {
    use oxide_sim::{AttackTarget, GhostBuilding, RememberedBuilding};

    let anchor = TilePos::new(15, 8);
    let mut scenario = open_arena_with(
        60,
        24,
        vec![
            unit(0, UnitKind::Breaker, 12, 8),
            unit(1, UnitKind::Harvester, 50, 8),
            unit(2, UnitKind::Sentinel, 15, 9),
        ],
        |rows| rows[1][57] = '3',
    );
    let mut third = scenario.players[0].clone();
    third.name = "Third".into();
    scenario.players.push(third);
    scenario.buildings.extend([
        common::building(1, BuildingKind::Barricade, 15, 8),
        common::building(2, BuildingKind::Barricade, 16, 8),
    ]);
    let state = scenario.build().unwrap();
    let shooter = state.units()[0].id;
    let bystander = state.units()[2].id;
    let provisional = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Barricade && b.player == PlayerId(1))
        .unwrap()
        .id;
    let physical = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Barricade && b.player == PlayerId(2))
        .unwrap()
        .id;
    let mut snapshot = serde_json::to_value(&state).unwrap();
    // One enemy planned unseen ground; another claimed it. The shooter still
    // remembers a removed mine whose owner differs from the physical building.
    for building in snapshot["buildings"].as_array_mut().unwrap() {
        if building["id"] == serde_json::json!(provisional) {
            building["provisional"] = true.into();
            building["built"] = false.into();
            building["hp"] = (BuildingKind::Barricade.base_stats().max_hp / 5).into();
        }
        if building["id"] == serde_json::json!(physical) {
            building["anchor"] = serde_json::to_value(anchor).unwrap();
        }
    }
    snapshot["units"][1]["order"] = serde_json::to_value(Order::Found {
        kind: BuildingKind::Barricade,
        anchor,
    })
    .unwrap();
    for visible in snapshot["vision"][1]["visible"]["cells"]
        .as_array_mut()
        .unwrap()
    {
        *visible = false.into();
    }
    snapshot["vision"][1]["tracking"] = serde_json::json!({"next_id": 0, "tracks": []});
    let ghosts = snapshot["vision"][0]["ghosts"].as_array_mut().unwrap();
    ghosts.retain(|ghost| ghost["anchor"] != serde_json::to_value(anchor).unwrap());
    ghosts.push(
        serde_json::to_value(GhostBuilding {
            kind: BuildingKind::ScuttleCharge,
            owner: PlayerId(1),
            anchor,
            hp: BuildingKind::ScuttleCharge.base_stats().max_hp,
            built: true,
        })
        .unwrap(),
    );
    ghosts.sort_by_key(|ghost| {
        (
            ghost["anchor"]["y"].as_i64(),
            ghost["anchor"]["x"].as_i64(),
            ghost["owner"].as_u64(),
        )
    });
    for include_provisional in [false, true] {
        let mut snapshot = snapshot.clone();
        if !include_provisional {
            snapshot["buildings"]
                .as_array_mut()
                .unwrap()
                .retain(|b| b["id"] != serde_json::json!(provisional));
            snapshot["units"][1]["order"] = serde_json::to_value(Order::Idle).unwrap();
        }
        let mut state: State = serde_json::from_value(snapshot).unwrap();
        state.validate_invariants().unwrap();
        face_toward(&mut state, shooter, anchor.center());
        let target = AttackTarget::RememberedBuilding(RememberedBuilding {
            owner: PlayerId(1),
            building_kind: BuildingKind::ScuttleCharge,
            anchor,
        });
        assert!(
            state
                .attack_view(PlayerId(0), target)
                .unwrap()
                .entity
                .is_none()
        );
        let hp = state.building(physical).unwrap().hp;
        let report = state.tick(&[cmd(
            0,
            Command::Attack {
                units: vec![shooter],
                target,
                queue: false,
            },
        )]);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. }))
        );
        assert_eq!(
            state.building(physical).unwrap().hp,
            hp - UnitKind::Breaker.stats().weapons[0].damage
        );
        assert!(
            state.unit(bystander).is_none(),
            "the nearby unit still takes splash"
        );
        if include_provisional {
            let site = state.building(provisional).unwrap();
            assert!(site.provisional);
            assert_eq!(site.hp, site.stats().max_hp / 5);
        }
        state.validate_invariants().unwrap();
    }
}
