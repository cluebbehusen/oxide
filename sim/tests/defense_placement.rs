//! GymBot-only static-defense placement: role identity without hidden
//! information or compass bias.

mod common;

use chassis::grid::TilePos;
use oxide_sim::bot::{Action, GymBot};
use oxide_sim::scenario::{BuildingSpec, UnitSpec};
use oxide_sim::{BuildingKind, Command, PlayerId, Scenario, UnitKind};

use common::{open_arena, unit};

fn set_tile(scenario: &mut Scenario, tile: TilePos, value: char) {
    let mut row: Vec<_> = scenario.map[tile.y as usize].chars().collect();
    row[tile.x as usize] = value;
    scenario.map[tile.y as usize] = row.into_iter().collect();
}

fn placement_scenario(units: Vec<UnitSpec>) -> Scenario {
    let mut scenario = open_arena(40, 20, units);
    scenario.players[0].scrap = 1_000;
    scenario.players[1].scrap = 1_000;
    scenario
}

fn with_remembered_scrap(state: oxide_sim::State, player: u8) -> oxide_sim::State {
    let width = state.map().width() as usize;
    let remembered: Vec<_> = state
        .map()
        .iter()
        .filter(|(_, tile)| tile.scrap > 0)
        .map(|(tile, value)| (tile.y as usize * width + tile.x as usize, value.scrap))
        .collect();
    let mut value = serde_json::to_value(state).unwrap();
    for explored in value["vision"][player as usize]["explored"]["cells"]
        .as_array_mut()
        .unwrap()
    {
        *explored = true.into();
    }
    for (index, amount) in remembered {
        value["vision"][player as usize]["remembered_scrap"]["cells"][index] = amount.into();
    }
    serde_json::from_value(value).unwrap()
}

fn build_anchor(
    state: &oxide_sim::State,
    player: u8,
    action: Action,
    kind: BuildingKind,
) -> TilePos {
    GymBot::new(PlayerId(player))
        .step(state, action)
        .into_iter()
        .find_map(|command| match command.command {
            Command::Build {
                kind: emitted,
                anchor,
                ..
            } if emitted == kind => Some(anchor),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{action:?} emitted no {kind:?} placement"))
}

#[test]
fn defense_placement_is_deterministic() {
    let mut scenario = placement_scenario(vec![unit(0, UnitKind::Harvester, 5, 5)]);
    set_tile(&mut scenario, TilePos::new(15, 8), 's');
    set_tile(&mut scenario, TilePos::new(22, 11), 's');
    let state = with_remembered_scrap(scenario.build().unwrap(), 0);

    for (action, kind) in [
        (Action::BuildTurret, BuildingKind::Turret),
        (Action::BuildFlak, BuildingKind::FlakTurret),
        (Action::BuildBastion, BuildingKind::Bastion),
    ] {
        assert_eq!(
            build_anchor(&state, 0, action, kind),
            build_anchor(&state, 0, action, kind),
            "{kind:?} placement must reproduce from the same observation"
        );
    }
}

#[test]
fn defense_placement_mirrors_between_physical_seats() {
    let mut scenario = placement_scenario(vec![
        unit(0, UnitKind::Harvester, 5, 5),
        unit(1, UnitKind::Harvester, 34, 14),
    ]);
    set_tile(&mut scenario, TilePos::new(12, 7), 's');
    set_tile(&mut scenario, TilePos::new(27, 12), 's');
    let state = scenario.build().unwrap();

    for (action, kind) in [
        (Action::BuildTurret, BuildingKind::Turret),
        (Action::BuildFlak, BuildingKind::FlakTurret),
        (Action::BuildBastion, BuildingKind::Bastion),
    ] {
        let west = build_anchor(&state, 0, action, kind);
        let east = build_anchor(&state, 1, action, kind);
        let (width, height) = kind.stats().size;
        assert_eq!(
            east,
            TilePos::new(
                state.map().width() - width - west.x,
                state.map().height() - height - west.y,
            ),
            "{kind:?} must use the same oriented tie-breaks from either seat"
        );
    }
}

#[test]
fn hidden_enemy_changes_do_not_move_a_defense() {
    let mut west = placement_scenario(vec![
        unit(0, UnitKind::Harvester, 5, 5),
        unit(1, UnitKind::Buzzard, 31, 14),
    ]);
    set_tile(&mut west, TilePos::new(14, 7), 's');
    let mut east = west.clone();
    east.units[1] = unit(1, UnitKind::Darter, 35, 10);

    let west = west.build().unwrap();
    let east = east.build().unwrap();
    assert_eq!(
        build_anchor(&west, 0, Action::BuildFlak, BuildingKind::FlakTurret),
        build_anchor(&east, 0, Action::BuildFlak, BuildingKind::FlakTurret),
        "unseen enemy kind and position cannot enter the placement score"
    );
}

#[test]
fn direct_fire_guards_salvage_while_artillery_stays_near_home() {
    let mut scenario = placement_scenario(vec![unit(0, UnitKind::Harvester, 5, 5)]);
    let salvage = TilePos::new(23, 10);
    set_tile(&mut scenario, salvage, 's');
    let state = with_remembered_scrap(scenario.build().unwrap(), 0);
    let home = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .unwrap()
        .anchor;

    let turret = build_anchor(&state, 0, Action::BuildTurret, BuildingKind::Turret);
    let bastion = build_anchor(&state, 0, Action::BuildBastion, BuildingKind::Bastion);
    assert!(
        turret.chebyshev(salvage) < bastion.chebyshev(salvage),
        "the cheap point defense belongs on the harvest line: {turret:?} vs {bastion:?}"
    );
    assert!(
        bastion.chebyshev(home) < turret.chebyshev(home),
        "the long gun belongs in the protected home envelope: {bastion:?} vs {turret:?}"
    );
}

#[test]
fn flak_leans_into_visible_air_while_ground_turrets_hold_the_line() {
    let mut scenario = placement_scenario(vec![
        unit(0, UnitKind::Harvester, 5, 5),
        unit(1, UnitKind::Buzzard, 9, 5),
    ]);
    let salvage = TilePos::new(22, 10);
    set_tile(&mut scenario, salvage, 's');
    let state = scenario.build().unwrap();
    let air = TilePos::new(9, 5);
    assert!(
        state.vision(PlayerId(0)).visible(air),
        "the fixture's air contact must be legitimate information"
    );

    let turret = build_anchor(&state, 0, Action::BuildTurret, BuildingKind::Turret);
    let flak = build_anchor(&state, 0, Action::BuildFlak, BuildingKind::FlakTurret);
    assert!(
        flak.chebyshev(air) < turret.chebyshev(air),
        "anti-air placement should answer the visible wing: {flak:?} vs {turret:?}"
    );
}

#[test]
fn a_second_emplacement_spreads_across_the_salvage_line() {
    let mut scenario = placement_scenario(vec![unit(0, UnitKind::Harvester, 5, 5)]);
    set_tile(&mut scenario, TilePos::new(14, 7), 's');
    set_tile(&mut scenario, TilePos::new(22, 7), 's');
    let baseline = with_remembered_scrap(scenario.build().unwrap(), 0);
    let first = build_anchor(&baseline, 0, Action::BuildTurret, BuildingKind::Turret);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: first.x,
        y: first.y,
    });
    let with_first = with_remembered_scrap(scenario.build().unwrap(), 0);
    let second = build_anchor(&with_first, 0, Action::BuildTurret, BuildingKind::Turret);

    assert_ne!(first, second);
    assert!(
        first.chebyshev(second) >= 4,
        "spacing should buy a second coverage cell, not a turret clump: {first:?}, {second:?}"
    );
}
