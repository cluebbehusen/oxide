//! Every building, upgrade and unit the seat can buy, each staged where it
//! buys it. Army units are covered by the composition test that every one of
//! them gets its turn; the rest are here.

use super::*;
use crate::defenses;
use crate::investments::{ADOPT, Investment, Situation, candidates};
use crate::memory::Memory;
use crate::profile::PersonalityTraits;
use oxide_sim::observation::ObservationData;

fn building(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

/// What West would save for on `scenario` at `tick` once it scored best,
/// with its economy up, no army to speak of, and every trait high: every
/// investment that scores enough to be adopted.
fn offered(scenario: &Scenario, state: &State, tick: u64) -> Vec<Investment> {
    let mut observation = ObservationData::fog_honest(state, PlayerId(0));
    observation.tick = tick;
    let model = map(scenario);
    let mut memory = Memory::default();
    memory.observe(&observation);
    let situation = Situation {
        observation: &observation,
        map: &model,
        memory: &memory,
        traits: PersonalityTraits {
            air: 100,
            siege: 100,
            support: 100,
            fortification: 100,
            greed: 100,
            guile: 100,
        },
        saturation: 1_000,
        income: 1_000,
        depletion: 0,
        pull: Vec::new(),
        exposed: true,
        stakes: defenses::Stakes::default(),
        severed: false,
        wanted: vec![
            crate::composition::Role::Line,
            crate::composition::Role::Siege,
            crate::composition::Role::AntiAir,
            crate::composition::Role::AirStrike,
        ],
        units: Vec::new(),
        waiting: None,
    };
    candidates(&situation)
        .into_iter()
        .filter(|candidate| candidate.score >= ADOPT)
        .map(|candidate| candidate.investment)
        .collect()
}

/// The building kinds and upgrade rungs among `investments`, each rung as
/// its building's kind and the tier it reaches.
fn bought(state: &State, investments: &[Investment]) -> Vec<(BuildingKind, u8)> {
    investments
        .iter()
        .filter_map(|investment| match *investment {
            Investment::Tech(kind) | Investment::Capacity(kind) => Some((kind, 0)),
            Investment::Defense { kind, .. } => Some((kind, 0)),
            Investment::Expansion(_) => Some((BuildingKind::Foundry, 0)),
            Investment::Extractor(_) => Some((BuildingKind::Extractor, 0)),
            Investment::Reclaimer => Some((BuildingKind::Reclaimer, 0)),
            Investment::Unit(_) => None,
            Investment::Upgrade { building, tier } => state
                .buildings()
                .iter()
                .find(|each| each.id == building)
                .map(|each| (each.kind, tier)),
        })
        .collect()
}

/// `state` with every West `kind` at `tier`.
fn tiered(state: &State, kind: BuildingKind, tier: u8) -> State {
    let mut value = serde_json::to_value(state).unwrap();
    let name = serde_json::to_value(kind).unwrap();
    for entry in value["buildings"].as_array_mut().unwrap() {
        if entry["player"] == 0 && entry["kind"] == name {
            entry["tier"] = tier.into();
        }
    }
    serde_json::from_value(value).unwrap()
}

/// West units of `kind` trained by the decision on `scenario`, played to
/// `tick`.
fn trained(scenario: &Scenario, tick: u64, kind: UnitKind) -> bool {
    let mut state = scenario.build().unwrap();
    if tick > 0 {
        advance_to(&mut state, tick, &[]);
    }
    trains(&seat_with(scenario, 0, thrifty()).act(&state, &mut OwnEvents::default()))
        .iter()
        .any(|(_, each)| *each == kind)
}

#[test]
fn every_building_and_upgrade_is_bought_somewhere() {
    let mut covered: Vec<(BuildingKind, u8)> = Vec::new();
    let mut offer = |scenario: &Scenario, state: &State, tick: u64| {
        covered.extend(bought(state, &offered(scenario, state, tick)));
    };

    // Tech, an Extractor on a free frame, and a Turret, Bastion, Array and
    // Flak Turret against the hostile start and a known enemy Airworks.
    let mut home = arena(0);
    home.map[1].replace_range(6..7, "E");
    home.units.extend([harvester(0, 5, 7), harvester(0, 4, 7)]);
    home.buildings
        .push(building(1, BuildingKind::Airworks, 10, 8));
    let state = home.build().unwrap();
    offer(&home, &state, 600);

    // An expansion Foundry.
    let mut frontier = arena(0);
    frontier.map = super::expansion::FRONTIER.map(str::to_owned).to_vec();
    frontier.units = vec![harvester(0, 4, 3), harvester(0, 5, 3)];
    let state = frontier.build().unwrap();
    offer(&frontier, &state, 3_000);

    // Reclaimers, Refineries, a Barricade in front of the Turret, Scuttle
    // Charges, and the base-tier upgrades once a Fabricator stands, with an
    // enemy in sight beyond the defenses' reach.
    let mut built = home.clone();
    built.units.extend([
        unit(1, UnitKind::Sentinel, 18, 8),
        unit(0, UnitKind::Kestrel, 17, 8),
    ]);
    built.buildings.extend([
        building(0, BuildingKind::Fabricator, 3, 1),
        building(0, BuildingKind::Reclaimer, 1, 9),
        building(0, BuildingKind::Turret, 7, 5),
        building(0, BuildingKind::FlakTurret, 7, 7),
        building(0, BuildingKind::Array, 9, 3),
    ]);
    let state = built.build().unwrap();
    offer(&built, &state, 3_000);

    // Bulwarks and Deep Arrays once a Crucible stands.
    let mut crucible = built.clone();
    crucible
        .buildings
        .push(building(0, BuildingKind::Crucible, 12, 2));
    let state = tiered(&crucible.build().unwrap(), BuildingKind::Turret, 1);
    offer(&crucible, &state, 3_000);

    // A Repair Bay where the wounded gather.
    let mut hurt = arena(0);
    let squad = [
        (2, 8),
        (3, 8),
        (4, 8),
        (5, 8),
        (2, 10),
        (3, 10),
        (4, 10),
        (5, 10),
    ];
    for (x, y) in squad {
        hurt.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
    let mut state = hurt.build().unwrap();
    for (x, y) in squad {
        state = wounded(&state, at(&state, x, y), 1);
    }
    offer(&hurt, &state, 3_000);

    let missing: Vec<(BuildingKind, u8)> = BuildingKind::ALL
        .into_iter()
        .flat_map(|kind| (0..kind.tiers().len() as u8).map(move |tier| (kind, tier)))
        .filter(|rung| !covered.contains(rung))
        .collect();
    assert!(missing.is_empty(), "never bought: {missing:?}");
}

#[test]
fn every_unit_outside_the_army_is_trained_somewhere() {
    let mut rows: Vec<(UnitKind, bool)> = Vec::new();

    let mut armed = arena(200);
    armed.units.extend(standing_army(0));
    rows.push((UnitKind::Harvester, trained(&armed, 0, UnitKind::Harvester)));

    let mut excavating = arena(1_000);
    excavating.units.extend(standing_army(0));
    excavating
        .buildings
        .push(building(0, BuildingKind::Fabricator, 3, 1));
    rows.push((
        UnitKind::Excavator,
        trained(&excavating, 0, UnitKind::Excavator),
    ));

    let mut scouting = field();
    scouting.players[0].scrap = 1_000;
    rows.push((
        UnitKind::Scuttler,
        trained(&scouting, 1_800, UnitKind::Scuttler),
    ));
    scouting.buildings.extend([
        building(0, BuildingKind::Fabricator, 6, 16),
        building(0, BuildingKind::Airworks, 10, 16),
    ]);
    rows.push((
        UnitKind::Kestrel,
        trained(&scouting, 1_800, UnitKind::Kestrel),
    ));
    scouting.players[0].faction = Faction::Cupric;
    rows.push((UnitKind::Gnat, trained(&scouting, 1_800, UnitKind::Gnat)));

    let mut lifting = super::lift::strait();
    lifting.units.retain(|unit| unit.kind != UnitKind::Skyhook);
    lifting.players[0].scrap = 1_000;
    rows.push((UnitKind::Skyhook, trained(&lifting, 0, UnitKind::Skyhook)));

    let mut sapping = field();
    sapping.players[0].scrap = 1_000;
    sapping.buildings.extend([
        building(0, BuildingKind::Fabricator, 6, 16),
        building(1, BuildingKind::Turret, 9, 9),
    ]);
    rows.push((UnitKind::Sapper, trained(&sapping, 0, UnitKind::Sapper)));

    let mut tending = arena(400);
    tending
        .buildings
        .push(building(0, BuildingKind::Fabricator, 3, 1));
    let squad = [
        (2, 8),
        (3, 8),
        (4, 8),
        (5, 8),
        (2, 10),
        (3, 10),
        (4, 10),
        (5, 10),
    ];
    for (x, y) in squad {
        tending.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
    let mut state = tending.build().unwrap();
    for (x, y) in squad {
        state = wounded(&state, at(&state, x, y), 1);
    }
    let tender = trains(&seat(&tending, 0).act(&state, &mut OwnEvents::default()))
        .iter()
        .any(|(_, kind)| *kind == UnitKind::Tender);
    rows.push((UnitKind::Tender, tender));

    let failed: Vec<UnitKind> = rows
        .iter()
        .filter(|(_, trained)| !trained)
        .map(|(kind, _)| *kind)
        .collect();
    assert!(failed.is_empty(), "not trained where staged: {failed:?}");
    let missing: Vec<UnitKind> = UnitKind::ALL
        .into_iter()
        .filter(|kind| crate::composition::role(*kind).is_none())
        .filter(|kind| !rows.iter().any(|(row, _)| row == kind))
        .collect();
    assert!(missing.is_empty(), "no staging for {missing:?}");
}
