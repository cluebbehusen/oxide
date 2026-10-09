use super::*;
use crate::game::Game;
use oxide_sim::{BuildingId, BuildingKind, Target, UnitId, UnitKind};

#[test]
fn lethal_scuttler_bite_retains_the_visible_unit_surface() {
    let scenario = serde_json::from_value(serde_json::json!({
        "name": "Lethal bite", "mode": "sandbox", "map": vec![".............................."; 22],
        "players": [
            {"name": "Local", "faction": "ferrous", "scrap": 0, "bot": false},
            {"name": "Target", "faction": "cupric", "scrap": 0, "bot": false}
        ],
        "units": [
            {"player": 0, "kind": "scuttler", "x": 11, "y": 7},
            {"player": 1, "kind": "harvester", "x": 12, "y": 7}
        ]
    }))
    .unwrap();
    let mut game = Game::with_viewport(scenario, Vec2::new(1280., 800.)).unwrap();
    let mut wire = serde_json::to_value(&*game.state).unwrap();
    wire["units"][1]["hp"] = serde_json::json!(1);
    game.state.0 = std::sync::Arc::new(serde_json::from_value(wire).unwrap());
    game.issue(oxide_sim::Command::Attack {
        units: vec![UnitId(0)],
        target: Target::Unit(UnitId(1)).into(),
        queue: false,
    });
    for _ in 0..40 {
        game.do_tick();
        if game.state.unit(UnitId(1)).is_none() {
            break;
        }
    }
    assert!(game.state.unit(UnitId(1)).is_none());
    assert!(game.presentation.fx.iter().any(|effect| matches!(
        effect.kind,
        EffectKind::DirectShot {
            style: ShotStyle::Contact,
            surface: Some(HitSurface::Unit(UnitHit {
                body: UnitBody {
                    kind: UnitKind::Harvester,
                    ..
                },
                ..
            })),
            ..
        }
    )));
}

#[test]
fn building_reports_keep_surface_facts_through_the_lethal_tick() {
    for lethal in [false, true] {
        let scenario = serde_json::from_value(serde_json::json!({
            "name": "Building strike", "mode": "sandbox", "map": vec![".............................."; 22],
            "players": [
                {"name": "Local", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "Target", "faction": "cupric", "scrap": 0, "bot": false}
            ],
            "units": [{"player": 0, "kind": "sentinel", "x": 11, "y": 7}],
            "buildings": [{"player": 1, "kind": "fabricator", "x": 13, "y": 9}]
        }))
        .unwrap();
        let mut game = Game::with_viewport(scenario, Vec2::new(1280., 800.)).unwrap();
        if lethal {
            let mut wire = serde_json::to_value(&*game.state).unwrap();
            wire["buildings"][0]["hp"] = serde_json::json!(1);
            game.state.0 = std::sync::Arc::new(serde_json::from_value(wire).unwrap());
        }
        let mut reference = (*game.state).clone();
        let command = oxide_sim::Command::Attack {
            units: vec![UnitId(0)],
            target: Target::Building(BuildingId(0)).into(),
            queue: false,
        };
        game.issue(command.clone());
        let expected = reference.tick(&[oxide_sim::PlayerCommand {
            player: game.presentation.human,
            command,
        }]);
        let report = game.do_tick();
        assert_eq!(game.state.hash(), reference.hash());
        assert_eq!(report.events, expected.events);
        assert_eq!(game.state.building(BuildingId(0)).is_none(), lethal);
        let hit = game
            .presentation
            .fx
            .iter()
            .find_map(|effect| match effect.kind {
                EffectKind::DirectShot {
                    surface: Some(HitSurface::Building(hit)),
                    ..
                } => Some(hit),
                _ => None,
            })
            .expect("ordinary and lethal hits both retain the building outline");
        assert_eq!(hit.kind, BuildingKind::Fabricator);
        assert_eq!(hit.anchor, Vec2::new(13., 9.));
        assert_eq!(hit.faction, oxide_sim::Faction::Cupric);
        assert!(
            game.presentation
                .building_hit(&game.state, Some(Target::Unit(UnitId(0))))
                .is_none()
        );
    }
}

#[test]
fn unseen_buildings_do_not_supply_surface_facts() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.units.clear();
    for player in &mut scenario.players {
        player.bot_config = None;
    }
    let game = Game::with_viewport(scenario, Vec2::new(1280., 800.)).unwrap();
    let hidden = game
        .state
        .buildings()
        .iter()
        .find(|building| building.player != game.presentation.human)
        .unwrap();
    assert!(
        game.presentation
            .building_hit(&game.state, Some(Target::Building(hidden.id)))
            .is_none()
    );
    assert!(
        game.presentation
            .payload_surface(
                &game.state,
                None,
                game.presentation.human,
                world_vec(hidden.center()),
                oxide_sim::stats::DomainMask::GROUND,
            )
            .is_none()
    );
}

fn blind_bulwark_scene(victim: UnitKind) -> Game {
    let mut scenario = oxide_sim::Scenario::skirmish();
    let mut rows = vec![vec!['.'; 40]; 30];
    rows[27][1] = '1';
    rows[27][37] = '2';
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    for player in &mut scenario.players {
        player.bot_config = None;
    }
    scenario.units = vec![oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: victim,
        x: 12,
        y: 8,
    }];
    scenario.buildings = vec![
        oxide_sim::scenario::BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x: 5,
            y: 6,
        },
        oxide_sim::scenario::BuildingSpec {
            player: 1,
            kind: BuildingKind::Array,
            x: 5,
            y: 16,
        },
    ];
    let mut game = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
    let mut wire = serde_json::to_value(&*game.state).unwrap();
    for building in wire["buildings"].as_array_mut().unwrap() {
        if building["kind"] == "turret" {
            building["tier"] = serde_json::json!(2);
        }
    }
    let state: oxide_sim::State = serde_json::from_value(wire).unwrap();
    state.validate_invariants().unwrap();
    let turret = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .unwrap();
    let range = turret.stats().weapons[0].range;
    assert!(
        state
            .buildings()
            .iter()
            .filter(|building| state.hostile(turret.player, building.player))
            .all(|building| {
                turret
                    .center()
                    .dist_sq(building.closest_point_to(turret.center()))
                    > range * range
            }),
        "visible buildings must not preempt the blind-fire fixture's radar target"
    );
    game.replace_state_after_jump(&state);
    assert!(
        state
            .vision(oxide_sim::PlayerId(1))
            .tracks()
            .iter()
            .any(|track| track.visible_unit.is_none())
    );
    game
}

#[test]
fn blind_hitscan_damage_alerts_only_its_owner_in_live_and_playback() {
    let mut live = blind_bulwark_scene(UnitKind::Excavator);
    let victim = live.state.units()[0].id;
    let before = live.state.unit(victim).unwrap().hp;
    let report = live.do_tick();
    assert_eq!(live.state.unit(victim).unwrap().hp, before - 60);
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::TurretFired { target: None, .. }))
    );
    assert_eq!(live.presentation.alerts.len(), 1);
    assert_eq!(
        live.presentation
            .sounds_pending
            .iter()
            .filter(|(sound, _)| *sound == SoundKind::Alert)
            .count(),
        1
    );
    for human in [0, 1] {
        let mut playback = blind_bulwark_scene(UnitKind::Excavator);
        playback.presentation.human = oxide_sim::PlayerId(human);
        playback
            .presentation
            .remember_previous_tick(&playback.state);
        playback
            .presentation
            .observe_tick(&live.state, &report.events, &report.movement);
        assert_eq!(playback.presentation.alerts.len(), usize::from(human == 0));
        assert_eq!(playback.presentation.last_alert.is_some(), human == 0);
        assert!(playback.presentation.aim_building_targets.is_empty());
        assert!(playback.presentation.aim_unit_targets.is_empty());
    }
}

#[test]
fn blind_hitscan_miss_does_not_raise_an_attack_alert() {
    let mut game = blind_bulwark_scene(UnitKind::Gnat);
    let victim = game.state.units()[0].id;
    let hp = game.state.unit(victim).unwrap().hp;
    let report = game.do_tick();
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::TurretFired { target: None, .. }))
    );
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::DamageTaken { .. }))
    );
    assert_eq!(game.state.unit(victim).unwrap().hp, hp);
    assert!(game.presentation.alerts.is_empty());
    assert!(
        !game
            .presentation
            .sounds_pending
            .iter()
            .any(|(sound, _)| *sound == SoundKind::Alert)
    );
}

#[test]
fn anonymous_unit_hitscan_uses_owner_damage_evidence_for_alerts() {
    let mut source = blind_bulwark_scene(UnitKind::Excavator);
    let report = source.do_tick();
    let events: Vec<_> = report
        .events
        .iter()
        .map(|event| match event {
            Event::TurretFired {
                turret_pos,
                target_pos,
                ..
            } => Event::AttackHit {
                attacker: UnitId(99),
                attacker_kind: UnitKind::Lancer,
                weapon: 0,
                target: None,
                attacker_pos: *turret_pos,
                target_pos: *target_pos,
            },
            other => other.clone(),
        })
        .collect();
    for (human, hit) in [(0, true), (1, true), (0, false)] {
        let mut game = blind_bulwark_scene(UnitKind::Excavator);
        game.presentation.human = oxide_sim::PlayerId(human);
        let events: Vec<_> = events
            .iter()
            .filter(|event| hit || !matches!(event, Event::DamageTaken { .. }))
            .cloned()
            .collect();
        game.presentation.remember_previous_tick(&game.state);
        game.presentation.observe_tick(&source.state, &events, &[]);
        assert_eq!(
            game.presentation.alerts.len(),
            usize::from(human == 0 && hit)
        );
        assert!(game.presentation.aim_unit_targets.is_empty());
    }
}

#[test]
fn casualty_art_survives_unit_removal_in_live_and_playback() {
    for (kind, attacker) in [
        (UnitKind::Sentinel, UnitKind::Scuttler),
        (UnitKind::Kestrel, UnitKind::Flakhound),
        (UnitKind::Condor, UnitKind::Flakhound),
    ] {
        let mut scenario = oxide_sim::Scenario::skirmish();
        for player in &mut scenario.players {
            player.bot_config = None;
        }
        scenario.units = vec![
            oxide_sim::scenario::UnitSpec {
                player: 0,
                kind,
                x: 10,
                y: 10,
            },
            oxide_sim::scenario::UnitSpec {
                player: 1,
                kind: attacker,
                x: 11,
                y: 10,
            },
        ];
        let mut live = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
        let mut wire = serde_json::to_value(&*live.state).unwrap();
        wire["units"][0]["hp"] = serde_json::json!(1);
        live.state.0 = std::sync::Arc::new(serde_json::from_value(wire).unwrap());
        let mut playback = Presentation::new(
            &live.state,
            live.presentation.human,
            Vec2::new(1280.0, 800.0),
        );
        let mut reference = (*live.state).clone();
        let mut died = false;
        for _ in 0..80 {
            let expected_body = UnitBody::capture(
                &live.presentation,
                &live.state,
                live.state.unit(UnitId(0)).unwrap(),
                false,
            );
            playback.remember_previous_tick(&reference);
            let expected = reference.tick(&[]);
            let report = live.do_tick();
            playback.observe_tick(&reference, &expected.events, &expected.movement);
            assert_eq!(live.state.hash(), reference.hash());
            if report.events.iter().any(|event| {
                matches!(
                    event,
                    Event::UnitDied {
                        unit: UnitId(0),
                        ..
                    }
                )
            }) {
                for presentation in [&live.presentation, &playback] {
                    assert!(reference.unit(UnitId(0)).is_none());
                    let body = presentation
                        .fx
                        .iter()
                        .find_map(|effect| match effect.kind {
                            EffectKind::Falling { body, seed: 0, .. }
                            | EffectKind::Debris { body, seed: 0, .. } => Some(body),
                            _ => None,
                        })
                        .expect("removed casualty retains its presentation");
                    assert_eq!(body.kind, kind);
                    assert_eq!(body.faction, expected_body.faction);
                    assert_eq!(body.player, expected_body.player);
                    assert_eq!(body.rotation, expected_body.rotation);
                    assert_eq!(body.velocity, expected_body.velocity);
                }
                died = true;
                break;
            }
        }
        assert!(died, "{kind:?} must die in the staged engagement");
    }
}

#[test]
fn ground_death_keeps_the_eased_origin_and_respects_reduced_motion_yaw() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.units = vec![oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: UnitKind::Harvester,
        x: 10,
        y: 10,
    }];
    let mut game = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
    let unit = game.state.units()[0].clone();
    let mut slide = crate::slide_motion::SlideMotion::new(0);
    for tick in 1..=10 {
        slide.observe(
            tick,
            0.0,
            Vec2::new(0.11, 0.0),
            Vec2::new(0.0, 0.11),
            0.11,
            true,
        );
    }
    game.presentation.slide_motion.insert(unit.id.0, slide);
    let drawn = game.presentation.draw_pos(unit.id, unit.pos, 1.0);
    assert!((drawn - world_vec(unit.pos)).length() > 0.14);
    let eased_body = UnitBody::capture(&game.presentation, &game.state, &unit, false);
    let reduced_body = UnitBody::capture(&game.presentation, &game.state, &unit, true);
    assert!(
        (eased_body.rotation - reduced_body.rotation - crate::slide_motion::MAX_YAW).abs() < 1e-6
    );
    assert_eq!(
        reduced_body.rotation,
        game.presentation
            .draw_heading(unit.id, unit.weapon_heading(), 1.0)
    );
    game.presentation.remember_previous_tick(&game.state);
    game.presentation.slide_motion.clear();
    game.presentation.spawn_fx(
        &game.state,
        &[Event::UnitDied {
            unit: unit.id,
            pos: unit.pos,
            player: unit.player,
            kind: unit.kind,
            grounded: true,
        }],
    );
    let origin = game
        .presentation
        .fx
        .iter()
        .find_map(|effect| match effect.kind {
            EffectKind::Debris { at, seed, .. } if seed == unit.id.0 => Some(at),
            _ => None,
        })
        .expect("ground casualty has debris");
    assert_eq!(origin, drawn);
}

#[test]
fn wreck_capture_keeps_the_hull_bearing_when_the_turret_aims_away() {
    for kind in [
        UnitKind::Sentinel,
        UnitKind::Warden,
        UnitKind::Lancer,
        UnitKind::Buzzard,
    ] {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.units = vec![oxide_sim::scenario::UnitSpec {
            player: 0,
            kind,
            x: 8,
            y: 8,
        }];
        let mut game = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
        game.presentation.hull_heading.insert(0, (0.4, 0.7));
        let unit = game.state.unit(UnitId(0)).unwrap();
        assert!(
            (game
                .presentation
                .draw_heading(unit.id, unit.weapon_heading(), 1.0)
                - 0.7)
                .abs()
                > 0.1
        );
        assert!(
            (UnitBody::capture(&game.presentation, &game.state, unit, false).rotation - 0.7).abs()
                < 1e-6,
            "{kind:?}"
        );
    }
}

#[test]
fn hostile_crash_contact_retains_feedback_when_it_kills_the_only_observer() {
    for witnessed in [true, false] {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.map = vec![".".repeat(48); 32];
        scenario.map[2].replace_range(2..3, "1");
        scenario.map[2].replace_range(42..43, "2");
        for player in &mut scenario.players {
            player.bot_config = None;
        }
        scenario.units = vec![
            oxide_sim::scenario::UnitSpec {
                player: 1,
                kind: UnitKind::Condor,
                x: 20,
                y: 25,
            },
            oxide_sim::scenario::UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: if witnessed { 20 } else { 4 },
                y: 25,
            },
        ];
        let mut game = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
        let at = chassis::grid::TilePos::new(20, 25).center();
        let crash = oxide_sim::state::AircraftCrash {
            unit: UnitId(0),
            player: oxide_sim::PlayerId(1),
            kind: UnitKind::Condor,
            heading: 0,
            launch: at,
            impact: at,
            started: 0,
            arrival: 13,
        };
        let mut wire = serde_json::to_value(&*game.state).unwrap();
        wire["units"].as_array_mut().unwrap().remove(0);
        wire["units"][0]["hp"] = serde_json::json!(40);
        wire["tick"] = serde_json::json!(13);
        wire["aircraft_crashes"] = serde_json::json!([crash]);
        for view in wire["vision"].as_array_mut().unwrap() {
            let tracks = view["tracking"]["tracks"].as_array_mut().unwrap();
            tracks.retain(|track| track["visible_unit"].as_u64() != Some(0));
            for track in tracks {
                for sample in track["history"].as_array_mut().unwrap() {
                    sample["tick"] = serde_json::json!(13);
                }
            }
        }
        game.replace_state_after_jump(&serde_json::from_value(wire).unwrap());
        let tile = chassis::grid::TilePos::containing(at);
        assert_eq!(game.my_vision().visible(tile), witnessed);
        game.present_ticks(1);
        assert!(!game.my_vision().visible(tile));
        assert_eq!(game.state.unit(UnitId(1)).is_none(), witnessed);
        assert!(
            game.presentation
                .sounds_pending
                .iter()
                .any(
                    |(sound, pos)| *sound == SoundKind::BuildingBoom && *pos == Some(world_vec(at))
                )
        );
        assert_eq!(game.presentation.fx.iter().any(|effect| matches!(effect.kind,
            EffectKind::Falling { crash: Some(saved), impact_witnessed: true, .. } if saved.unit == crash.unit
        )), witnessed);
    }
}

#[test]
fn scheduled_crash_uses_sim_time_and_restores_after_a_seek() {
    let mut game =
        Game::with_viewport(oxide_sim::Scenario::skirmish(), Vec2::new(1280.0, 800.0)).unwrap();
    let mut wire = serde_json::to_value(&*game.state).unwrap();
    let unit = wire["units"].as_array_mut().unwrap().remove(0);
    let crash = oxide_sim::state::AircraftCrash {
        unit: serde_json::from_value(unit["id"].clone()).unwrap(),
        player: game.presentation.human,
        kind: UnitKind::Condor,
        heading: 32,
        launch: chassis::grid::TilePos::new(8, 8).center(),
        impact: chassis::grid::TilePos::new(9, 8).center(),
        started: 0,
        arrival: 13,
    };
    wire["tick"] = serde_json::json!(5);
    wire["aircraft_crashes"] = serde_json::json!([crash]);
    let state = serde_json::from_value(wire).unwrap();
    game.replace_state_after_jump(&state);
    game.presentation.paused = true;
    let age = game.presentation.fx[0].age_at(game.state.current_tick(), 0.0);
    assert!((age - 4.0 * crate::game::TICK_DT).abs() < 1.0e-6);
    game.update_fx(30.0);
    assert_eq!(
        game.presentation.fx.len(),
        1,
        "a paused fall cannot expire on wall time"
    );
    assert_eq!(
        game.presentation.fx[0].age_at(game.state.current_tick(), 0.0),
        age
    );
    game.advance_ticks(4);
    assert_eq!(
        game.presentation.fx.len(),
        1,
        "a bulk seek restores a pending fall"
    );
    let fx = &game.presentation.fx[0];
    assert!(
        matches!(fx.kind, EffectKind::Falling { crash: Some(restored), .. } if restored == crash)
    );
    assert!(
        (fx.age_at(game.state.current_tick(), 0.5) - 8.5 * crate::game::TICK_DT).abs() < 1.0e-6
    );
    game.present_ticks(5);
    assert_eq!(game.state.current_tick(), 14);
    assert!(game.state.aircraft_crashes().is_empty());
    assert_eq!(
        game.presentation
            .fx
            .iter()
            .filter(|fx| matches!(fx.kind, EffectKind::Falling { crash: Some(_), .. }))
            .count(),
        1,
        "impact continues the same casualty effect without a duplicate explosion"
    );
}

#[test]
fn hidden_casualty_does_not_reveal_art_or_audio() {
    let mut game =
        Game::with_viewport(oxide_sim::Scenario::skirmish(), Vec2::new(1280.0, 800.0)).unwrap();
    let at = chassis::grid::TilePos::new(30, 20).center();
    assert!(
        !game
            .my_vision()
            .visible(chassis::grid::TilePos::containing(at))
    );
    game.presentation.fx_previous = PreviousEffects::capture(&game.presentation, &game.state);
    game.presentation.spawn_fx(
        &game.state,
        &[Event::UnitDied {
            unit: UnitId(999),
            kind: UnitKind::Condor,
            player: oxide_sim::PlayerId(1),
            pos: at,
            grounded: false,
        }],
    );
    assert!(game.presentation.fx.is_empty());
    assert!(game.presentation.sounds_pending.is_empty());
}

#[test]
fn collapse_preserves_the_casualty_and_live_playback_state_parity() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot_config = None;
    }
    scenario.units = vec![oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Scuttler,
        x: 10,
        y: 10,
    }];
    scenario.buildings = vec![oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::Bastion,
        x: 11,
        y: 10,
    }];
    let mut live =
        crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
            .unwrap();
    let mut wire = serde_json::to_value(&*live.state).unwrap();
    let casualty = wire["buildings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|b| b["kind"] == "bastion")
        .unwrap();
    casualty["hp"] = serde_json::json!(1);
    live.state.0 = std::sync::Arc::new(serde_json::from_value(wire).unwrap());
    let mut playback = Presentation::new(
        &live.state,
        live.presentation.human,
        Vec2::new(1280.0, 800.0),
    );
    let mut reference = (*live.state).clone();
    let id = live
        .state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    for _ in 0..40 {
        playback.remember_previous_tick(&reference);
        let expected = reference.tick(&[]);
        let report = live.do_tick();
        playback.observe_tick(&reference, &expected.events, &expected.movement);
        assert_eq!(live.state.hash(), reference.hash());
        if report.events.iter().any(|event| matches!(event, oxide_sim::Event::BuildingDestroyed { building, .. } if *building == id)) {
            assert!(live.state.building(id).is_none());
            for presentation in [&live.presentation, &playback] {
                assert!(presentation.fx.iter().any(|fx| matches!(fx.kind, EffectKind::Collapse { body, seed, .. } if body.kind == BuildingKind::Bastion && seed == id.0)));
                assert!(!presentation.fx.iter().any(|fx| matches!(fx.kind, EffectKind::Puff { .. })));
            }
            live.update_fx(4.6);
            assert!(!live.presentation.fx.iter().any(|fx| matches!(fx.kind, EffectKind::Collapse { .. })));
            return;
        }
    }
    panic!("the adjacent Scuttler must destroy the damaged Bastion");
}

#[test]
fn projectile_impacts_sound_on_visible_and_hidden_ground_for_either_owner() {
    use oxide_sim::{PlayerId, ProjectileKind};
    for kind in [
        ProjectileKind::Shell,
        ProjectileKind::Missile,
        ProjectileKind::Bomb,
    ] {
        for player in [PlayerId(0), PlayerId(1)] {
            for visible in [true, false] {
                let mut scenario = oxide_sim::Scenario::skirmish();
                for seat in &mut scenario.players {
                    seat.bot_config = None;
                    seat.faction = oxide_sim::Faction::Ferrous;
                }
                scenario.units.push(oxide_sim::scenario::UnitSpec {
                    player: player.0,
                    kind: match kind {
                        ProjectileKind::Shell => UnitKind::Bombard,
                        ProjectileKind::Missile => UnitKind::Avalanche,
                        ProjectileKind::Bomb => UnitKind::Condor,
                    },
                    x: if player.0 == 0 { 8 } else { 28 },
                    y: 16,
                });
                let mut game = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
                let tile = if visible {
                    chassis::grid::TilePos::new(5, 5)
                } else {
                    chassis::grid::TilePos::new(30, 20)
                };
                assert_eq!(game.my_vision().visible(tile), visible);
                let at = tile.center();
                let shooter = game
                    .state
                    .units()
                    .iter()
                    .rev()
                    .find(|unit| unit.player == player)
                    .unwrap();
                let shell = oxide_sim::state::Shell {
                    kind,
                    player,
                    shooter: Target::Unit(shooter.id),
                    launch: shooter.pos,
                    impact: at,
                    launched_at: 0,
                    arrival: 0,
                    damage: 1,
                    targets: oxide_sim::stats::DomainMask::GROUND,
                    splash: None,
                };
                let mut wire = serde_json::to_value(&*game.state).unwrap();
                wire["shells"] = serde_json::json!([shell]);
                game.replace_state_after_jump(&serde_json::from_value(wire).unwrap());
                let mut reference = (*game.state).clone();
                let report = reference.tick(&[]);
                assert!(report.events.iter().any(
                    |event| matches!(event, Event::ShellLanded { at: impact, .. } if *impact == at)
                ));
                game.present_ticks(1);
                let sound = if kind == ProjectileKind::Missile {
                    SoundKind::RocketImpact
                } else {
                    SoundKind::Artillery
                };
                assert!(
                    game.presentation
                        .sounds_pending
                        .contains(&(sound, Some(world_vec(at)))),
                    "{kind:?}, {player:?}, visible={visible}"
                );
                assert_eq!(game.state.hash(), reference.hash());
                assert_eq!(game.my_vision().visible(tile), visible);
            }
        }
    }
}

#[test]
fn charge_detonation_plays_one_blast_and_preserves_other_building_losses() {
    for viewer in [0, 1, 2] {
        for collateral_charge in [false, true] {
            let mut map = vec![".".repeat(80); 40];
            for (x, y, mark) in [(3, 3, "1"), (60, 3, "2"), (65, 30, "3")] {
                map[y].replace_range(x..=x, mark);
            }
            let mut buildings = vec![serde_json::json!({
                "player": 1, "kind": "scuttle_charge", "x": 30, "y": 20
            })];
            if collateral_charge {
                buildings.push(serde_json::json!({
                    "player": 2, "kind": "scuttle_charge", "x": 31, "y": 20
                }));
            }
            let scenario = serde_json::from_value(serde_json::json!({
                "name": "Charge explosion audio", "map": map,
                "players": [
                    {"name": "Observer", "faction": "ferrous", "bot": false},
                    {"name": "Mine", "faction": "cupric", "bot": true},
                    {"name": "Trigger", "faction": "ferrous", "bot": true}
                ],
                "units": [{"player": 2, "kind": "warden", "x": 30, "y": 20}],
                "buildings": buildings
            }))
            .unwrap();
            let mut game = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
            game.presentation.human = oxide_sim::PlayerId(viewer);
            let tile = chassis::grid::TilePos::new(30, 20);
            if viewer == 0 {
                assert!(!game.my_vision().visible(tile));
            }
            let report = game.do_tick();
            if viewer == 0 {
                assert!(game.presentation.alerts.is_empty());
            } else if viewer == 2 && !collateral_charge {
                assert_eq!(
                    game.presentation.alerts,
                    vec![(world_vec(tile.center()), 0.0)]
                );
                assert!(
                    game.presentation
                        .sounds_pending
                        .iter()
                        .any(|(kind, _)| *kind == SoundKind::Alert)
                );
                let mut playback =
                    Game::with_viewport(game.scenario.clone(), Vec2::new(1280.0, 800.0)).unwrap();
                playback.presentation.human = game.presentation.human;
                playback
                    .presentation
                    .remember_previous_tick(&playback.state);
                playback
                    .presentation
                    .observe_tick(&game.state, &report.events, &report.movement);
                assert_eq!(playback.presentation.alerts, game.presentation.alerts);
            }
            let detonated: Vec<_> = report
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::ChargeDetonated { building, .. } => Some(*building),
                    _ => None,
                })
                .collect();
            assert_eq!(detonated.len(), 1);
            assert!(report.events.iter().any(|event| matches!(
                event, Event::BuildingDestroyed { building, .. } if *building == detonated[0]
            )));
            let other_losses = report.events.iter().filter(|event| matches!(
                event, Event::BuildingDestroyed { building, .. } if *building != detonated[0]
            )).count();
            assert_eq!(other_losses, usize::from(collateral_charge));
            let explosion_cues: Vec<_> = game
                .presentation
                .sounds_pending
                .iter()
                .copied()
                .filter(|(kind, _)| kind.is_explosion())
                .collect();
            let mut expected = vec![(SoundKind::DemolitionBoom, Some(world_vec(tile.center())))];
            if collateral_charge {
                expected.push((
                    SoundKind::BuildingBoom,
                    Some(world_vec(tile.offset(1, 0).center())),
                ));
            }
            assert_eq!(
                explosion_cues, expected,
                "viewer={viewer}, collateral={collateral_charge}"
            );
            let mixed = crate::audio_mix::frame_mix(
                explosion_cues,
                world_vec(tile.center()),
                Vec2::new(20.0, 12.5),
                32.0,
            );
            assert_eq!(mixed.len(), expected.len());
            assert!(mixed.iter().all(|sound| sound.gain == 1.0));
        }
    }
}

#[test]
fn hidden_demolition_and_building_loss_sound_without_revealing_identity() {
    let mut game =
        Game::with_viewport(oxide_sim::Scenario::skirmish(), Vec2::new(1280.0, 800.0)).unwrap();
    let at = chassis::grid::TilePos::new(30, 20).center();
    let player = oxide_sim::PlayerId(1);
    assert!(
        !game
            .my_vision()
            .visible(chassis::grid::TilePos::containing(at))
    );
    let hash = game.state.hash();
    for event in [
        Event::ChargeDetonated {
            building: BuildingId(999),
            player,
            at,
        },
        Event::AttackHit {
            attacker: UnitId(999),
            attacker_kind: UnitKind::Sapper,
            weapon: 0,
            target: None,
            attacker_pos: at,
            target_pos: at,
        },
        Event::BuildingDestroyed {
            building: BuildingId(999),
            player,
            tier: 0,
            pos: at,
        },
    ] {
        game.presentation.sounds_pending.clear();
        game.presentation.spawn_fx(&game.state, &[event]);
        assert_eq!(game.presentation.sounds_pending.len(), 1);
        assert!(game.presentation.sounds_pending[0].0.is_explosion());
        assert_eq!(game.presentation.sounds_pending[0].1, Some(world_vec(at)));
        assert_eq!(game.state.hash(), hash);
        assert!(game.presentation.toasts.is_empty());
        assert!(
            !game
                .presentation
                .fx
                .iter()
                .any(|effect| matches!(effect.kind, EffectKind::Collapse { .. }))
        );
    }
    game.presentation.sounds_pending.clear();
    game.presentation.spawn_fx(
        &game.state,
        &[Event::AttackHit {
            attacker: UnitId(999),
            attacker_kind: UnitKind::Sentinel,
            weapon: 0,
            target: None,
            attacker_pos: at,
            target_pos: at,
        }],
    );
    assert!(
        game.presentation.sounds_pending.is_empty(),
        "ordinary hidden firing remains silent"
    );
}

#[test]
fn impact_metadata_is_consumed_in_landing_order() {
    let mut game = crate::game::Game::with_viewport(
        oxide_sim::Scenario::skirmish(),
        macroquad::prelude::vec2(1280.0, 800.0),
    )
    .unwrap();
    game.state.tick(&[]);
    let at = game.state.units()[0].pos;
    for kind in [
        oxide_sim::ProjectileKind::Missile,
        oxide_sim::ProjectileKind::Shell,
    ] {
        game.presentation
            .fx_previous
            .shells
            .push(oxide_sim::state::Shell {
                kind,
                shooter: Target::Unit(UnitId(0)),
                player: oxide_sim::PlayerId(0),
                launch: at,
                impact: at,
                launched_at: 0,
                arrival: 0,
                damage: 1,
                targets: oxide_sim::stats::DomainMask::GROUND,
                splash: None,
            });
    }
    let event = oxide_sim::Event::ShellLanded {
        player: oxide_sim::PlayerId(0),
        at,
        targets: oxide_sim::stats::DomainMask::GROUND,
        splash: None,
    };
    game.presentation
        .spawn_fx(&game.state, &[event.clone(), event]);
    let payloads: Vec<_> = game
        .presentation
        .fx
        .iter()
        .filter_map(|fx| {
            if let EffectKind::Impact { payload, .. } = fx.kind {
                Some(payload)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        payloads,
        vec![
            oxide_sim::ProjectileKind::Missile,
            oxide_sim::ProjectileKind::Shell
        ]
    );
    assert!(game.presentation.fx_previous.shells.is_empty());
}

#[test]
fn destroyed_upgraded_flak_keeps_its_final_six_round_volley() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.units = vec![oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Buzzard,
        x: 13,
        y: 10,
    }];
    scenario.buildings = vec![oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::FlakTurret,
        x: 11,
        y: 10,
    }];
    let mut game =
        crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
            .unwrap();
    let mut value = serde_json::to_value(&*game.state).unwrap();
    value["units"][0]["heading"] = serde_json::json!(128);
    value["buildings"][2]["tier"] = serde_json::json!(1);
    value["buildings"][2]["hp"] = serde_json::json!(1);
    let mut state: oxide_sim::State = serde_json::from_value(value).unwrap();
    game.replace_state_after_jump(&state);
    let report = state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: oxide_sim::Command::Attack {
            units: vec![UnitId(0)],
            target: Target::Building(BuildingId(2)).into(),
            queue: false,
        },
    }]);
    game.presentation.remember_previous_tick(&game.state);
    game.presentation
        .observe_tick(&state, &report.events, &report.movement);
    assert!(state.building(BuildingId(2)).is_none());
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::TurretFired {
            turret: BuildingId(2),
            tier: 1,
            ..
        }
    )));
    assert!(game.presentation.fx.iter().any(|effect| matches!(
        effect.kind,
        EffectKind::DirectShot {
            style: ShotStyle::FlakBurst {
                rounds_per_yoke: 3,
                ..
            },
            ..
        }
    )));
}

fn face_south(game: &mut crate::game::Game, id: UnitId) {
    let mut value = serde_json::to_value(&*game.state).unwrap();
    let unit = value["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|unit| unit["id"] == serde_json::json!(id))
        .unwrap();
    unit["heading"] = serde_json::json!(64);
    game.replace_state_after_jump(&serde_json::from_value(value).unwrap());
}

fn defense_tracking_game() -> (crate::game::Game, BuildingId, UnitId) {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 11,
        y: 10,
    });
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Harvester,
        x: 14,
        y: 10,
    });
    let game = crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
        .expect("tracking scenario builds");
    let building = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let target = game
        .state
        .units()
        .iter()
        .find(|unit| unit.tile() == chassis::grid::TilePos::new(14, 10))
        .unwrap()
        .id;
    (game, building, target)
}

#[test]
fn every_weapon_family_uses_its_physical_report() {
    assert_eq!(unit_shot_style(UnitKind::Scuttler, 0), ShotStyle::Contact);
    assert_eq!(unit_shot_style(UnitKind::Lancer, 0), ShotStyle::Rail);
    assert_eq!(
        unit_shot_style(UnitKind::Flakhound, 0),
        ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::OneTick,
            rounds_per_yoke: 2,
        }
    );
    assert_eq!(
        unit_shot_style(UnitKind::Stinger, 0),
        ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::None,
            rounds_per_yoke: 1,
        }
    );
    // Both Sentinel slots speak through its one physical barrel;
    // the second is a weaker skyward poke, not a paired flak gun.
    assert_eq!(
        unit_shot_style(UnitKind::Sentinel, 0),
        ShotStyle::Kinetic { heavy: false }
    );
    assert_eq!(
        unit_shot_style(UnitKind::Sentinel, 1),
        ShotStyle::Kinetic { heavy: false }
    );
    assert_eq!(
        unit_shot_style(UnitKind::Warden, 0),
        ShotStyle::Kinetic { heavy: true }
    );
    assert_eq!(unit_shot_style(UnitKind::Breaker, 0), ShotStyle::Mortar);
    assert_eq!(
        unit_shot_style(UnitKind::Buzzard, 0),
        ShotStyle::Kinetic { heavy: true }
    );
    assert_eq!(unit_shot_style(UnitKind::Darter, 0), ShotStyle::ForgeSpot);
    assert_eq!(unit_shot_style(UnitKind::Talon, 0), ShotStyle::ForgeSpot);
    assert_eq!(unit_shot_style(UnitKind::Wisp, 0), ShotStyle::ForgeSpot);
    assert_eq!(
        defense_shot_style(BuildingKind::FlakTurret, 0),
        ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::OneAndHalfTicks,
            rounds_per_yoke: 2,
        }
    );
    assert_eq!(
        defense_shot_style(BuildingKind::FlakTurret, 1),
        ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::OneAndHalfTicks,
            rounds_per_yoke: 3,
        }
    );
    assert_eq!(
        defense_shot_style(BuildingKind::Turret, 0),
        ShotStyle::ForgeSpot
    );
}

#[test]
fn flak_yoke_rounds_switch_with_the_second_muzzle_frame() {
    assert_eq!(
        crate::presentation_animation::FLAKHOUND_REPORT_TICKS / 2.0,
        FlakYokeDelay::OneTick.ticks()
    );
    assert_eq!(
        crate::presentation_animation::FLAK_TURRET_REPORT_TICKS / 2.0,
        FlakYokeDelay::OneAndHalfTicks.ticks()
    );
}

#[test]
fn splash_bloom_stays_with_the_one_direct_report_until_arrival() {
    let mut effects = Vec::new();
    push_direct_report(
        &mut effects,
        ShotStyle::FlakBurst {
            yoke_delay: FlakYokeDelay::OneTick,
            rounds_per_yoke: 2,
        },
        Vec2::ZERO,
        Vec2::ONE,
        Some(1.25),
        42,
        None,
    );

    assert_eq!(effects.len(), 1, "one hit creates one flak report");
    assert!(matches!(
        effects[0].kind,
        EffectKind::DirectShot {
            style: ShotStyle::FlakBurst {
                yoke_delay: FlakYokeDelay::OneTick,
                rounds_per_yoke: 2,
            },
            splash: Some(1.25),
            completed_tick: 42,
            ..
        }
    ));
}

#[test]
fn direct_reports_age_on_sim_time_instead_of_wall_time() {
    let shot = Effect {
        kind: EffectKind::DirectShot {
            style: ShotStyle::ForgeSpot,
            from: Vec2::ZERO,
            to: Vec2::ONE,
            splash: None,
            completed_tick: 100,
            surface: None,
            attacker: None,
        },
        age: 0.0,
    };
    assert_eq!(shot.age_at(100, 0.0), 0.0);
    assert!((shot.age_at(102, 0.5) - 2.5 * crate::game::TICK_DT).abs() < 1.0e-6);
}

#[test]
fn own_sapper_report_survives_its_same_tick_death_and_vision_loss() {
    let scenario = oxide_sim::Scenario::skirmish();
    let mut game =
        crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
            .expect("Sapper presentation scenario builds");
    let attacker = UnitId(999);
    let at = chassis::grid::TilePos::new(30, 20).center();
    let events = [
        oxide_sim::Event::AttackHit {
            attacker,
            attacker_kind: UnitKind::Sapper,
            weapon: 0,
            target: Some(Target::Building(BuildingId(999))),
            attacker_pos: at,
            target_pos: at,
        },
        oxide_sim::Event::UnitDied {
            unit: attacker,
            kind: UnitKind::Sapper,
            player: game.presentation.human,
            pos: at,
            grounded: true,
        },
    ];

    game.presentation.spawn_fx(&game.state, &events);

    assert!(game.presentation.fx.iter().any(|effect| matches!(
        effect.kind,
        EffectKind::SapperDetonation { player, .. } if player == game.presentation.human
    )));
    assert!(
        game.presentation
            .sounds_pending
            .iter()
            .any(|(sound, _)| *sound == SoundKind::DemolitionBoom)
    );
}

#[test]
fn enemy_sapper_report_survives_killing_the_last_local_observer() {
    let scenario = oxide_sim::Scenario::skirmish();
    let mut game =
        crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
            .expect("Sapper presentation scenario builds");
    let attacker = UnitId(998);
    let victim = UnitId(999);
    let at = chassis::grid::TilePos::new(30, 20).center();
    let events = [
        oxide_sim::Event::AttackHit {
            attacker,
            attacker_kind: UnitKind::Sapper,
            weapon: 0,
            target: Some(Target::Unit(victim)),
            attacker_pos: at,
            target_pos: at,
        },
        oxide_sim::Event::UnitDied {
            unit: attacker,
            kind: UnitKind::Sapper,
            player: oxide_sim::PlayerId(1),
            pos: at,
            grounded: true,
        },
        oxide_sim::Event::UnitDied {
            unit: victim,
            kind: UnitKind::Sentinel,
            player: game.presentation.human,
            pos: at,
            grounded: true,
        },
    ];

    game.presentation.spawn_fx(&game.state, &events);

    assert!(
        game.presentation
            .sounds_pending
            .iter()
            .any(|(sound, _)| *sound == SoundKind::DemolitionBoom),
        "the witnessed demolition must not become silent after its last observer dies"
    );
    assert!(game.presentation.fx.iter().any(|effect| matches!(
        effect.kind,
        EffectKind::SapperDetonation {
            source_witnessed: false,
            impact_witnessed: true,
            ..
        }
    )));
}

#[test]
fn final_volley_drains_after_the_simulation_stops() {
    let mut game = crate::game::Game::with_viewport(
        oxide_sim::Scenario::skirmish(),
        macroquad::prelude::vec2(1280.0, 800.0),
    )
    .expect("skirmish builds");
    game.issue(oxide_sim::Command::Surrender);
    game.do_tick();
    assert!(game.state.result().is_some());

    let completed_tick = game.state.current_tick();
    game.presentation.fx.push(Effect {
        kind: EffectKind::DirectShot {
            style: ShotStyle::ForgeSpot,
            from: Vec2::ZERO,
            to: Vec2::ONE,
            splash: None,
            completed_tick,
            surface: None,
            attacker: None,
        },
        age: 0.0,
    });
    game.update_fx(ShotStyle::ForgeSpot.life() + 0.01);
    assert!(game.presentation.fx.is_empty());
}

#[test]
fn only_bombard_and_bastion_use_real_shell_entities() {
    let units = [
        UnitKind::Harvester,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Bombard,
        UnitKind::Flakhound,
        UnitKind::Stinger,
        UnitKind::Buzzard,
        UnitKind::Darter,
        UnitKind::Talon,
        UnitKind::Wisp,
    ];
    let unit_shells: Vec<_> = units
        .into_iter()
        .filter(|kind| {
            kind.stats()
                .weapons
                .iter()
                .any(|weapon| weapon.projectile.is_some())
        })
        .collect();
    assert_eq!(unit_shells, vec![UnitKind::Bombard]);

    let buildings = [
        BuildingKind::Foundry,
        BuildingKind::Turret,
        BuildingKind::Fabricator,
        BuildingKind::FlakTurret,
        BuildingKind::Bastion,
        BuildingKind::Array,
        BuildingKind::Reclaimer,
        BuildingKind::RepairBay,
    ];
    let building_shells: Vec<_> = buildings
        .into_iter()
        .filter(|kind| {
            kind.base_stats()
                .weapons
                .iter()
                .any(|weapon| weapon.projectile.is_some())
        })
        .collect();
    assert_eq!(building_shells, vec![BuildingKind::Bastion]);
}

#[test]
fn approved_combatants_use_their_own_reports() {
    assert_eq!(unit_fire_sound(UnitKind::Sentinel), SoundKind::SentinelFire);
    assert_eq!(unit_fire_sound(UnitKind::Scuttler), SoundKind::ScuttlerFire);
    assert_eq!(unit_fire_sound(UnitKind::Lancer), SoundKind::LancerFire);
    assert_eq!(
        unit_fire_sound(UnitKind::Flakhound),
        SoundKind::FlakhoundFire
    );
    assert_eq!(unit_fire_sound(UnitKind::Stinger), SoundKind::StingerFire);
    assert_eq!(unit_fire_sound(UnitKind::Buzzard), SoundKind::BuzzardFire);
    assert_eq!(unit_fire_sound(UnitKind::Darter), SoundKind::DarterFire);
    assert_eq!(unit_fire_sound(UnitKind::Talon), SoundKind::TalonFire);
    assert_eq!(unit_fire_sound(UnitKind::Wisp), SoundKind::WispFire);
    assert_eq!(
        defense_fire_sound(BuildingKind::FlakTurret),
        SoundKind::FlakTurretFire
    );
    assert_eq!(
        shell_fire_sound(Target::Unit(UnitId(4))),
        SoundKind::BombardFire
    );
    assert_eq!(
        shell_fire_sound(Target::Building(BuildingId(7))),
        SoundKind::BastionFire
    );
}

#[test]
fn generic_combatants_keep_the_generic_report() {
    assert_eq!(unit_fire_sound(UnitKind::Harvester), SoundKind::Laser);
    assert_eq!(defense_fire_sound(BuildingKind::Turret), SoundKind::Laser);
}

#[test]
fn artillery_launch_audio_respects_sight_and_allegiance() {
    let bombard = Target::Unit(UnitId(4));
    assert_eq!(
        shell_launch_audio(
            bombard,
            Some(UnitKind::Bombard),
            AllegianceCue::Hostile,
            true,
            true
        ),
        Some((SoundKind::BombardFire, ShellSoundAnchor::Muzzle))
    );
    assert_eq!(
        shell_launch_audio(
            bombard,
            Some(UnitKind::Bombard),
            AllegianceCue::Hostile,
            false,
            true
        ),
        Some((SoundKind::ArtilleryLaunch, ShellSoundAnchor::Impact))
    );
    assert_eq!(
        shell_launch_audio(
            bombard,
            Some(UnitKind::Bombard),
            AllegianceCue::Hostile,
            false,
            false
        ),
        None
    );
    assert_eq!(
        shell_launch_audio(
            bombard,
            Some(UnitKind::Bombard),
            AllegianceCue::Ally,
            false,
            true
        ),
        None,
        "a fogged allied shell must not sound like an incoming threat"
    );
    assert_eq!(
        shell_launch_audio(
            bombard,
            Some(UnitKind::Bombard),
            AllegianceCue::Mine,
            false,
            false
        ),
        Some((SoundKind::BombardFire, ShellSoundAnchor::Muzzle)),
        "the local gun remains audible without revealing another seat"
    );
}

#[test]
fn every_projectile_shooter_launches_with_its_own_report() {
    let shooter = Target::Unit(UnitId(4));
    for (kind, report) in [
        (UnitKind::Bombard, SoundKind::BombardFire),
        (UnitKind::Avalanche, SoundKind::AvalancheFire),
        (UnitKind::Condor, SoundKind::BombRelease),
        (UnitKind::Moth, SoundKind::BombRelease),
    ] {
        assert_eq!(
            shell_launch_audio(shooter, Some(kind), AllegianceCue::Mine, true, true),
            Some((report, ShellSoundAnchor::Muzzle)),
            "{kind:?}"
        );
    }
}

#[test]
fn shot_visuals_begin_at_the_authored_muzzle_not_chassis_center() {
    let from = macroquad::prelude::vec2(2.0, 3.0);
    let to = macroquad::prelude::vec2(12.0, 3.0);
    assert_eq!(
        visual_shot_origin(from, to, 0.38),
        macroquad::prelude::vec2(2.38, 3.0)
    );
    assert_eq!(visual_shot_origin(from, from, 0.38), from);
    let buzzard_muzzle = 38.0 / 128.0 * crate::render::unit_draw_scale(UnitKind::Buzzard);
    assert_eq!(unit_muzzle_reach(UnitKind::Buzzard), buzzard_muzzle);
    let origin = unit_shot_origin(UnitKind::Buzzard, from, to);
    assert!((origin.x - (from.x + buzzard_muzzle)).abs() < 1e-5);
    assert!((origin.y - (from.y - 0.18)).abs() < 1e-5);
    assert_eq!(unit_muzzle_reach(UnitKind::Darter), 0.32);
    for kind in [
        UnitKind::Darter,
        UnitKind::Talon,
        UnitKind::Wisp,
        UnitKind::Shrike,
        UnitKind::Sylph,
    ] {
        assert_eq!(
            unit_shot_origin(kind, from, to).y,
            from.y - crate::render::air_presentation(kind, 1.0).2
        );
    }
    let warden_muzzle = 46.0 / 128.0 * crate::render::unit_draw_scale(UnitKind::Warden);
    assert!((unit_muzzle_reach(UnitKind::Warden) - warden_muzzle).abs() < 0.002);
    assert_eq!(
        unit_muzzle_reach(UnitKind::Breaker),
        40.0 / 128.0 * crate::render::unit_draw_scale(UnitKind::Breaker)
    );
    assert!(
        defense_muzzle_reach(BuildingKind::Bastion) > defense_muzzle_reach(BuildingKind::Turret)
    );
}

#[test]
fn defense_mount_tracks_only_a_target_the_viewer_may_see() {
    let (mut game, building, _) = defense_tracking_game();
    let report = game.state.tick(&[]);
    game.presentation.spawn_fx(&game.state, &report.events);
    assert!(game.state.building(building).unwrap().cooldown > 0);
    let hostile = game
        .state
        .units()
        .iter()
        .find(|unit| {
            game.state.hostile(game.presentation.human, unit.player)
                && !game.my_vision().visible(unit.tile())
        })
        .expect("skirmish has a fogged hostile unit");
    assert!(!game.my_vision().visible(hostile.tile()));
    let target = Target::Unit(hostile.id);
    game.presentation
        .aim_building_targets
        .insert(building.0, target);
    game.presentation
        .aim_buildings
        .insert(building.0, (0.42, 0.0));
    game.update_fx(crate::game::TICK_DT);

    game.presentation.refresh_defense_aim(&game.state);
    assert_eq!(game.presentation.aim_buildings[&building.0].0, 0.42);

    game.presentation.overlay = true;
    game.presentation.refresh_defense_aim(&game.state);
    assert_ne!(game.presentation.aim_buildings[&building.0].0, 0.42);
}

#[test]
fn defense_mount_follows_its_visible_target_during_reload() {
    let (mut game, building, target) = defense_tracking_game();
    face_south(&mut game, target);
    let report = game.state.tick(&[]);
    game.presentation.spawn_fx(&game.state, &report.events);
    let first_angle = game.presentation.aim_buildings[&building.0].0;
    let first_pos = game.state.unit(target).unwrap().pos;

    game.update_fx(crate::game::TICK_DT);
    let report = game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: oxide_sim::Command::Run {
            units: vec![target],
            goal: chassis::grid::TilePos::new(14, 14),
            queue: false,
        },
    }]);
    game.presentation.spawn_fx(&game.state, &report.events);

    assert_ne!(game.state.unit(target).unwrap().pos, first_pos);
    assert_ne!(game.presentation.aim_buildings[&building.0].0, first_angle);
    assert!(game.state.building(building).unwrap().cooldown > 0);
}

#[test]
fn shell_report_keeps_predicted_heading_on_the_launch_frame() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
        player: 0,
        kind: BuildingKind::Bastion,
        x: 11,
        y: 10,
    });
    scenario.units.push(oxide_sim::scenario::UnitSpec {
        player: 1,
        kind: UnitKind::Harvester,
        x: 16,
        y: 10,
    });
    let mut game =
        crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
            .expect("tracking scenario builds");
    let shooter = game
        .state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let target = game
        .state
        .units()
        .iter()
        .find(|unit| unit.tile() == chassis::grid::TilePos::new(16, 10))
        .unwrap()
        .id;
    face_south(&mut game, target);
    let report = game.state.tick(&[oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: oxide_sim::Command::Run {
            units: vec![target],
            goal: chassis::grid::TilePos::new(16, 14),
            queue: false,
        },
    }]);
    let (from, to) = report
        .events
        .iter()
        .find_map(|event| match event {
            oxide_sim::Event::ShellLaunched {
                shooter: Target::Building(id),
                from,
                to,
                ..
            } if *id == shooter => Some((*from, *to)),
            _ => None,
        })
        .expect("the Bastion launches while its target begins moving");
    assert!(game.state.building(shooter).unwrap().cooldown > 0);
    assert!(
        game.my_vision()
            .visible(game.state.unit(target).unwrap().tile())
    );
    let expected = world_vec(to) - world_vec(from);
    let expected_angle = expected.y.atan2(expected.x) + std::f32::consts::FRAC_PI_2;
    let current = world_vec(game.state.unit(target).unwrap().pos) - world_vec(from);
    let current_angle = current.y.atan2(current.x) + std::f32::consts::FRAC_PI_2;
    assert!((expected_angle - current_angle).abs() > 1e-4);

    game.presentation.spawn_fx(&game.state, &report.events);
    let angle = game.presentation.aim_buildings[&shooter.0].0;
    assert!((angle - expected_angle).abs() < 1e-6);
}

#[test]
fn ground_kills_scatter_debris_and_air_kills_fall() {
    crate::render::set_reduced_motion(false);
    let mut game = crate::game::Game::with_viewport(
        oxide_sim::Scenario::skirmish(),
        macroquad::prelude::vec2(1280.0, 800.0),
    )
    .expect("embedded skirmish builds");
    let at = chassis::fx::Vec2Fx {
        x: chassis::fx::Fx::from_num(5),
        y: chassis::fx::Fx::from_num(5),
    };
    game.presentation.spawn_fx(
        &game.state,
        &[oxide_sim::Event::UnitDied {
            unit: oxide_sim::UnitId(7),
            kind: UnitKind::Sentinel,
            player: oxide_sim::PlayerId(1),
            pos: at,
            grounded: true,
        }],
    );
    assert!(
        game.presentation
            .fx
            .iter()
            .any(|e| matches!(e.kind, EffectKind::Debris { seed: 7, .. })),
        "a ground kill scatters shards seeded by the casualty"
    );
    game.presentation.fx.clear();
    game.presentation.spawn_fx(
        &game.state,
        &[oxide_sim::Event::UnitDied {
            unit: oxide_sim::UnitId(8),
            kind: UnitKind::Buzzard,
            player: oxide_sim::PlayerId(1),
            pos: at,
            grounded: false,
        }],
    );
    assert!(
        game.presentation
            .fx
            .iter()
            .any(|e| matches!(e.kind, EffectKind::Falling { .. })),
        "a flyer tells its death with the fall"
    );
    assert!(
        !game
            .presentation
            .fx
            .iter()
            .any(|e| matches!(e.kind, EffectKind::Debris { .. })),
        "no double story for one death"
    );
    game.presentation.fx.clear();
    game.presentation.spawn_fx(
        &game.state,
        &[oxide_sim::Event::UnitDied {
            unit: oxide_sim::UnitId(9),
            kind: UnitKind::Condor,
            player: oxide_sim::PlayerId(1),
            pos: at,
            grounded: true,
        }],
    );
    assert!(
        game.presentation
            .fx
            .iter()
            .any(|e| matches!(e.kind, EffectKind::Debris { seed: 9, .. })),
        "a parked airframe has no altitude to fall from"
    );
    assert!(
        !game
            .presentation
            .fx
            .iter()
            .any(|e| matches!(e.kind, EffectKind::Falling { .. })),
        "no fall for a body already on the ground"
    );
}

#[test]
fn ranged_reports_retain_unit_contacts_through_lethal_hits() {
    for kind in [
        UnitKind::Sentinel,
        UnitKind::Lancer,
        UnitKind::Buzzard,
        UnitKind::Warden,
        UnitKind::Breaker,
    ] {
        let scenario = serde_json::from_value(serde_json::json!({
            "name":"Unit impacts", "mode":"sandbox", "map":vec![".............................."; 22],
            "players":[
                {"name":"Local","faction":"ferrous","scrap":0,"bot":false},
                {"name":"Target","faction":"cupric","scrap":0,"bot":false}
            ],
            "units":[{"player":0,"kind":kind,"x":10,"y":10},{"player":1,"kind":"excavator","x":14,"y":10}]
        })).unwrap();
        let mut game = Game::with_viewport(scenario, Vec2::new(1280., 800.)).unwrap();
        let mut wire = serde_json::to_value(&*game.state).unwrap();
        wire["units"][1]["hp"] = serde_json::json!(1);
        game.replace_state_after_jump(&serde_json::from_value(wire).unwrap());
        game.pending.push(oxide_sim::PlayerCommand {
            player: oxide_sim::PlayerId(0),
            command: oxide_sim::Command::Attack {
                units: vec![UnitId(0)],
                target: Target::Unit(UnitId(1)).into(),
                queue: false,
            },
        });
        for _ in 0..160 {
            game.present_ticks(1);
            if game.state.unit(UnitId(1)).is_none() {
                break;
            }
        }
        assert!(
            game.state.unit(UnitId(1)).is_none(),
            "{kind:?} lands its hit"
        );
        assert!(
            game.presentation.fx.iter().any(|effect| matches!(
                effect.kind,
                EffectKind::DirectShot {
                    surface: Some(HitSurface::Unit(UnitHit { id: UnitId(1), .. })),
                    ..
                }
            )),
            "{kind:?} retains the removed body's contact"
        );
    }
}

#[test]
fn bomb_contacts_preserve_ground_spread_and_use_simulation_time() {
    let scenario = serde_json::from_value(serde_json::json!({
        "name":"Bomb contacts", "mode":"sandbox", "map":vec!["...................................."; 24],
        "players":[
            {"name":"Local","faction":"ferrous","scrap":0,"bot":false},
            {"name":"Target","faction":"cupric","scrap":0,"bot":false}
        ],
        "units":[{"player":0,"kind":"moth","x":12,"y":11},{"player":0,"kind":"harvester","x":17,"y":15}],
        "buildings":[{"player":1,"kind":"fabricator","x":17,"y":10}]
    })).unwrap();
    let mut game = Game::with_viewport(scenario, Vec2::new(1280., 800.)).unwrap();
    game.pending.push(oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(0),
        command: oxide_sim::Command::Attack {
            units: vec![UnitId(0)],
            target: Target::Building(BuildingId(0)).into(),
            queue: false,
        },
    });
    let mut arrivals = Vec::new();
    for _ in 0..180 {
        let events = game.present_ticks(1);
        for event in events {
            if let Event::ShellLanded { at, .. } = event {
                let at = world_vec(at);
                let effect = game
                    .presentation
                    .fx
                    .iter()
                    .rev()
                    .find(|fx| matches!(fx.kind, EffectKind::Impact { at:p, .. } if p == at))
                    .unwrap();
                let EffectKind::Impact {
                    surface, payload, ..
                } = effect.kind
                else {
                    unreachable!()
                };
                let inside = at.x >= 17. && at.x <= 19. && at.y >= 10. && at.y <= 12.;
                assert_eq!(
                    surface.is_some(),
                    inside,
                    "a ground bomb cannot acquire building contact: {at:?}"
                );
                assert_eq!(payload, oxide_sim::ProjectileKind::Bomb);
                assert_eq!(effect.age_at(game.state.current_tick(), 0.), 0.);
                arrivals.push(at);
            }
        }
        if arrivals.len() == 6 {
            break;
        }
    }
    assert_eq!(arrivals.len(), 6);
    assert!(arrivals.iter().any(|at| at.x < 17.));
    assert!(arrivals.iter().any(|at| at.x > 19.));
    let tick = game.state.current_tick();
    let before = game
        .presentation
        .fx
        .iter()
        .find(|fx| matches!(fx.kind, EffectKind::Impact { .. }))
        .unwrap()
        .age_at(tick, 0.);
    game.update_fx(0.5);
    let after = game
        .presentation
        .fx
        .iter()
        .find(|fx| matches!(fx.kind, EffectKind::Impact { .. }))
        .unwrap()
        .age_at(tick, 0.);
    assert_eq!(before, after, "wall time cannot advance a paused impact");
}
#[test]
fn checkpoint_projectiles_recover_unit_contacts_without_launch_history() {
    for kind in [
        UnitKind::Bombard,
        UnitKind::Avalanche,
        UnitKind::Condor,
        UnitKind::Moth,
    ] {
        let scenario = serde_json::from_value(serde_json::json!({
            "name":"Restored contacts", "mode":"sandbox", "map":vec!["...................................."; 24],
            "players":[{"name":"Local","faction":"ferrous","scrap":0,"bot":false},
                {"name":"Target","faction":"cupric","scrap":0,"bot":false}],
            "units":[{"player":0,"kind":kind,"x":12,"y":11},
                {"player":0,"kind":"harvester","x":17,"y":15},
                {"player":1,"kind":"excavator","x":17,"y":11}]
        }))
        .unwrap();
        let mut game = Game::with_viewport(scenario, Vec2::new(1280., 800.)).unwrap();
        game.pending.push(oxide_sim::PlayerCommand {
            player: game.presentation.human,
            command: oxide_sim::Command::Attack {
                units: vec![UnitId(0)],
                target: Target::Unit(UnitId(2)).into(),
                queue: false,
            },
        });
        for _ in 0..200 {
            game.present_ticks(1);
            if !game.state.shells().is_empty() {
                break;
            }
        }
        assert!(!game.state.shells().is_empty(), "{kind:?} launched");
        let mut restored: Game =
            serde_json::from_slice(&serde_json::to_vec(&game).unwrap()).unwrap();
        assert_eq!(game.state.hash(), restored.state.hash());
        assert_eq!(restored.recorder.start_tick(), game.state.current_tick());
        assert!(restored.recorder.commands.is_empty());
        assert!(
            restored
                .presentation
                .projectile_releases
                .flight(restored.state.shells(), 0)
                .is_none()
        );
        let mut hit_unit = false;
        for _ in 0..80 {
            let before = game.present_ticks(1);
            let after = restored.present_ticks(1);
            assert_eq!(before, after);
            assert_eq!(game.state.hash(), restored.state.hash());
            for effect in &restored.presentation.fx {
                if let EffectKind::Impact {
                    surface: Some(HitSurface::Unit(hit)),
                    ..
                } = effect.kind
                {
                    assert_eq!(hit.id, UnitId(2));
                    hit_unit = true;
                }
            }
            if game.state.shells().is_empty() {
                break;
            }
        }
        assert!(
            hit_unit,
            "{kind:?} restored landing retained a unit recipient"
        );
    }
}

#[test]
fn lethal_hit_retains_the_moving_body_frame() {
    let scenario = serde_json::from_value(serde_json::json!({
        "name":"Moving lethal contact", "mode":"sandbox", "map":vec!["....................................";24],
        "players":[{"name":"Local","faction":"ferrous","scrap":0,"bot":false},
            {"name":"Target","faction":"cupric","scrap":0,"bot":false}],
        "units":[{"player":0,"kind":"lancer","x":10,"y":10},
            {"player":1,"kind":"scuttler","x":13,"y":10}]
    }))
    .unwrap();
    let mut game = Game::with_viewport(scenario, Vec2::new(1280., 800.)).unwrap();
    let mut wire = serde_json::to_value(&*game.state).unwrap();
    wire["units"][1]["hp"] = serde_json::json!(1);
    wire["units"][0]["cooldowns"][0] = serde_json::json!(10);
    game.replace_state_after_jump(&serde_json::from_value(wire).unwrap());
    game.pending.push(oxide_sim::PlayerCommand {
        player: oxide_sim::PlayerId(1),
        command: oxide_sim::Command::Run {
            units: vec![UnitId(1)],
            goal: chassis::grid::TilePos::new(25, 10),
            queue: false,
        },
    });
    game.present_ticks(6);
    game.issue(oxide_sim::Command::Attack {
        units: vec![UnitId(0)],
        target: Target::Unit(UnitId(1)).into(),
        queue: false,
    });
    for _ in 0..160 {
        let unit = game.state.unit(UnitId(1)).unwrap();
        let animation = crate::render::unit_animation(&game.presentation, &game.state, unit);
        let expected = crate::render::UnitSpriteFrame::capture(unit.kind, animation);
        let mut idle = animation;
        idle.locomotion = crate::presentation_animation::LocomotionState::Rest;
        game.present_ticks(1);
        if game.state.unit(UnitId(1)).is_none() {
            let hit = game
                .presentation
                .fx
                .iter()
                .find_map(|effect| match effect.kind {
                    EffectKind::DirectShot {
                        surface: Some(HitSurface::Unit(hit)),
                        ..
                    } if hit.id == UnitId(1) => Some(hit),
                    _ => None,
                })
                .unwrap();
            assert_eq!(hit.frame, expected);
            assert_ne!(
                hit.frame,
                crate::render::UnitSpriteFrame::capture(UnitKind::Scuttler, idle)
            );
            return;
        }
    }
    panic!("Lancer did not reach the moving target");
}
