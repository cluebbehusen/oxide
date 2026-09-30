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
fn known_defenses_bring_siege_or_sappers() {
    let siege = |kinds: &[UnitKind]| {
        kinds.iter().any(|kind| {
            *kind == UnitKind::Sapper || composition::role(*kind) == Some(composition::Role::Siege)
        })
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
fn working_producers_ask_for_another_while_unspent_income_and_need_last() {
    let scenario = armed(&[], &[]);
    let mut state = scenario.build().unwrap();
    let model = map(&scenario);
    let memory = Memory::default();
    let busy = PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: fabricator(&state),
            kind: UnitKind::Warden,
        },
    };
    state.tick(&[busy]);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let wants = |income: u32, wanted: Vec<composition::Role>| {
        investments::candidates(&Situation {
            observation: &observation,
            map: &model,
            memory: &memory,
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
            exposed: false,
            wanted,
        })
        .into_iter()
        .map(|candidate| candidate.investment)
        .collect::<Vec<_>>()
    };
    // What the working Fabricator spends a minute on its Warden.
    let stats = UnitKind::Warden.stats();
    let spends = stats.cost * 20 * 60 / stats.train_ticks;
    let another = Investment::Capacity(BuildingKind::Fabricator);
    let line = || vec![composition::Role::Line];
    assert!(
        wants(2 * spends, line()).contains(&another),
        "income left over keeps a second Fabricator as busy"
    );
    assert!(
        !wants(2 * spends - 1, line()).contains(&another),
        "but not just short of it"
    );
    assert!(
        !wants(4 * spends, Vec::new()).contains(&another),
        "and not while nothing it trains is wanted"
    );
    assert!(
        !wants(4 * spends, line()).contains(&Investment::Capacity(BuildingKind::Foundry)),
        "an idle Foundry is no reason for another"
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

#[test]
fn the_most_wanted_role_takes_the_scrap_before_a_nearer_producer() {
    // Two Foundries nearer home than the Fabricator, their Harvesters enough,
    // and a bank that leaves scrap for one unit beside the saving target.
    let mut scenario = armed(&[], &[]);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 3,
        y: 8,
    });
    scenario.units.extend(workforce(0));
    for player in &mut scenario.players {
        player.scrap = 200;
    }
    let state = scenario.build().unwrap();
    // Two Darters seen over the East base: anti-air is wanted, and nothing
    // is in sight to defend against.
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["memory"]["units"] = (0..2)
        .map(|index: u32| {
            serde_json::json!({
                "id": 1_000 + index,
                "kind": "darter",
                "tile": {"x": 20, "y": 2 + index},
                "seen": state.current_tick(),
            })
        })
        .collect();
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let trained = trains(&opponent.act(&state, &mut OwnEvents::default()));
    let [(at, kind)] = trained[..] else {
        panic!("{trained:?}");
    };
    assert_eq!(
        at,
        fabricator(&state),
        "the Foundries nearer home do not spend the scrap on line units first"
    );
    assert!(anti_air(&[kind]), "{kind:?}");
}

#[test]
fn a_working_foundry_or_crucible_asks_for_another_at_home() {
    let mut scenario = armed(&[], &[]);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Crucible,
        x: 12,
        y: 8,
    });
    let mut state = scenario.build().unwrap();
    let model = map(&scenario);
    let memory = Memory::default();
    let at = |kind: BuildingKind| {
        state
            .buildings()
            .iter()
            .find(|building| building.player == PlayerId(0) && building.kind == kind)
            .unwrap()
            .id
    };
    let train = |building, kind| PlayerCommand {
        player: PlayerId(0),
        command: Command::Train { building, kind },
    };
    let orders = [
        train(at(BuildingKind::Foundry), UnitKind::Sentinel),
        train(at(BuildingKind::Crucible), UnitKind::Breaker),
    ];
    state.tick(&orders);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let wants = |wanted: Vec<composition::Role>| {
        investments::candidates(&Situation {
            observation: &observation,
            map: &model,
            memory: &memory,
            traits: PersonalityTraits {
                air: 50,
                siege: 50,
                support: 50,
                fortification: 50,
                greed: 50,
                guile: 50,
            },
            saturation: 1_000,
            income: 100_000,
            depletion: 0,
            pull: Vec::new(),
            exposed: false,
            wanted,
        })
        .into_iter()
        .map(|candidate| candidate.investment)
        .collect::<Vec<_>>()
    };
    let offered = wants(vec![composition::Role::Line]);
    for kind in [BuildingKind::Foundry, BuildingKind::Crucible] {
        assert!(
            offered.contains(&Investment::Capacity(kind)),
            "{kind:?}: {offered:?}"
        );
    }
    let offered = wants(Vec::new());
    assert!(
        offered
            .iter()
            .all(|investment| !matches!(investment, Investment::Capacity(_))),
        "with nothing wanted, income alone buys no producer: {offered:?}"
    );
}
