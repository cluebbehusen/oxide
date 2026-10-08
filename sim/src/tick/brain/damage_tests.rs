use super::*;

#[test]
fn a_lethal_volley_precedes_construction_triggers_and_completion() {
    use crate::{BuildingKind, PlayerId};
    use chassis::grid::TilePos;
    for kill_mine in [false, true] {
        let mut state = crate::Scenario::skirmish().build().unwrap();
        let anchor = TilePos::new(12, 8);
        let mine = state.place_building(PlayerId(1), BuildingKind::ScuttleCharge, anchor);
        let site = state.place_site(PlayerId(0), BuildingKind::Barricade, anchor);
        let victim = if kill_mine { mine } else { site };
        let hit = PendingHit::along(
            &state,
            Target::Unit(state.units[0].id),
            Target::Building(victim),
            state.building(victim).unwrap().hp,
            anchor.center(),
            anchor.center(),
        );
        let gain = PendingHpGain {
            starts: true,
            site,
            step: 1,
            completes: true,
            player: PlayerId(0),
            kind: BuildingKind::Barricade,
            paid: 0,
            repair_bay: None,
        };
        let mut events = Vec::new();
        resolve_hits(&mut state, &[hit], &[gain], &[], &[], &mut events);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ChargeDetonated { .. }))
        );
        assert_eq!(state.building(site).unwrap().built, kill_mine);
        assert_eq!(state.building(mine).unwrap().hp == 0, kill_mine);
    }
}

#[test]
fn damage_evidence_survives_same_tick_repair_and_excludes_zero_damage() {
    let mut state = crate::Scenario::skirmish().build().unwrap();
    let victim = state.units[0].id;
    let attacker = state.units.last().unwrap().id;
    state.units[0].hp -= 10;
    let (hp, player, pos) = {
        let unit = state.unit(victim).unwrap();
        (unit.hp, unit.player, unit.pos)
    };
    let hit = PendingHit::along(
        &state,
        Target::Unit(attacker),
        Target::Unit(victim),
        4,
        pos,
        pos,
    );
    let heal = PendingUnitHeal {
        unit: victim,
        step: 4,
        player,
        paid: 0,
        source: crate::event::UnitRepairSource::FieldWelder { unit: victim },
    };
    let mut events = Vec::new();
    resolve_hits(&mut state, &[hit], &[], &[heal], &[], &mut events);
    assert_eq!(state.unit(victim).unwrap().hp, hp);
    assert!(
        matches!(events.first(), Some(Event::DamageTaken { player: owner, pos: at }) if *owner == player && *at == pos)
    );
    assert!(events.iter().any(
        |event| matches!(event, Event::UnitRepaired { unit, amount: 4, .. } if *unit == victim)
    ));

    let hit = PendingHit::along(
        &state,
        Target::Unit(attacker),
        Target::Unit(victim),
        0,
        pos,
        pos,
    );
    events.clear();
    resolve_hits(&mut state, &[hit], &[], &[], &[], &mut events);
    assert!(events.is_empty());

    let building = &state.buildings[0];
    let (id, hp, player, pos) = (building.id, building.hp, building.player, building.center());
    let hit = PendingHit::along(
        &state,
        Target::Unit(attacker),
        Target::Building(id),
        4,
        pos,
        pos,
    );
    resolve_hits(&mut state, &[hit], &[], &[], &[], &mut events);
    assert_eq!(state.building(id).unwrap().hp, hp - 4);
    assert_eq!(events, vec![Event::DamageTaken { player, pos }]);
}
