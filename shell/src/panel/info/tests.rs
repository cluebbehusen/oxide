use super::*;
use crate::game::Game;

fn selected(kind: UnitKind, foreign: bool) -> Panel {
    let mut scenario = oxide_sim::Scenario::skirmish();
    let player = u8::from(foreign);
    scenario.units = vec![oxide_sim::scenario::UnitSpec {
        player,
        kind,
        x: 6,
        y: 5,
    }];
    let mut game = Game::with_viewport(scenario, macroquad::prelude::vec2(640.0, 400.0)).unwrap();
    game.presentation.selection.units = game.state.units().iter().map(|u| u.id).collect();
    build_for_palette(&game.view(), &BindingMap::classic(), false).unwrap()
}

#[test]
fn every_unit_exposes_health_sight_and_its_weapon_reload() {
    for kind in UnitKind::ALL {
        let panel = selected(kind, false);
        assert_eq!(
            panel.info.health.map(|health| (health.hp, health.max_hp)),
            Some((kind.stats().max_hp, kind.stats().max_hp))
        );
        assert!(
            panel
                .info
                .rows
                .iter()
                .any(|r| r.label == "Sight" && r.value == format!("{} tiles", kind.stats().vision))
        );
        let cycles: Vec<_> = panel
            .info
            .rows
            .iter()
            .filter(|r| r.label == "Reload")
            .map(|r| &r.value)
            .collect();
        assert_eq!(cycles.len(), kind.stats().weapons.len());
        for (actual, weapon) in cycles.into_iter().zip(kind.stats().weapons) {
            assert_eq!(*actual, tick_time_label(weapon.cooldown_ticks));
        }
    }
}

#[test]
fn role_facts_preserve_demolition_damage() {
    let sapper = selected(UnitKind::Sapper, false);
    assert!(
        sapper
            .info
            .rows
            .iter()
            .any(|r| r.label == "Structure hit" && r.value == "250 damage")
    );
}

#[test]
fn hostile_inspection_does_not_expose_current_load_or_orders() {
    for kind in [UnitKind::Skyhook, UnitKind::Harvester] {
        let panel = selected(kind, true);
        assert!(panel.cards.is_empty() && panel.queue.is_empty());
        let load = panel
            .info
            .rows
            .iter()
            .find(|r| matches!(r.label.as_str(), "Cargo" | "Scrap load"))
            .unwrap();
        assert!(!load.value.contains('/'));
    }
}
