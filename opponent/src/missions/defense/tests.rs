use super::*;
use oxide_sim::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{Faction, PlayerId, Scenario};

/// West's view of a 20 by 9 field, starts at (2, 1) and (17, 1), with
/// `units` as owner, kind and tile, and their ids in that order.
fn viewed(units: &[(u8, UnitKind, i32, i32)]) -> (ObservationData, Vec<UnitId>) {
    let map: Vec<String> = (0..9)
        .map(|row| {
            let mut tiles: Vec<char> = ".".repeat(20).chars().collect();
            if row == 1 {
                tiles[2] = '1';
                tiles[17] = '2';
            }
            tiles.into_iter().collect()
        })
        .collect();
    let scenario = Scenario {
        mode: ScenarioMode::Match,
        name: "defense".into(),
        seed: 1,
        map,
        players: [Faction::Ferrous, Faction::Cupric]
            .into_iter()
            .map(|faction| PlayerSpec {
                name: format!("{faction:?}"),
                faction,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units: units
            .iter()
            .map(|&(player, kind, x, y)| UnitSpec { player, kind, x, y })
            .collect(),
        buildings: Vec::new(),
        meta: None,
    };
    let state = scenario.build().unwrap();
    let ids = units
        .iter()
        .map(|&(_, _, x, y)| {
            state
                .units()
                .iter()
                .find(|unit| unit.tile() == TilePos::new(x, y))
                .unwrap()
                .id
        })
        .collect();
    (ObservationData::fog_honest(&state, PlayerId(0)), ids)
}

#[test]
fn a_gun_the_foundry_no_longer_faces_is_lost_but_focus_fire_is_kept() {
    let (observation, ids) = viewed(&[
        (0, UnitKind::Sentinel, 3, 4),
        (1, UnitKind::Bombard, 6, 4),
        (1, UnitKind::Sentinel, 6, 6),
    ]);
    let (gun, raider) = (ids[1], ids[2]);
    assert!(observation.enemy_units.len() == 2, "premise: both in sight");
    assert!(lost(&observation, None, Some(gun)));
    assert!(!lost(&observation, None, Some(raider)));
    assert!(lost(&observation, None, Some(UnitId(999))));
    assert!(!lost(&observation, Some(gun), Some(gun)));
}

#[test]
fn defenders_refused_an_order_are_not_counted_as_sent() {
    let (observation, ids) = viewed(&[
        (0, UnitKind::Sentinel, 3, 4),
        (0, UnitKind::Flakhound, 3, 5),
        (1, UnitKind::Bombard, 6, 4),
    ]);
    let (sentinel, flakhound, gun) = (ids[0], ids[1], ids[2]);
    let goal = TilePos::new(6, 4);
    let sent = |allowance| {
        let mut ledger = Ledger::new(PlayerId(0), 0, allowance);
        dispatch(
            &observation,
            &mut ledger,
            vec![sentinel, flakhound],
            goal,
            Pursuit::Gun(gun),
        )
    };
    let mut both = vec![sentinel, flakhound];
    both.sort_unstable();
    assert_eq!(sent(2), both);
    assert_eq!(sent(1), [sentinel], "the anti-air hunt was refused");
    assert!(sent(0).is_empty());
}
