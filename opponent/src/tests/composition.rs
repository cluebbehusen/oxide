use super::*;
use crate::investments::{self, Investment, Situation};
use crate::memory::Memory;
use crate::{PersonalityTraits, composition};

/// The arena with a built west Fabricator and a large west bank, plus
/// whatever the enemy stages in sight of the west base.
fn armed(
    enemy_units: &[(UnitKind, i32, i32)],
    enemy_buildings: &[(BuildingKind, i32, i32)],
) -> Scenario {
    let mut scenario = arena(2_000);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 8,
        y: 8,
    });
    scenario
        .units
        .extend(enemy_units.iter().map(|(kind, x, y)| UnitSpec {
            player: 1,
            kind: *kind,
            x: *x,
            y: *y,
        }));
    scenario
        .buildings
        .extend(enemy_buildings.iter().map(|(kind, x, y)| BuildingSpec {
            player: 1,
            kind: *kind,
            x: *x,
            y: *y,
        }));
    scenario
}

fn fabricator(state: &State) -> BuildingId {
    state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0) && building.kind == BuildingKind::Fabricator
        })
        .unwrap()
        .id
}

/// What the west Fabricator queues on the first decision.
fn fabricator_trains(scenario: &Scenario) -> Vec<UnitKind> {
    let state = scenario.build().unwrap();
    let at = fabricator(&state);
    trains(&seat(scenario, 0).act(&state, &mut OwnEvents::default()))
        .into_iter()
        .filter(|(building, _)| *building == at)
        .map(|(_, kind)| kind)
        .collect()
}

fn anti_air(kinds: &[UnitKind]) -> bool {
    kinds
        .iter()
        .any(|kind| composition::role(*kind) == Some(composition::Role::AntiAir))
}

#[test]
fn seen_enemy_air_brings_anti_air_in_proportion() {
    let darters = [(UnitKind::Darter, 6, 8), (UnitKind::Darter, 7, 9)];
    assert!(anti_air(&fabricator_trains(&armed(&darters, &[]))));
    assert!(
        !anti_air(&fabricator_trains(&armed(&[], &[]))),
        "no air, no anti-air"
    );

    let mut covered = armed(&darters, &[]);
    covered.units.extend([
        UnitSpec {
            player: 0,
            kind: UnitKind::Flakhound,
            x: 9,
            y: 6,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Flakhound,
            x: 10,
            y: 6,
        },
    ]);
    assert!(
        !anti_air(&fabricator_trains(&covered)),
        "two Flakhounds already answer two Darters"
    );
}

#[test]
fn a_seen_enemy_airworks_raises_anti_air_before_any_flyer() {
    let airworks = [(BuildingKind::Airworks, 8, 2)];
    assert!(anti_air(&fabricator_trains(&armed(&[], &airworks))));

    let target = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        seat(scenario, 0)
            .act_traced(&state, &mut OwnEvents::default())
            .1
            .unwrap()
            .target
            .map(|target| target.investment)
    };
    let mut bare = arena(200);
    assert_eq!(target(&bare), None, "premise: nothing worth saving for yet");
    bare.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Airworks,
        x: 8,
        y: 2,
    });
    assert_eq!(
        target(&bare),
        Some(Investment::Tech(BuildingKind::Fabricator)),
        "the cheapest anti-air producer"
    );
}

#[test]
fn known_defenses_bring_siege() {
    let siege = |kinds: &[UnitKind]| {
        kinds
            .iter()
            .any(|kind| composition::role(*kind) == Some(composition::Role::Siege))
    };
    assert!(siege(&fabricator_trains(&armed(
        &[],
        &[(BuildingKind::Turret, 12, 9)]
    ))));
    assert!(!siege(&fabricator_trains(&armed(&[], &[]))));
}

#[test]
fn raiders_are_never_line_units() {
    for kind in UnitKind::ALL {
        if composition::role(kind) == Some(composition::Role::Line) {
            assert_ne!(kind, UnitKind::Scuttler);
        }
    }
    let kinds = fabricator_trains(&armed(&[], &[]));
    assert!(!kinds.contains(&UnitKind::Scuttler));
}

#[test]
fn busy_producers_ask_for_another() {
    let mut state = armed(&[], &[]).build().unwrap();
    let busy = PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: fabricator(&state),
            kind: UnitKind::Warden,
        },
    };
    state.tick(&[busy]);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let wants = |income: u32| {
        investments::candidates(&Situation {
            observation: &observation,
            traits: PersonalityTraits {
                air: 50,
                siege: 50,
                support: 50,
                fortification: 50,
                greed: 50,
                guile: 50,
            },
            saturation: 1_000,
            income,
            depletion: 0,
            pull: Vec::new(),
        })
        .into_iter()
        .map(|candidate| candidate.investment)
        .collect::<Vec<_>>()
    };
    let another = Investment::Capacity(BuildingKind::Fabricator);
    assert!(wants(600).contains(&another));
    assert!(
        !wants(599).contains(&another),
        "income for one more is too low"
    );
}

#[test]
fn enemy_units_fade_and_vanish_when_their_spot_is_seen_empty() {
    let state = armed(&[(UnitKind::Sentinel, 6, 9)], &[]).build().unwrap();
    let seen = ObservationData::fog_honest(&state, PlayerId(0));
    let mut memory = Memory::default();
    memory.observe(&seen);
    assert_eq!(memory.units().len(), 1);
    assert_eq!(memory.units()[0].confidence(seen.tick), 1_000);

    let mut hidden = seen.clone();
    hidden.enemy_units.clear();
    hidden.tick = 300;
    let tile = memory.units()[0].tile;
    let index = usize::try_from(tile.y * hidden.map_width + tile.x).unwrap();
    hidden.visible[index] = false;
    let mut faded = memory.clone();
    faded.observe(&hidden);
    assert_eq!(faded.units()[0].confidence(300), 500);
    hidden.tick = 600;
    faded.observe(&hidden);
    assert!(faded.units().is_empty(), "forgotten after 600 ticks");

    let mut empty = seen;
    empty.enemy_units.clear();
    empty.tick = 12;
    memory.observe(&empty);
    assert!(memory.units().is_empty(), "its spot is in sight and empty");
}

#[test]
fn a_producer_trains_the_best_unit_it_can_afford() {
    let clustered = [
        (UnitKind::Harvester, 10, 9),
        (UnitKind::Harvester, 11, 9),
        (UnitKind::Harvester, 10, 10),
        (UnitKind::Harvester, 11, 10),
    ];
    let mut scenario = armed(&clustered, &[(BuildingKind::Turret, 12, 9)]);
    scenario.players[0].scrap = 150 + UnitKind::Sentinel.stats().cost;
    let mut state = scenario.build().unwrap();
    let busy = PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: foundries(&state, PlayerId(0))[0],
            kind: UnitKind::Sentinel,
        },
    };
    advance_to(&mut state, 12, &[busy]);
    let at = fabricator(&state);
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let trained: Vec<UnitKind> = trains(&commands)
        .into_iter()
        .filter(|(building, _)| *building == at)
        .map(|(_, kind)| kind)
        .collect();
    assert!(
        UnitKind::Bombard.stats().cost > 150,
        "premise: the splash unit is out of reach"
    );
    assert_eq!(trained, [UnitKind::Lancer]);
}
