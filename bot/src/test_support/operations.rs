use super::{building_spec, player_spec, unit_spec};
use crate::observation::{BuildingObs, ObservationData, UnitObs};
use crate::{Observation, PersonalityTraits, ResolvedProfile, Specialty};
use chassis::grid::TilePos;
use oxide_sim::scenario::{BotDifficulty, BotStance, Scenario};
use oxide_sim::{BuildingKind, Faction, PlayerId, UnitKind};

pub(crate) const TEST_HOME: TilePos = TilePos::new(5, 15);
pub(crate) const TEST_TARGET: TilePos = TilePos::new(50, 15);

pub(crate) fn foundry_competition_profile() -> ResolvedProfile {
    ResolvedProfile {
        difficulty: BotDifficulty::Standard,
        stance: BotStance::Balanced,
        personality_seed: 1_616_304,
        primary: Specialty::Air,
        secondary: Specialty::Siege,
        traits: PersonalityTraits {
            air: 70,
            siege: 60,
            support: 35,
            fortification: 35,
            greed: 64,
            guile: 36,
        },
    }
}

pub(crate) fn bulk_lift_capacity_scenario() -> Scenario {
    let mut rows = vec![vec!['.'; 40]; 24];
    rows.first_mut().expect("map has a north edge").fill('#');
    rows.last_mut().expect("map has a south edge").fill('#');
    for row in &mut rows {
        row[0] = '#';
        row[39] = '#';
        row[20] = '~';
    }
    rows[11][2] = '1';
    rows[11][36] = '2';

    let mut units: Vec<_> = (0..8)
        .map(|index| unit_spec(0, UnitKind::Harvester, 4 + index, 8))
        .collect();
    units.extend(
        (0..40).map(|index| unit_spec(0, UnitKind::Sentinel, 3 + index % 14, 15 + index / 14)),
    );
    units.push(unit_spec(0, UnitKind::Kestrel, 31, 11));

    Scenario {
        mode: Default::default(),
        name: "brain bulk-lift capacity".into(),
        seed: 0x0A16_00C0,
        map: rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: vec![
            player_spec("Ferrous", Faction::Ferrous, 0, None),
            player_spec("Cupric", Faction::Cupric, 0, None),
        ],
        units,
        buildings: vec![
            building_spec(0, BuildingKind::Fabricator, 2, 3),
            building_spec(0, BuildingKind::Airworks, 6, 3),
            building_spec(0, BuildingKind::Crucible, 10, 3),
        ],
        meta: None,
    }
}

pub(crate) fn foundry_saving_lift_competition_scenario(scrap: u32) -> Scenario {
    let mut scenario = bulk_lift_capacity_scenario();
    scenario.name = "accepted Foundry saving competes with a bulk lift".into();
    scenario.players[0].scrap = scrap;
    let frame = TilePos::new(14, 11);
    let row = scenario
        .map
        .get_mut(frame.y as usize)
        .expect("the lift fixture contains the frame row");
    let mut bytes = row.as_bytes().to_vec();
    bytes[frame.x as usize] = b'E';
    *row = String::from_utf8(bytes).expect("the lift fixture map remains ASCII");
    scenario
        .buildings
        .push(building_spec(0, BuildingKind::Extractor, frame.x, frame.y));
    scenario
        .units
        .extend((0..7).map(|index| unit_spec(0, UnitKind::Skyhook, 3 + index, 20)));
    scenario
}

pub(crate) fn test_island_observation() -> Observation {
    let mut obs = Observation::from_data(ObservationData {
        tick: 0,
        map_width: 64,
        map_height: 32,
        enemy_buildings: vec![test_building(500, 1, BuildingKind::Foundry, TEST_TARGET)],
        visible: vec![true; 64 * 32],
        explored: vec![true; 64 * 32],
        known_rock: (0..32).map(|y| TilePos::new(32, y)).collect(),
        ..crate::test_support::observation_data()
    });
    obs.my_buildings.push(test_building(
        1,
        0,
        BuildingKind::Foundry,
        TEST_HOME.offset(-1, -1),
    ));
    obs.my_queues.push(Vec::new());
    obs
}

pub(crate) fn test_unit(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
    crate::test_support::unit(id, PlayerId(0), kind, tile)
}

pub(crate) fn test_building(
    id: u32,
    player: u8,
    kind: BuildingKind,
    anchor: TilePos,
) -> BuildingObs {
    crate::test_support::building(id, PlayerId(player), kind, anchor)
}
