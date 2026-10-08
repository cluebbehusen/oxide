use super::*;
use crate::UnitKind;
use crate::scenario::UnitSpec;

#[test]
fn automatic_radar_uses_a_secondary_solution_without_pursuit() {
    let mut scenario = crate::Scenario::skirmish();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 5,
            y: 7,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Gnat,
            x: 14,
            y: 8,
        },
    ];
    let mut rows = vec![vec!['.'; 40]; 30];
    rows[1][1] = '1';
    rows[27][37] = '2';
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    scenario.buildings = vec![crate::scenario::BuildingSpec {
        player: 0,
        kind: crate::BuildingKind::Array,
        x: 5,
        y: 16,
    }];
    let mut state = scenario.build().unwrap();
    // Retain radar-only observations to exercise reach beyond the current roster's sight.
    state.units[0].pos = TilePos::new(12, 6).center();
    state.units[0].heading = 32;
    state.units[1].pos = TilePos::new(30, 20).center();
    let gun = state.units[0].id;
    let start = state.units[0].pos;
    assert!(
        radar_in_range(
            &state,
            PlayerId(0),
            start,
            Domain::Ground,
            &UnitKind::Sentinel.stats().weapons[0]
        )
        .is_none()
    );
    let mut index = super::super::super::super::spatial::UnitIndex::new();
    index.rebuild(&state.units);
    let mut events = Vec::new();
    let mut hits = Vec::new();
    let mut launches = Vec::new();
    assert!(automatic_radar(
        &mut state,
        &index,
        gun,
        false,
        &mut events,
        &mut hits,
        &mut launches,
        &mut None
    ));
    assert!(events.iter().any(|event| matches!(event,
        Event::AttackHit { attacker, weapon: 1, target: None, .. } if *attacker == gun)));
    assert_eq!(state.units[0].cooldowns[0], 0);
    assert_eq!(
        state.units[0].cooldowns[1],
        UnitKind::Sentinel.stats().weapons[1].cooldown_ticks
    );
    assert_eq!(state.units[0].pos, start);
    assert_eq!(state.units[0].order, Order::Idle);
    assert!(state.units[0].path.is_none());
}

#[test]
fn automatic_contact_yields_to_sight_but_explicit_contact_keeps_preference() {
    let mut scenario = crate::Scenario::skirmish();
    let mut rows = vec![vec!['.'; 40]; 30];
    rows[1][1] = '1';
    rows[27][37] = '2';
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    scenario.buildings = vec![crate::scenario::BuildingSpec {
        player: 0,
        kind: crate::BuildingKind::Array,
        x: 5,
        y: 16,
    }];
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Avalanche,
            x: 5,
            y: 7,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 14,
            y: 8,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 10,
            y: 7,
        },
    ];
    for pursue in [false, true] {
        let mut state = scenario.build().unwrap();
        let gun = state.units[0].id;
        let visible = state.units[2].id;
        let target = AttackTarget::Contact(
            state
                .vision(PlayerId(0))
                .tracks()
                .iter()
                .find(|track| track.visible_unit.is_none())
                .unwrap()
                .id,
        );
        state.units[0].order = Order::Attack {
            target,
            resume: None,
            pursue,
        };
        state.tick(&[]);
        let Order::Attack {
            target: current, ..
        } = state.unit(gun).unwrap().order
        else {
            panic!("attack must remain active");
        };
        if pursue {
            assert_eq!(current, target);
        } else {
            assert_eq!(
                state.attack_view(PlayerId(0), current).unwrap().entity,
                Some(Target::Unit(visible))
            );
        }
        state.validate_invariants().unwrap();
    }
}

fn impact_scene() -> State {
    let mut scenario = crate::Scenario::skirmish();
    let mut rows = vec![vec!['.'; 24]; 18];
    rows[1][1] = '1';
    rows[15][21] = '2';
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    scenario.buildings.clear();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 5,
            y: 6,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 8,
            y: 6,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Gnat,
            x: 8,
            y: 7,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 9,
            y: 6,
        },
    ];
    scenario.build().unwrap()
}

#[test]
fn blind_hitscan_selects_one_eligible_occupant_and_splash_never_duplicates_it() {
    let mut state = impact_scene();
    let center = TilePos::new(8, 6).center();
    let shooter = state.units[0].id;
    let first = state.units[1].id;
    let air = state.units[2].id;
    let second = state.units[3].id;
    state.units[1].pos = center + Vec2Fx::new(Fx::lit("0.2"), Fx::ZERO);
    state.units[2].pos = center;
    state.units[3].pos = center - Vec2Fx::new(Fx::lit("0.2"), Fx::ZERO);
    let view = AttackView {
        entity: None,
        position: center,
        footprint: None,
        domain: None,
        velocity: Vec2Fx::ZERO,
    };
    let mut hits = Vec::new();
    let mut weapon = UnitKind::Sentinel.stats().weapons[0];
    let from = state.units[0].pos;
    buffer_blind(
        &state,
        (Target::Unit(shooter), PlayerId(0)),
        from,
        center,
        view,
        &weapon,
        &mut hits,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].victim, Target::Unit(first));
    weapon.splash = Some(Fx::ONE);
    hits.clear();
    buffer_blind(
        &state,
        (Target::Unit(shooter), PlayerId(0)),
        from,
        center,
        view,
        &weapon,
        &mut hits,
    );
    assert_eq!(
        hits.iter().map(|h| h.victim).collect::<Vec<_>>(),
        vec![Target::Unit(first), Target::Unit(second)]
    );
    hits.clear();
    weapon = UnitKind::Sentinel.stats().weapons[1];
    buffer_blind(
        &state,
        (Target::Unit(shooter), PlayerId(0)),
        from,
        center,
        view,
        &weapon,
        &mut hits,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].victim, Target::Unit(air));
    hits.clear();
    let empty = AttackView {
        position: TilePos::new(12, 6).center(),
        ..view
    };
    buffer_blind(
        &state,
        (Target::Unit(shooter), PlayerId(0)),
        from,
        empty.position,
        empty,
        &weapon,
        &mut hits,
    );
    assert!(hits.is_empty());
}
