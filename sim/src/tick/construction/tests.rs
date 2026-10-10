use super::*;
use crate::{BuildingKind, Command, PlayerCommand, Scenario, Target, UnitKind};
use chassis::grid::TilePos;

#[test]
fn last_worker_wreck_survives_provisional_site_cleanup_and_refund() {
    for provisional in [false, true] {
        let mut state = Scenario::skirmish().build().unwrap();
        let worker = state
            .units
            .iter()
            .find(|u| u.player == PlayerId(0) && u.kind.stats().harvest.is_some())
            .unwrap()
            .id;
        let tile = state.unit(worker).unwrap().tile();
        let value = state.unit(worker).unwrap().kind.stats().cost * crate::stats::WRECK_VALUE_NUM
            / crate::stats::WRECK_VALUE_DEN;
        let site = if provisional {
            state.place_provisional_site(PlayerId(0), BuildingKind::Turret, tile)
        } else {
            state.place_site(PlayerId(0), BuildingKind::Turret, tile)
        };
        let cost = BuildingKind::Turret.base_stats().construction.unwrap().cost;
        state.player_mut(PlayerId(0)).scrap -= cost;
        let bank = state.player(PlayerId(0)).scrap;
        let unit = state.unit_mut(worker).unwrap();
        unit.order = Order::Build { site };
        unit.hp = 0;
        let mut events = Vec::new();
        super::super::cleanup(&mut state, &[], &mut events);
        cancel_abandoned(&mut state, &mut events);
        assert!(state.unit(worker).is_none());
        assert!(state.building(site).is_none());
        assert_eq!(state.player(PlayerId(0)).scrap, bank + cost);
        assert_eq!(
            state.map.wreck_at(tile),
            if provisional { value } else { 0 }
        );
    }
}

#[test]
fn paid_site_activation_does_not_recheck_lost_prerequisites() {
    let mut state = Scenario::skirmish().build().unwrap();
    let worker = state
        .units
        .iter()
        .find(|u| u.player == PlayerId(0) && u.kind.stats().harvest.is_some())
        .unwrap()
        .id;
    let kind = BuildingKind::Airworks;
    let anchor = (0..state.map.height())
        .flat_map(|y| (0..state.map.width()).map(move |x| TilePos::new(x, y)))
        .find(|&tile| {
            state
                .place_refusal_except(PlayerId(0), kind, tile, None)
                .is_none()
        })
        .unwrap();
    assert!(!state.prerequisites_met(PlayerId(0), kind));
    let site = state.place_provisional_site(PlayerId(0), kind, anchor);
    state.unit_mut(worker).unwrap().order = Order::Found { kind, anchor };
    state.refresh_vision();
    assert!(
        state
            .building(site)
            .unwrap()
            .tiles()
            .all(|t| state.can_see(PlayerId(0), t))
    );
    let bank = state.player(PlayerId(0)).scrap;
    let mut events = Vec::new();
    reveal(&mut state, &mut events);
    assert!(!state.building(site).unwrap().provisional());
    assert_eq!(state.unit(worker).unwrap().order, Order::Build { site });
    assert_eq!(state.player(PlayerId(0)).scrap, bank);
    assert!(events.is_empty());
}

#[test]
fn a_lethal_hit_to_the_last_worker_refunds_its_unstarted_site() {
    let mut state = Scenario::skirmish().build().unwrap();
    let worker = state
        .units
        .iter()
        .find(|u| u.player == PlayerId(0) && u.kind.stats().harvest.is_some())
        .unwrap()
        .id;
    let tile = state.unit(worker).unwrap().tile();
    let shooter = state.spawn_unit(PlayerId(1), UnitKind::Sentinel, tile.offset(1, 0).center());
    let anchor = TilePos::new(state.map.width() - 5, 2);
    let site = state.place_provisional_site(PlayerId(0), BuildingKind::Turret, anchor);
    let worker_state = state.unit_mut(worker).unwrap();
    worker_state.hp = 1;
    worker_state.order = Order::Found {
        kind: BuildingKind::Turret,
        anchor,
    };
    state.refresh_vision();
    let cost = BuildingKind::Turret.base_stats().construction.unwrap().cost;
    state.player_mut(PlayerId(0)).scrap -= cost;
    let bank = state.player(PlayerId(0)).scrap;
    let mut report = state.tick(&[PlayerCommand {
        player: PlayerId(1),
        command: Command::Attack {
            units: vec![shooter],
            target: Target::Unit(worker).into(),
            queue: false,
        },
    }]);
    for _ in 0..120 {
        if state.unit(worker).is_none() {
            break;
        }
        report = state.tick(&[]);
    }
    assert!(
        state.unit(worker).is_none(),
        "the volley kills the last worker"
    );
    assert!(state.building(site).is_none());
    assert_eq!(state.player(PlayerId(0)).scrap, bank + cost);
    assert_eq!(report.events.iter().filter(|e| matches!(e, Event::BuildCancelled { building, refund, .. } if *building == site && *refund == cost)).count(), 1);
    state.validate_invariants().unwrap();
}
