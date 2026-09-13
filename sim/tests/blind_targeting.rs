//! Player-knowledge targeting contracts, through ordinary commands and ticks.

mod common;
use chassis::grid::TilePos;
use common::*;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::{AttackTarget, BuildingKind, Command, Event, Order, PlayerId, Target, UnitKind};

fn radar_scene(
    gun: Option<UnitKind>,
    defense: Option<BuildingKind>,
    enemy: UnitKind,
) -> oxide_sim::State {
    let mut units = vec![unit(1, enemy, 14, 8)];
    if let Some(kind) = gun {
        units.push(unit(0, kind, 5, 7));
    }
    let mut scenario = open_arena(40, 30, units);
    scenario.buildings = vec![BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 5,
        y: 16,
    }];
    if let Some(kind) = defense {
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind,
            x: 5,
            y: 6,
        });
    }
    let state = scenario.build().unwrap();
    assert!(!state.can_see(PlayerId(0), TilePos::new(14, 8)));
    assert!(
        state
            .vision(PlayerId(0))
            .contacts()
            .contains(&TilePos::new(14, 8))
    );
    state
}

fn blip(state: &oxide_sim::State) -> AttackTarget {
    AttackTarget::Contact(
        state
            .vision(PlayerId(0))
            .tracks()
            .iter()
            .find(|track| track.visible_unit.is_none())
            .unwrap()
            .id,
    )
}

#[test]
fn bastion_expends_a_blind_shot_on_an_air_contact() {
    let mut state = radar_scene(None, Some(BuildingKind::Bastion), UnitKind::Gnat);
    let enemy = state.units()[0].id;
    let hp = state.unit(enemy).unwrap().hp;
    let report = state.tick(&[]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::ShellLaunched { target: None, .. }))
    );
    for _ in 0..60 {
        state.tick(&[]);
        state.validate_invariants().unwrap();
    }
    assert_eq!(state.unit(enemy).unwrap().hp, hp);
}

#[test]
fn explicit_contact_is_retained_and_stop_clears_defense_focus() {
    let mut state = radar_scene(None, Some(BuildingKind::Bastion), UnitKind::Gnat);
    let id = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let target = blip(&state);
    state.tick(&[cmd(
        0,
        Command::FocusFire {
            buildings: vec![id, id],
            target,
        },
    )]);
    assert_eq!(state.building(id).unwrap().focus, Some(target));
    state.tick(&[cmd(
        0,
        Command::ClearFocus {
            buildings: vec![id, id],
        },
    )]);
    assert!(state.building(id).unwrap().focus.is_none());
}

#[test]
fn automatic_avalanche_fires_without_pursuing_but_explicit_attack_pursues() {
    let mut state = radar_scene(Some(UnitKind::Avalanche), None, UnitKind::Gnat);
    let gun = state
        .units()
        .iter()
        .find(|u| u.player == PlayerId(0))
        .unwrap()
        .id;
    let start = state.unit(gun).unwrap().pos;
    let mut fired = false;
    for _ in 0..100 {
        fired |= state
            .tick(&[])
            .events
            .iter()
            .any(|e| matches!(e, Event::ShellLaunched { target: None, .. }));
    }
    assert!(fired);
    assert_eq!(state.unit(gun).unwrap().pos, start);
    assert_eq!(state.unit(gun).unwrap().order, Order::Idle);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![gun],
            goal: TilePos::new(2, 27),
            queue: false,
        },
    )]);
    for _ in 0..700 {
        state.tick(&[]);
    }
    let target = blip(&state);
    let start = state.unit(gun).unwrap().pos;
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![gun],
            target,
            queue: false,
        },
    )]);
    for _ in 0..80 {
        state.tick(&[]);
    }
    assert_ne!(state.unit(gun).unwrap().pos, start);
}

#[test]
fn lost_contact_clears_preference_and_returning_detection_has_a_new_id() {
    let mut state = radar_scene(None, Some(BuildingKind::Bastion), UnitKind::Gnat);
    let gun = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let enemy = state.units()[0].id;
    let old = blip(&state);
    state.tick(&[
        cmd(
            0,
            Command::FocusFire {
                buildings: vec![gun],
                target: old,
            },
        ),
        cmd(
            1,
            Command::Move {
                units: vec![enemy],
                goal: TilePos::new(35, 8),
                queue: false,
            },
        ),
    ]);
    for _ in 0..300 {
        state.tick(&[]);
    }
    assert!(state.building(gun).unwrap().focus.is_none());
    assert!(state.attack_view(PlayerId(0), old).is_none());
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![enemy],
            goal: TilePos::new(14, 8),
            queue: false,
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
    }
    assert_ne!(blip(&state), old);
    assert!(state.building(gun).unwrap().focus.is_none());
}

#[test]
fn visible_attack_becomes_anonymous_when_only_radar_remains() {
    let mut scenario = open_arena(
        40,
        30,
        vec![
            unit(0, UnitKind::Harvester, 12, 8),
            unit(1, UnitKind::Gnat, 14, 8),
        ],
    );
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 5,
            y: 16,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 5,
            y: 6,
        },
    ];
    let mut state = scenario.build().unwrap();
    let scout = state.units()[0].id;
    let enemy = state.units()[1].id;
    let track = state
        .vision(PlayerId(0))
        .tracks()
        .iter()
        .find(|t| t.visible_unit == Some(enemy))
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(2, 8),
            queue: false,
        },
    )]);
    for _ in 0..120 {
        state.tick(&[]);
    }
    let contact = state.vision(PlayerId(0)).track(track).unwrap();
    assert_eq!(contact.visible_unit, None);
    state.validate_invariants().unwrap();
}

#[test]
fn unknown_contact_command_is_hash_inert_at_dispatch() {
    let state = radar_scene(Some(UnitKind::Avalanche), None, UnitKind::Gnat);
    let gun = state
        .units()
        .iter()
        .find(|u| u.player == PlayerId(0))
        .unwrap()
        .id;
    let commands = [cmd(
        0,
        Command::Attack {
            units: vec![gun],
            target: AttackTarget::Contact(oxide_sim::ContactId(u32::MAX)),
            queue: false,
        },
    )];
    state.inspect_command_phase(&commands, |view| {
        assert_eq!(
            view.unit(gun).unwrap().order,
            state.unit(gun).unwrap().order
        );
    });
}

#[test]
fn contact_histories_round_trip_and_replay_deterministically() {
    let mut state = radar_scene(Some(UnitKind::Avalanche), None, UnitKind::Gnat);
    let enemy = state.units()[0].id;
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![enemy],
            goal: TilePos::new(18, 8),
            queue: false,
        },
    )]);
    for _ in 0..20 {
        state.tick(&[]);
    }
    let mut replay: oxide_sim::State =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    for _ in 0..100 {
        assert_eq!(state.tick(&[]), replay.tick(&[]));
        assert_eq!(state.hash(), replay.hash());
    }
    assert!(
        state
            .vision(PlayerId(0))
            .tracks()
            .iter()
            .all(|t| t.history.len() <= 21)
    );
    assert!(
        state
            .attack_view(PlayerId(0), Target::Unit(enemy).into())
            .is_none()
    );
}

#[test]
fn remembered_building_keeps_taking_fire_until_scouting_confirms_its_loss() {
    let mut scenario = open_arena(
        40,
        30,
        vec![
            unit(0, UnitKind::Harvester, 12, 8),
            unit(0, UnitKind::Avalanche, 5, 8),
        ],
    );
    scenario.buildings = vec![BuildingSpec {
        player: 1,
        kind: BuildingKind::Reclaimer,
        x: 14,
        y: 8,
    }];
    let mut state = scenario.build().unwrap();
    let scout = state.units()[0].id;
    let gun = state.units()[1].id;
    let building = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Reclaimer)
        .unwrap()
        .id;
    let memory = state
        .attack_objective(PlayerId(0), Target::Building(building).into())
        .unwrap();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(2, 3),
            queue: false,
        },
    )]);
    for _ in 0..100 {
        state.tick(&[]);
    }
    assert!(!state.can_see(PlayerId(0), TilePos::new(14, 8)));
    let start = state.unit(gun).unwrap().pos;
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![gun],
            target: memory,
            queue: false,
        },
    )]);
    let mut blind_shots = 0;
    for _ in 0..600 {
        blind_shots += state
            .tick(&[])
            .events
            .iter()
            .filter(|e| matches!(e, Event::ShellLaunched { target: None, .. }))
            .count();
        state.validate_invariants().unwrap();
    }
    assert!(blind_shots >= 3);
    assert!(state.building(building).is_none());
    assert_eq!(state.unit(gun).unwrap().pos, start);
    assert!(matches!(state.unit(gun).unwrap().order,Order::Attack{target,..} if target==memory));
    assert!(state.attack_view(PlayerId(0), memory).is_some());
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(12, 3),
            queue: false,
        },
    )]);
    for _ in 0..130 {
        state.tick(&[]);
    }
    assert!(state.attack_view(PlayerId(0), memory).is_none());
    assert!(!matches!(state.unit(gun).unwrap().order,Order::Attack{target,..} if target==memory));
}

#[test]
fn fabricated_and_corrupted_contact_histories_are_rejected() {
    let state = radar_scene(None, Some(BuildingKind::Bastion), UnitKind::Gnat);
    let base = serde_json::to_value(state).unwrap();
    for field in ["id", "tile", "visible_unit", "history"] {
        let mut data = base.clone();
        let track = &mut data["vision"][0]["tracking"]["tracks"][0];
        match field {
            "id" => track[field] = serde_json::json!(u32::MAX),
            "tile" => track[field] = serde_json::json!({"x":i32::MAX,"y":0}),
            "visible_unit" => track[field] = serde_json::json!(0),
            "history" => track[field] = serde_json::json!([]),
            _ => unreachable!(),
        }
        assert!(
            serde_json::from_value::<oxide_sim::State>(data).is_err(),
            "{field}"
        );
    }
}

#[test]
fn every_ground_combat_chassis_can_act_on_a_building_memory() {
    for kind in UnitKind::ALL {
        if !kind.stats().can_target(oxide_sim::stats::Domain::Ground) && !kind.stats().demolition {
            continue;
        }
        let mut scenario = open_arena(
            45,
            30,
            vec![unit(0, UnitKind::Harvester, 17, 8), unit(0, kind, 3, 16)],
        );
        scenario.buildings = vec![BuildingSpec {
            player: 1,
            kind: BuildingKind::Reclaimer,
            x: 19,
            y: 8,
        }];
        let mut state = scenario.build().unwrap();
        let scout = state.units()[0].id;
        let gun = state.units()[1].id;
        let building = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Reclaimer)
            .unwrap()
            .id;
        let target = state
            .attack_objective(PlayerId(0), Target::Building(building).into())
            .unwrap();
        state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![scout],
                goal: TilePos::new(2, 3),
                queue: false,
            },
        )]);
        for _ in 0..250 {
            state.tick(&[]);
        }
        assert!(!state.can_see(PlayerId(0), TilePos::new(19, 8)), "{kind:?}");
        let hp = state.building(building).unwrap().hp;
        let report = state.tick(&[cmd(
            0,
            Command::Attack {
                units: vec![gun],
                target,
                queue: false,
            },
        )]);
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. })),
            "{kind:?}"
        );
        for _ in 0..1400 {
            if state.building(building).is_none_or(|b| b.hp < hp) {
                break;
            }
            state.tick(&[]);
            state.validate_invariants().unwrap();
        }
        assert!(
            state.building(building).is_none_or(|b| b.hp < hp),
            "{kind:?} never reached and attacked memory: {:?}",
            state.unit(gun)
        );
    }
}

#[test]
fn concealed_identity_domain_health_and_position_do_not_change_firing_decisions() {
    let base = radar_scene(None, Some(BuildingKind::Bastion), UnitKind::Gnat);
    let mut air = base.clone();
    let target = blip(&base);
    let gun = base
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let command = cmd(
        0,
        Command::FocusFire {
            buildings: vec![gun],
            target,
        },
    );
    let a = air.tick(std::slice::from_ref(&command));
    let launches = |report: oxide_sim::TickReport| {
        report
            .events
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::ShellLaunched { .. }
                        | Event::AttackHit { .. }
                        | Event::TurretFired { .. }
                        | Event::CommandRejected { .. }
                )
            })
            .collect::<Vec<_>>()
    };
    let expected = launches(a);
    for variant in 0..4 {
        let mut document = serde_json::to_value(&base).unwrap();
        match variant {
            0 => document["units"][0]["kind"] = serde_json::json!("harvester"),
            1 => {
                document["units"][0]["id"] = serde_json::json!(17);
                document["next_unit_id"] = serde_json::json!(18);
            }
            2 => document["units"][0]["hp"] = serde_json::json!(1),
            _ => {
                document["units"][0]["pos"] = serde_json::to_value(chassis::fx::Vec2Fx::new(
                    chassis::fx::Fx::lit("14.2"),
                    chassis::fx::Fx::lit("8.2"),
                ))
                .unwrap();
            }
        }
        let mut concealed: oxide_sim::State = serde_json::from_value(document).unwrap();
        concealed.validate_invariants().unwrap();
        assert_eq!(
            concealed.vision(PlayerId(0)).tracks(),
            base.vision(PlayerId(0)).tracks()
        );
        let report = concealed.tick(std::slice::from_ref(&command));
        assert_eq!(expected, launches(report), "concealed variant {variant}");
        assert_eq!(
            air.building(gun).unwrap().focus,
            concealed.building(gun).unwrap().focus
        );
        assert_eq!(
            air.building(gun).unwrap().cooldown,
            concealed.building(gun).unwrap().cooldown
        );
        concealed.validate_invariants().unwrap();
    }
}

#[test]
fn expired_queued_contact_cannot_bind_to_its_reappearance() {
    let mut state = radar_scene(Some(UnitKind::Avalanche), None, UnitKind::Gnat);
    let enemy = state.units()[0].id;
    let gun = state.units()[1].id;
    let old = blip(&state);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![gun],
                goal: TilePos::new(2, 26),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Attack {
                units: vec![gun],
                target: old,
                queue: true,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![gun],
                goal: TilePos::new(4, 26),
                queue: true,
            },
        ),
        cmd(
            1,
            Command::Move {
                units: vec![enemy],
                goal: TilePos::new(35, 8),
                queue: false,
            },
        ),
    ]);
    for _ in 0..150 {
        state.tick(&[]);
    }
    assert!(state.attack_view(PlayerId(0), old).is_none());
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![enemy],
            goal: TilePos::new(14, 8),
            queue: false,
        },
    )]);
    for _ in 0..1000 {
        state.tick(&[]);
        state.validate_invariants().unwrap();
        assert!(!matches!(
            state.unit(gun).unwrap().order,
            Order::Attack { .. }
        ));
    }
    assert_ne!(blip(&state), old);
    assert_eq!(state.unit(gun).unwrap().tile(), TilePos::new(4, 26));
    assert!(state.unit(gun).unwrap().queue.is_empty());
}

#[test]
fn removing_the_array_ends_its_contacts_and_defense_preference() {
    let mut scenario = open_arena(
        40,
        30,
        vec![
            unit(0, UnitKind::Harvester, 4, 16),
            unit(1, UnitKind::Gnat, 14, 8),
        ],
    );
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 5,
            y: 16,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 5,
            y: 6,
        },
    ];
    let mut state = scenario.build().unwrap();
    let worker = state.units()[0].id;
    let array = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Array)
        .unwrap()
        .id;
    let defense = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let contact = blip(&state);
    state.tick(&[
        cmd(
            0,
            Command::FocusFire {
                buildings: vec![defense],
                target: contact,
            },
        ),
        cmd(
            0,
            Command::Salvage {
                units: vec![worker],
                building: array,
                queue: false,
            },
        ),
    ]);
    for _ in 0..600 {
        if state.building(array).is_none() {
            break;
        }
        state.tick(&[]);
    }
    assert!(state.building(array).is_none());
    assert!(state.attack_view(PlayerId(0), contact).is_none());
    assert!(state.building(defense).unwrap().focus.is_none());
}

#[test]
fn explicit_radar_attack_routes_to_a_firing_stand_beside_impassable_ground() {
    let mut scenario = open_arena_with(
        40,
        30,
        vec![
            unit(0, UnitKind::Avalanche, 2, 27),
            unit(1, UnitKind::Gnat, 14, 8),
        ],
        |rows| rows[8][14] = '#',
    );
    scenario.buildings = vec![BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 5,
        y: 16,
    }];
    let mut state = scenario.build().unwrap();
    let gun = state.units()[0].id;
    let target = blip(&state);
    let before = state.unit(gun).unwrap().pos;
    for _ in 0..60 {
        state.tick(&[]);
    }
    assert_eq!(state.unit(gun).unwrap().pos, before);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![gun],
            target,
            queue: false,
        },
    )]);
    let mut fired = false;
    for _ in 0..800 {
        let report = state.tick(&[]);
        state.validate_invariants().unwrap();
        if report.events.iter().any(|e| matches!(e, Event::ShellLaunched { shooter: Target::Unit(id), target: None, .. } if *id == gun)) {
            fired = true;
            break;
        }
    }
    assert!(
        fired,
        "a reachable firing stand must work even when the contact's tile is impassable"
    );
    assert_ne!(state.unit(gun).unwrap().pos, before);
}

#[test]
fn unreachable_blind_attacks_stall_once_and_clear_the_program() {
    for kind in [UnitKind::Avalanche, UnitKind::Sapper] {
        for remembered in [false, true] {
            let mut scenario = open_arena(40, 30, vec![unit(0, kind, 5, 10)]);
            let mut rows: Vec<Vec<char>> = scenario
                .map
                .iter()
                .map(|row| row.chars().collect())
                .collect();
            rows[27][37] = '.';
            rows[27][1] = '2';
            for row in &mut rows {
                row[16] = '^';
            }
            scenario.map = rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect();
            scenario.buildings = vec![BuildingSpec {
                player: 0,
                kind: BuildingKind::Array,
                x: 10,
                y: 18,
            }];
            if remembered {
                scenario.units.push(unit(0, UnitKind::Harvester, 22, 13));
                scenario.buildings.push(BuildingSpec {
                    player: 1,
                    kind: BuildingKind::Reclaimer,
                    x: 22,
                    y: 10,
                });
            } else {
                scenario.units.push(unit(1, UnitKind::Gnat, 22, 10));
            }
            let mut state = scenario.build().unwrap();
            let gun = state.units()[0].id;
            let target = if remembered {
                let building = state
                    .buildings()
                    .iter()
                    .find(|b| b.kind == BuildingKind::Reclaimer)
                    .unwrap()
                    .id;
                let target = state
                    .attack_objective(PlayerId(0), Target::Building(building).into())
                    .unwrap();
                let scout = state.units()[1].id;
                state.tick(&[cmd(
                    0,
                    Command::Move {
                        units: vec![scout],
                        goal: TilePos::new(34, 25),
                        queue: false,
                    },
                )]);
                for _ in 0..250 {
                    state.tick(&[]);
                }
                assert!(
                    state
                        .attack_view(PlayerId(0), target)
                        .unwrap()
                        .entity
                        .is_none()
                );
                target
            } else {
                blip(&state)
            };
            let start = state.unit(gun).unwrap().pos;
            let report = state.tick(&[
                cmd(
                    0,
                    Command::Attack {
                        units: vec![gun],
                        target,
                        queue: false,
                    },
                ),
                cmd(
                    0,
                    Command::Move {
                        units: vec![gun],
                        goal: TilePos::new(4, 10),
                        queue: true,
                    },
                ),
            ]);
            assert!(report.events.iter().any(|event| matches!(event, Event::OrderStalled { unit, reason: oxide_sim::event::StallReason::NoRoute, .. } if *unit == gun)), "{kind:?} remembered={remembered}: {:?}", report.events);
            for _ in 0..20 {
                assert!(!state.tick(&[]).events.iter().any(
                    |event| matches!(event, Event::OrderStalled { unit, .. } if *unit == gun)
                ));
                state.validate_invariants().unwrap();
            }
            let unit = state.unit(gun).unwrap();
            assert_eq!(unit.order, Order::Idle);
            assert!(unit.path.is_none());
            assert!(unit.queue.is_empty());
            assert_eq!(unit.pos, start);
        }
    }
}

#[test]
fn legacy_focus_initializes_tracks_without_losing_its_preference() {
    let mut scenario = open_arena(32, 24, vec![unit(1, UnitKind::Harvester, 12, 10)]);
    scenario.players[0].team = Some(0);
    scenario.players[1].team = Some(1);
    let ally = scenario.players[0].clone();
    scenario.players.push(ally);
    scenario.map[20].replace_range(1..2, "3");
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 8,
            y: 10,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::Reclaimer,
            x: 12,
            y: 12,
        },
    ];
    let state = scenario.build().unwrap();
    let defense = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let building = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Reclaimer)
        .unwrap()
        .id;
    for target in [
        Target::Unit(state.units()[0].id),
        Target::Building(building),
    ] {
        let mut data = serde_json::to_value(&state).unwrap();
        for view in data["vision"].as_array_mut().unwrap() {
            view.as_object_mut().unwrap().remove("tracking");
        }
        let row = data["buildings"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|b| b["id"] == serde_json::json!(defense))
            .unwrap();
        row["focus"] = serde_json::to_value(target).unwrap();
        let mut loaded: oxide_sim::State = serde_json::from_value(data).unwrap();
        loaded.validate_invariants().unwrap();
        assert_eq!(
            loaded.vision(PlayerId(0)).tracks(),
            loaded.vision(PlayerId(2)).tracks()
        );
        assert_eq!(
            loaded
                .attack_view(PlayerId(0), target.into())
                .unwrap()
                .entity,
            Some(target)
        );
        let round_trip: oxide_sim::State =
            serde_json::from_value(serde_json::to_value(&loaded).unwrap()).unwrap();
        assert_eq!(loaded.hash(), round_trip.hash());
        let report = loaded.tick(&[]);
        assert!(loaded.building(defense).unwrap().focus.is_some());
        assert!(report.events.iter().any(|event| matches!(event, Event::ShellLaunched { shooter: Target::Building(id), target: Some(victim), .. } if *id == defense && *victim == target)));
    }
}

#[test]
fn legacy_focus_still_rejects_friendly_hidden_and_incompatible_units() {
    for case in ["friendly", "hidden", "air"] {
        let enemy_kind = if case == "air" {
            UnitKind::Gnat
        } else {
            UnitKind::Harvester
        };
        let mut scenario = open_arena(
            32,
            24,
            vec![unit(
                if case == "friendly" { 0 } else { 1 },
                enemy_kind,
                if case == "hidden" { 24 } else { 12 },
                10,
            )],
        );
        scenario.buildings = vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 8,
            y: 10,
        }];
        let state = scenario.build().unwrap();
        let defense = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Bastion)
            .unwrap()
            .id;
        let mut data = serde_json::to_value(&state).unwrap();
        for view in data["vision"].as_array_mut().unwrap() {
            view.as_object_mut().unwrap().remove("tracking");
        }
        data["buildings"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|b| b["id"] == serde_json::json!(defense))
            .unwrap()["focus"] = serde_json::to_value(Target::Unit(state.units()[0].id)).unwrap();
        let error = serde_json::from_value::<oxide_sim::State>(data).unwrap_err();
        assert!(
            error.to_string().contains("invalid defense focus"),
            "{case}: {error}"
        );
    }
}
