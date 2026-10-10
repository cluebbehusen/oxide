use super::*;
use crate::game::Game;
use crate::slide_motion::SlideMotion;
use oxide_sim::Command;
use oxide_sim::UnitKind;

fn production_game() -> Game {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot_config = None;
    }
    scenario.units.clear();
    Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap()
}

#[test]
fn a_wreck_on_a_live_node_counts_toward_its_scrap() {
    assert_eq!(classify_salvage(0, 0), None);
    assert_eq!(classify_salvage(0, 30), Some(Salvage::Wreck(30)));
    assert_eq!(classify_salvage(400, 0), Some(Salvage::Scrap(400)));
    assert_eq!(classify_salvage(400, 30), Some(Salvage::Scrap(430)));
}

#[test]
fn newborn_collision_waits_for_an_interpolation_origin() {
    let mut game = production_game();
    let foundry = game
        .state
        .buildings()
        .iter()
        .find(|b| b.player == game.presentation.human)
        .unwrap()
        .id;
    for _ in 0..2 {
        game.issue(Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        });
    }
    for _ in 0..600 {
        let report = game.do_tick();
        for event in report.events {
            let Event::UnitTrained { unit: id, .. } = event else {
                continue;
            };
            if !report
                .movement
                .iter()
                .any(|motion| motion.unit == id && motion.correction != chassis::fx::Vec2Fx::ZERO)
            {
                continue;
            }
            let unit = game.state.unit(id).unwrap();
            let born_at = world_vec(unit.pos);
            assert!(!game.presentation.prev_pos.contains_key(&id.0));
            assert!(!game.presentation.slide_motion.contains_key(&id.0));
            for alpha in [0.0, 0.5, 1.0] {
                assert_eq!(game.presentation.draw_pos(id, unit.pos, alpha), born_at);
            }
            game.do_tick();
            let unit = game.state.unit(id).unwrap();
            assert_eq!(game.presentation.draw_pos(id, unit.pos, 0.0), born_at);
            assert!(game.presentation.slide_motion.contains_key(&id.0));
            return;
        }
    }
    panic!("production never spawned a colliding unit");
}

#[test]
fn a_stationary_fixed_weapon_clears_lean_when_aiming_but_idle_lean_eases() {
    for aiming in [false, true] {
        let mut scenario = Scenario::skirmish();
        scenario.units = vec![oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: UnitKind::Bombard,
            x: 10,
            y: 10,
        }];
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        let id = game.state.units()[0].id;
        let tick = game.state.current_tick();
        let mut slide = SlideMotion::new(tick.wrapping_sub(2));
        slide.observe(
            tick.wrapping_sub(1),
            0.0,
            vec2(0.11, 0.0),
            vec2(0.0, 0.11),
            0.11,
            true,
        );
        game.presentation.slide_motion.insert(id.0, slide);
        if aiming {
            game.presentation
                .aim_units
                .insert(id.0, (0.0, game.presentation.fx_clock));
        }
        game.presentation.observe_slides(&game.state, &[]);
        for alpha in [0.0, 0.5] {
            let yaw = game.presentation.slide_yaw(id, alpha, false);
            if aiming {
                assert_eq!(yaw, 0.0);
            } else {
                assert!(yaw > 0.0);
            }
        }
        assert_eq!(game.presentation.slide_yaw(id, 1.0, false), 0.0);
    }
}

#[test]
fn reduced_motion_suppresses_existing_lean_without_discarding_position_easing() {
    let mut game = production_game();
    let id = UnitId(99);
    let mut slide = SlideMotion::new(0);
    slide.observe(1, 0.0, vec2(0.11, 0.0), vec2(0.0, 0.11), 0.11, true);
    game.presentation.slide_motion.insert(id.0, slide);
    for alpha in [0.5, 1.0] {
        assert!(game.presentation.slide_yaw(id, alpha, false) > 0.0);
        assert_eq!(game.presentation.slide_yaw(id, alpha, true), 0.0);
        assert!(game.presentation.slide_motion[&id.0].lag(alpha).length() > 0.0);
    }
    assert!(game.presentation.slide_yaw(id, 1.0, false) > 0.0);
}
