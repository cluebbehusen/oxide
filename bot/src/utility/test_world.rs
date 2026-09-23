use super::*;
#[cfg(test)]
use crate::observation::ObservationData;
use oxide_sim::ids::PlayerId;
use oxide_sim::scenario::{PlayerSpec, Scenario};

pub(super) const WIDTH: i32 = 40;

pub(super) const HEIGHT: i32 = 24;

pub(super) const LEFT_HOME: TilePos = TilePos::new(4, 10);

pub(super) const RIGHT_HOME: TilePos = TilePos::new(34, 10);

pub(super) fn scenario_with(terrain: impl FnMut(TilePos) -> char) -> Scenario {
    scenario_with_starts(LEFT_HOME, RIGHT_HOME, terrain)
}

pub(super) fn scenario_with_starts(
    first: TilePos,
    second: TilePos,
    mut terrain: impl FnMut(TilePos) -> char,
) -> Scenario {
    let mut rows = Vec::new();
    for y in 0..HEIGHT {
        let mut row = Vec::new();
        for x in 0..WIDTH {
            let tile = TilePos::new(x, y);
            let authored = if tile == first {
                '1'
            } else if tile == second {
                '2'
            } else {
                terrain(tile)
            };
            row.push(authored as u8);
        }
        rows.push(String::from_utf8(row).expect("ASCII fixture row"));
    }
    Scenario {
        mode: Default::default(),
        name: "defense fixture".into(),
        seed: 7,
        map: rows,
        players: vec![
            PlayerSpec {
                name: "left".into(),
                faction: oxide_sim::state::Faction::Ferrous,
                team: None,
                scrap: 10_000,
                bot: true,
                bot_config: None,
            },
            PlayerSpec {
                name: "right".into(),
                faction: oxide_sim::state::Faction::Cupric,
                team: None,
                scrap: 10_000,
                bot: true,
                bot_config: None,
            },
        ],
        units: Vec::new(),
        buildings: Vec::new(),
        meta: None,
    }
}

pub(super) fn briefing() -> PublicMapBriefing {
    PublicMapBriefing::from_scenario(&scenario_with(|_| '.')).expect("briefing fixture")
}

pub(super) fn building(
    id: u32,
    player: PlayerId,
    kind: BuildingKind,
    anchor: TilePos,
) -> BuildingObs {
    crate::test_support::building(id, player, kind, anchor)
}

pub(super) fn unit(id: u32, player: PlayerId, kind: UnitKind, tile: TilePos) -> UnitObs {
    crate::test_support::unit(id, player, kind, tile)
}

pub(super) fn observation(me: PlayerId, home: TilePos) -> Observation {
    let worker_tile = if me == PlayerId(0) {
        home.offset(3, 3)
    } else {
        TilePos::new(WIDTH - 1 - (LEFT_HOME.x + 3), home.y + 3)
    };
    let mut worker = unit(1, me, UnitKind::Harvester, worker_tile);
    worker.idle = true;
    Observation::from_data(ObservationData {
        tick: 1_000,
        me,
        scrap: 10_000,
        map_width: WIDTH,
        map_height: HEIGHT,
        my_units: vec![worker],
        my_buildings: vec![building(0, me, BuildingKind::Foundry, home)],
        my_queues: vec![Vec::new()],
        visible: vec![false; (WIDTH * HEIGHT) as usize],
        explored: vec![true; (WIDTH * HEIGHT) as usize],
        faction: if me == PlayerId(0) {
            oxide_sim::state::Faction::Ferrous
        } else {
            oxide_sim::state::Faction::Cupric
        },
        ..crate::test_support::observation_data()
    })
}
