use super::*;
use crate::defenses;
use crate::investments::{ADOPT, Investment};
use crate::memory::Memory;
use crate::profile::PersonalityTraits;
use oxide_sim::observation::ObservationData;

/// West's Foundry anchor in the arena.
const FOUNDRY: TilePos = TilePos::new(3, 5);

/// The arena with both seats' harvesting saturated.
fn settled(scrap: u32) -> Scenario {
    let mut scenario = arena(scrap);
    scenario.units.extend(workforce(0));
    scenario.units.extend(workforce(1));
    scenario.units.extend(standing_army(0));
    scenario.units.extend(standing_army(1));
    scenario
}

fn traits(fortification: u8) -> PersonalityTraits {
    PersonalityTraits {
        air: 50,
        siege: 50,
        support: 50,
        fortification,
        greed: 50,
        guile: 50,
    }
}

/// West's defense and defense-upgrade investments in `state`, with too
/// small an army to hold on its own.
fn wanted(
    scenario: &Scenario,
    state: &State,
    memory: &Memory,
    fortification: u8,
) -> Vec<(Investment, u32)> {
    wanted_by(0, true, scenario, state, memory, fortification)
}

/// A seat's defense and defense-upgrade investments in `state`.
fn wanted_by(
    player: u8,
    exposed: bool,
    scenario: &Scenario,
    state: &State,
    memory: &Memory,
    fortification: u8,
) -> Vec<(Investment, u32)> {
    wanted_at(
        player,
        exposed,
        scenario,
        state,
        memory,
        fortification,
        defenses::Stakes::default(),
    )
}

/// A seat's defense and defense-upgrade investments in `state` against
/// `stakes`.
fn wanted_at(
    player: u8,
    exposed: bool,
    scenario: &Scenario,
    state: &State,
    memory: &Memory,
    fortification: u8,
    stakes: defenses::Stakes,
) -> Vec<(Investment, u32)> {
    let observation = ObservationData::fog_honest(state, PlayerId(player));
    let model = map(scenario);
    defenses::investments(
        &observation,
        &model,
        memory,
        traits(fortification),
        true,
        exposed,
        stakes,
    )
}

/// Where a defense of `kind` is wanted, and its score.
fn offer(wanted: &[(Investment, u32)], kind: BuildingKind) -> Option<(TilePos, u32)> {
    wanted
        .iter()
        .find_map(|(investment, score)| match investment {
            Investment::Defense {
                kind: offered,
                anchor,
            } if *offered == kind => Some((*anchor, *score)),
            _ => None,
        })
}

fn builds(commands: &[PlayerCommand]) -> Vec<(BuildingKind, TilePos)> {
    commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Build { kind, anchor, .. } => Some((kind, anchor)),
            _ => None,
        })
        .collect()
}

fn building(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

/// Whether a 1x1 site stands off the West Foundry on the side facing the
/// East start, a gap of two or three tiles away.
fn guards_the_east_side(anchor: TilePos) -> bool {
    let gap = crate::frame::gap(FOUNDRY, (2, 2), anchor, (1, 1));
    anchor.x > FOUNDRY.x + 1 && (2..=3).contains(&gap)
}

#[test]
fn a_fortified_seat_guards_its_foundry_toward_the_enemy_and_a_thrifty_one_does_not() {
    let scenario = settled(400);
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, fortified()).act(&state, &mut OwnEvents::default());
    let [(BuildingKind::Turret, anchor)] = builds(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert!(guards_the_east_side(anchor), "{anchor:?}");

    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    assert!(
        builds(&commands)
            .iter()
            .all(|(kind, _)| *kind != BuildingKind::Turret),
        "{commands:?}"
    );
}

#[test]
fn a_turret_on_the_approach_makes_room_for_a_bastion() {
    let score = |wanted: &[(Investment, u32)], kind| {
        wanted
            .iter()
            .find(|(investment, _)| {
                matches!(investment, Investment::Defense { kind: wanted, .. } if *wanted == kind)
            })
            .map_or(0, |(_, score)| *score)
    };
    let mut scenario = settled(0);
    let bare = scenario.build().unwrap();
    let before = wanted(&scenario, &bare, &Memory::default(), 85);
    assert!(
        score(&before, BuildingKind::Turret) > score(&before, BuildingKind::Bastion),
        "{before:?}"
    );
    let turret = before
        .iter()
        .find_map(|(investment, _)| match investment {
            Investment::Defense {
                kind: BuildingKind::Turret,
                anchor,
            } => Some(*anchor),
            _ => None,
        })
        .expect("a Turret is wanted first");
    scenario
        .buildings
        .push(building(0, BuildingKind::Turret, turret.x, turret.y));
    let state = scenario.build().unwrap();
    let after = wanted(&scenario, &state, &Memory::default(), 85);
    assert!(score(&after, BuildingKind::Bastion) >= ADOPT, "{after:?}");
    assert!(
        score(&after, BuildingKind::Turret) < score(&after, BuildingKind::Bastion),
        "a second Turret adds less than a Bastion: {after:?}"
    );
}

#[test]
fn a_known_army_draws_more_guns_than_a_lone_enemy() {
    let second = |army: &[(i32, i32)]| {
        let mut scenario = settled(0);
        let first = offer(
            &wanted(
                &scenario,
                &scenario.build().unwrap(),
                &Memory::default(),
                85,
            ),
            BuildingKind::Turret,
        )
        .expect("a first Turret is wanted")
        .0;
        scenario
            .buildings
            .push(building(0, BuildingKind::Turret, first.x, first.y));
        scenario.units.push(unit(0, UnitKind::Kestrel, 10, 5));
        for (x, y) in army {
            scenario.units.push(unit(1, UnitKind::Warden, *x, *y));
        }
        let state = scenario.build().unwrap();
        let mut memory = Memory::default();
        memory.observe(&ObservationData::fog_honest(&state, PlayerId(0)));
        offer(
            &wanted(&scenario, &state, &memory, 85),
            BuildingKind::Turret,
        )
        .map_or(0, |(_, score)| score)
    };
    let lone = second(&[(13, 5)]);
    let army = second(&[
        (13, 4),
        (13, 5),
        (13, 6),
        (14, 4),
        (14, 5),
        (14, 6),
        (15, 4),
        (15, 6),
    ]);
    assert!(army > lone, "{army} against {lone}");
}

#[test]
fn a_quiet_approach_draws_more_guns_the_larger_the_stance_minimum() {
    let guns = |minimum: u64| {
        let stakes = defenses::Stakes {
            minimum,
            ..defenses::Stakes::default()
        };
        let mut scenario = settled(0);
        for count in 0..16 {
            let state = scenario.build().unwrap();
            let offered = wanted_at(0, true, &scenario, &state, &Memory::default(), 85, stakes);
            let Some((anchor, _)) = offer(&offered, BuildingKind::Turret) else {
                return count;
            };
            scenario
                .buildings
                .push(building(0, BuildingKind::Turret, anchor.x, anchor.y));
        }
        panic!("the guns never hold the stance minimum off");
    };
    let minimum = defenses::Stakes::default().minimum;
    let (small, large) = (guns(minimum / 2), guns(minimum * 2));
    assert!(small > 0, "premise");
    assert!(large > small, "{large} against {small}");
}

#[test]
fn flak_waits_for_air_evidence() {
    let flak = |wanted: &[(Investment, u32)]| {
        wanted.iter().any(|(investment, _)| {
            matches!(
                investment,
                Investment::Defense {
                    kind: BuildingKind::FlakTurret,
                    ..
                }
            )
        })
    };
    let mut scenario = settled(0);
    let state = scenario.build().unwrap();
    assert!(!flak(&wanted(&scenario, &state, &Memory::default(), 85)));

    scenario.units.push(unit(1, UnitKind::Darter, 10, 5));
    let state = scenario.build().unwrap();
    assert!(flak(&wanted(&scenario, &state, &Memory::default(), 85)));
}

#[test]
fn ground_defenses_face_only_enemies_that_can_walk_in() {
    let ground_when = |exposed: bool, scenario: &Scenario| {
        let state = scenario.build().unwrap();
        wanted_by(0, exposed, scenario, &state, &Memory::default(), 85)
            .iter()
            .any(|(investment, _)| {
                matches!(
                    investment,
                    Investment::Defense {
                        kind: BuildingKind::Turret | BuildingKind::Bastion,
                        ..
                    }
                )
            })
    };
    let ground = |scenario: &Scenario| ground_when(false, scenario);
    let mut scenario = settled(0);
    for row in &mut scenario.map[1..11] {
        row.replace_range(11..13, "##");
    }
    assert!(
        !ground(&scenario),
        "the hostile start is across a chasm from every asset"
    );
    assert!(
        ground_when(true, &scenario),
        "with no army to meet a landing, the seat guards against one"
    );
    scenario.units.push(unit(1, UnitKind::Sentinel, 14, 5));
    assert!(
        !ground(&scenario),
        "a raider in sight across the chasm cannot walk in"
    );
    scenario.units.push(unit(1, UnitKind::Sentinel, 9, 5));
    assert!(
        ground(&scenario),
        "a raider on the seat's own ground counts"
    );
}

#[test]
fn a_building_is_guarded_only_where_a_harvester_can_build() {
    let far_side = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        wanted_by(0, false, scenario, &state, &Memory::default(), 85)
            .iter()
            .any(|(investment, _)| {
                matches!(investment, Investment::Defense { anchor, .. } if anchor.x > 12)
            })
    };
    let mut scenario = settled(0);
    for row in &mut scenario.map[1..11] {
        row.replace_range(11..13, "##");
    }
    scenario
        .buildings
        .push(building(0, BuildingKind::Foundry, 14, 8));
    assert!(
        !far_side(&scenario),
        "no Harvester stands on the Foundry's island"
    );
    scenario.units.push(harvester(0, 14, 7));
    assert!(far_side(&scenario));
}

#[test]
fn a_refused_site_is_not_chosen_again() {
    let scenario = settled(0);
    let state = scenario.build().unwrap();
    let turret = |memory: &Memory| {
        wanted(&scenario, &state, memory, 85)
            .into_iter()
            .find_map(|(investment, _)| match investment {
                Investment::Defense {
                    kind: BuildingKind::Turret,
                    anchor,
                } => Some(anchor),
                _ => None,
            })
            .unwrap()
    };
    let mut memory = Memory::default();
    let first = turret(&memory);
    memory.fail(BuildingKind::Turret, first, 0);
    let second = turret(&memory);
    assert_ne!(first, second);
    assert!(guards_the_east_side(second), "{second:?}");
}

#[test]
fn an_emergency_turret_faces_raiders_only_while_the_approach_is_bare() {
    let raided = |defended: bool| {
        let mut scenario = arena(200);
        scenario.units.push(unit(1, UnitKind::Sentinel, 9, 5));
        if defended {
            scenario
                .buildings
                .push(building(0, BuildingKind::Turret, 7, 5));
        }
        let state = scenario.build().unwrap();
        seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default())
    };
    let commands = raided(false);
    let [(BuildingKind::Turret, anchor)] = builds(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert!(guards_the_east_side(anchor), "{anchor:?}");
    assert!(
        trains(&commands)
            .iter()
            .all(|(_, kind)| *kind != UnitKind::Harvester),
        "scrap goes to the fight, not the economy: {commands:?}"
    );
    assert!(
        builds(&raided(true)).is_empty(),
        "a covered approach needs no emergency"
    );
}

#[test]
fn an_emergency_turret_answers_a_raider_beside_the_building() {
    let mut scenario = arena(200);
    scenario.units.push(unit(1, UnitKind::Sentinel, 6, 5));
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    assert!(
        builds(&commands)
            .iter()
            .any(|(kind, _)| *kind == BuildingKind::Turret),
        "{commands:?}"
    );
}

#[test]
fn an_emergency_turret_guards_the_building_under_attack() {
    let expansion = TilePos::new(14, 8);
    let mut scenario = arena(200);
    scenario
        .buildings
        .push(building(0, BuildingKind::Foundry, expansion.x, expansion.y));
    scenario.units.push(unit(1, UnitKind::Sentinel, 16, 10));
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    let [(BuildingKind::Turret, anchor)] = builds(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert!(
        crate::frame::gap(expansion, (2, 2), anchor, (1, 1)) <= 3,
        "beside the raided Foundry, not the home one: {anchor:?}"
    );
}

#[test]
fn each_pressed_building_gets_its_own_emergency_turret() {
    let expansion = TilePos::new(14, 8);
    let mut scenario = arena(400);
    scenario
        .buildings
        .push(building(0, BuildingKind::Foundry, expansion.x, expansion.y));
    scenario.units.extend([
        unit(1, UnitKind::Sentinel, 16, 10),
        unit(1, UnitKind::Sentinel, 6, 5),
    ]);
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    let turrets: Vec<TilePos> = builds(&commands)
        .into_iter()
        .filter(|(kind, _)| *kind == BuildingKind::Turret)
        .map(|(_, anchor)| anchor)
        .collect();
    for foundry in [FOUNDRY, expansion] {
        assert!(
            turrets
                .iter()
                .any(|anchor| crate::frame::gap(foundry, (2, 2), *anchor, (1, 1)) <= 3),
            "a Turret beside {foundry:?}: {commands:?}"
        );
    }
}

#[test]
fn the_least_valuable_of_many_buildings_is_guarded_too() {
    let reclaimer = TilePos::new(21, 10);
    let mut scenario = arena(200);
    scenario.buildings.extend(
        [(1, 1), (4, 1), (7, 1), (1, 8), (4, 8)]
            .into_iter()
            .map(|(x, y)| building(0, BuildingKind::Fabricator, x, y)),
    );
    scenario.buildings.push(building(
        0,
        BuildingKind::Reclaimer,
        reclaimer.x,
        reclaimer.y,
    ));
    scenario.units.push(unit(1, UnitKind::Sentinel, 22, 7));
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    assert!(
        builds(&commands).iter().any(|(kind, anchor)| {
            *kind == BuildingKind::Turret
                && crate::frame::gap(reclaimer, (1, 1), *anchor, (1, 1)) <= 3
        }),
        "{commands:?}"
    );
}

#[test]
fn mirrored_seats_face_mirrored_threats_that_tie() {
    let mut scenario = settled(400);
    for player in &mut scenario.players {
        player.bot_config = Some(fortified());
    }
    scenario.units.extend([
        unit(1, UnitKind::Sentinel, 8, 3),
        unit(1, UnitKind::Sentinel, 8, 8),
        unit(0, UnitKind::Sentinel, 15, 8),
        unit(0, UnitKind::Sentinel, 15, 3),
    ]);
    let state = scenario.build().unwrap();
    let west = seat_with(&scenario, 0, fortified()).act(&state, &mut OwnEvents::default());
    let east = seat_with(&scenario, 1, fortified()).act(&state, &mut OwnEvents::default());
    assert!(!builds(&west).is_empty(), "premise: {west:?}");
    assert_eq!(mirror(&state, west), east);
}

#[test]
fn a_defense_upgrades_only_with_its_prerequisite_and_no_threat_near() {
    let staged = |fabricator: bool, raider: Option<(UnitKind, i32)>| {
        let mut scenario = settled(0);
        // Only the staged raider threatens the Turret.
        scenario
            .units
            .retain(|unit| unit.player == 0 || unit.kind != UnitKind::Sentinel);
        scenario
            .buildings
            .push(building(0, BuildingKind::Turret, 7, 5));
        if fabricator {
            scenario
                .buildings
                .push(building(0, BuildingKind::Fabricator, 3, 1));
        }
        if let Some((kind, x)) = raider {
            scenario
                .units
                .extend([unit(1, kind, x, 5), unit(0, UnitKind::Kestrel, x - 1, 6)]);
        }
        let state = scenario.build().unwrap();
        let turret = state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Turret)
            .unwrap()
            .id;
        wanted(&scenario, &state, &Memory::default(), 60)
            .iter()
            .any(|(investment, _)| {
                *investment
                    == Investment::Upgrade {
                        building: turret,
                        tier: 1,
                    }
            })
    };
    assert!(staged(true, None));
    assert!(!staged(false, None), "a Heavy Turret needs a Fabricator");
    assert!(
        !staged(true, Some((UnitKind::Sentinel, 11))),
        "an upgrade under fire would be caught down"
    );
    assert!(
        staged(true, Some((UnitKind::Sentinel, 16))),
        "premise: a Sentinel that far cannot reach it"
    );
    assert!(
        !staged(true, Some((UnitKind::Bombard, 16))),
        "a Bombard that far still can"
    );
}

#[test]
fn mirrored_seats_fortify_mirrored_spots() {
    let mut scenario = settled(400);
    for player in &mut scenario.players {
        player.bot_config = Some(fortified());
    }
    let state = scenario.build().unwrap();
    let west = seat_with(&scenario, 0, fortified()).act(&state, &mut OwnEvents::default());
    let east = seat_with(&scenario, 1, fortified()).act(&state, &mut OwnEvents::default());
    assert!(!builds(&west).is_empty(), "premise: {west:?}");
    assert_eq!(mirror(&state, west), east);
}

#[test]
fn checkpoints_reject_a_defense_off_the_map() {
    let scenario = settled(0);
    let state = scenario.build().unwrap();
    let json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    let restore = |target: serde_json::Value| {
        let mut json = json.clone();
        json["saving"] = serde_json::json!({"protected": 0, "target": target});
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario))
    };
    let defense = |x: i32| {
        serde_json::json!({
            "investment": {"defense": {"kind": "turret", "anchor": {"x": x, "y": 5}}},
            "attempt": null,
        })
    };
    assert!(restore(defense(8)).is_ok());
    assert_eq!(
        restore(defense(99)).err().unwrap(),
        "checkpoint saving target is off the map"
    );
}

#[test]
fn an_array_watches_the_way_in_and_lets_a_bastion_fire_further() {
    let mut scenario = settled(0);
    let state = scenario.build().unwrap();
    let bare = wanted(&scenario, &state, &Memory::default(), 85);
    let (array, _) = offer(&bare, BuildingKind::Array).expect("an Array is wanted");
    let bastion = offer(&bare, BuildingKind::Bastion).map_or(0, |(_, score)| score);

    scenario
        .buildings
        .push(building(0, BuildingKind::Array, array.x, array.y));
    let state = scenario.build().unwrap();
    let watched = wanted(&scenario, &state, &Memory::default(), 85);
    assert!(
        offer(&watched, BuildingKind::Array).is_none(),
        "a second Array would watch nothing new: {watched:?}"
    );
    assert!(
        offer(&watched, BuildingKind::Bastion).is_some_and(|(_, score)| score > bastion),
        "radar spots for the Bastion: {bare:?} then {watched:?}"
    );
}

#[test]
fn an_array_watches_for_aircraft_where_no_ground_threat_is_known() {
    let mut scenario = settled(0);
    for row in &mut scenario.map[1..11] {
        row.replace_range(11..13, "##");
    }
    scenario.units.extend([
        unit(1, UnitKind::Darter, 21, 3),
        unit(0, UnitKind::Kestrel, 20, 3),
    ]);
    let state = scenario.build().unwrap();
    let wanted = wanted_by(0, false, &scenario, &state, &Memory::default(), 85);
    assert!(offer(&wanted, BuildingKind::Array).is_some(), "{wanted:?}");
}

#[test]
fn an_array_deepens_once_a_crucible_stands() {
    let staged = |crucible: bool, bombard: bool| {
        let mut scenario = settled(0);
        scenario
            .buildings
            .push(building(0, BuildingKind::Array, 7, 5));
        if crucible {
            scenario
                .buildings
                .push(building(0, BuildingKind::Crucible, 3, 1));
        }
        if bombard {
            scenario.units.extend([
                unit(1, UnitKind::Bombard, 16, 5),
                unit(0, UnitKind::Kestrel, 15, 6),
            ]);
        }
        let state = scenario.build().unwrap();
        let array = state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Array)
            .unwrap()
            .id;
        wanted(&scenario, &state, &Memory::default(), 60)
            .iter()
            .any(|(investment, _)| {
                *investment
                    == Investment::Upgrade {
                        building: array,
                        tier: 1,
                    }
            })
    };
    assert!(staged(true, false));
    assert!(!staged(false, false), "a Deep Array needs a Crucible");
    assert!(
        !staged(true, true),
        "a Bombard in range would catch the Array down"
    );
}

#[test]
fn a_barricade_fronts_a_turret_toward_the_enemy() {
    let turret = TilePos::new(7, 5);
    let mut scenario = settled(0);
    scenario
        .buildings
        .push(building(0, BuildingKind::Turret, turret.x, turret.y));
    let state = scenario.build().unwrap();
    let wanted_now = wanted(&scenario, &state, &Memory::default(), 85);
    let (barricade, _) =
        offer(&wanted_now, BuildingKind::Barricade).expect("a Barricade is wanted");
    assert_eq!(
        crate::frame::gap(turret, (1, 1), barricade, (1, 1)),
        1,
        "{barricade:?}"
    );
    assert!(barricade.x > turret.x, "{barricade:?}");

    scenario.buildings.push(building(
        0,
        BuildingKind::Barricade,
        barricade.x,
        barricade.y,
    ));
    let state = scenario.build().unwrap();
    let fronted = wanted(&scenario, &state, &Memory::default(), 85);
    assert!(
        offer(&fronted, BuildingKind::Barricade).is_none(),
        "{fronted:?}"
    );
}

#[test]
fn scuttle_charges_mine_the_way_in_apart_from_each_other() {
    let charge = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        offer(
            &wanted(scenario, &state, &Memory::default(), 85),
            BuildingKind::ScuttleCharge,
        )
        .map(|(anchor, _)| anchor)
    };
    let mut scenario = settled(0);
    assert_eq!(charge(&scenario), None, "a charge needs a Fabricator");

    scenario
        .buildings
        .push(building(0, BuildingKind::Fabricator, 3, 1));
    let first = charge(&scenario).expect("a charge is wanted");
    let gap = crate::frame::gap(FOUNDRY, (2, 2), first, (1, 1));
    assert!(
        first.x > FOUNDRY.x + 1 && (2..=5).contains(&gap),
        "{first:?}"
    );

    scenario
        .buildings
        .push(building(0, BuildingKind::ScuttleCharge, first.x, first.y));
    let second = charge(&scenario).expect("a second charge is wanted");
    assert!(second.chebyshev(first) >= 3, "{first:?} {second:?}");
}

#[test]
fn mirrored_seats_watch_bar_and_mine_mirrored_spots() {
    let mut scenario = settled(0);
    scenario.buildings.extend([
        building(0, BuildingKind::Turret, 7, 5),
        building(1, BuildingKind::Turret, 16, 6),
        building(0, BuildingKind::Fabricator, 3, 1),
        building(1, BuildingKind::Fabricator, 19, 9),
    ]);
    let state = scenario.build().unwrap();
    let (width, height) = (state.map().width(), state.map().height());
    let defenses = |player: u8| -> Vec<(BuildingKind, TilePos, u32)> {
        wanted_by(player, true, &scenario, &state, &Memory::default(), 85)
            .into_iter()
            .filter_map(|(investment, score)| match investment {
                Investment::Defense { kind, anchor } => Some((kind, anchor, score)),
                _ => None,
            })
            .collect()
    };
    let west = defenses(0);
    for kind in [
        BuildingKind::Array,
        BuildingKind::Barricade,
        BuildingKind::ScuttleCharge,
    ] {
        assert!(
            west.iter().any(|(wanted, _, _)| *wanted == kind),
            "premise: {kind:?} in {west:?}"
        );
    }
    let mirrored: Vec<_> = west
        .into_iter()
        .map(|(kind, anchor, score)| {
            let (w, h) = kind.base_stats().size;
            let anchor = TilePos::new(width - w - anchor.x, height - h - anchor.y);
            (kind, anchor, score)
        })
        .collect();
    assert_eq!(mirrored, defenses(1));
}

/// West's start in a pocket whose way out is (6, 6), and a Turret guarding
/// it. The tile at (6, 9) is `second_exit`: open ground, wall or scrap.
fn pocket(second_exit: char) -> Scenario {
    let mut scenario = arena(400);
    scenario.map = [
        "########################",
        "#.....#................#",
        "#.1...#................#",
        "#.....#................#",
        "#.....#................#",
        "#.....#............2...#",
        "#......................#",
        "#.....#................#",
        "#.....#........s.......#",
        match second_exit {
            '.' => "#......................#",
            's' => "#.....s................#",
            _ => "#.....#................#",
        },
        "#.....#........s.......#",
        "########################",
    ]
    .map(str::to_owned)
    .to_vec();
    scenario.players[0].bot_config = Some(fortified());
    scenario.units = vec![harvester(0, 3, 5), harvester(1, 18, 8)];
    scenario
        .buildings
        .push(building(0, BuildingKind::Turret, 4, 6));
    scenario
}

#[test]
fn a_seat_does_not_wall_itself_in() {
    let exit = TilePos::new(6, 6);
    let barricades = |second_exit: char| {
        let scenario = pocket(second_exit);
        let state = scenario.build().unwrap();
        let mut json =
            serde_json::to_value(seat_with(&scenario, 0, fortified()).checkpoint()).unwrap();
        json["saving"] = serde_json::json!({
            "protected": 0,
            "target": {
                "investment": {"defense": {"kind": "barricade", "anchor": {"x": exit.x, "y": exit.y}}},
                "attempt": null,
            },
        });
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        let mut opponent =
            Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
        builds(&opponent.act(&state, &mut OwnEvents::default()))
            .contains(&(BuildingKind::Barricade, exit))
    };
    assert!(
        barricades('.'),
        "premise: with a second way out the Barricade is bought"
    );
    assert!(
        !barricades('#'),
        "the Barricade would close the only way out"
    );
    let scrapped = pocket('s');
    let state = scrapped.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        !defenses::keeps_paths(&observation, &map(&scrapped), exit),
        "live scrap closes the second way out until it is mined"
    );
}

#[test]
fn an_exposed_opening_buys_no_tech_before_its_first_turret() {
    let mut scenario = arena(150);
    scenario.units.push(unit(0, UnitKind::Sentinel, 3, 10));
    let built = |state: &State| -> Vec<BuildingKind> {
        builds(&seat_with(&scenario, 0, thrifty()).act(state, &mut OwnEvents::default()))
            .into_iter()
            .map(|(kind, _)| kind)
            .collect()
    };
    let mut state = scenario.build().unwrap();
    let opening = built(&state);
    assert!(
        opening.iter().all(|kind| *kind != BuildingKind::Fabricator),
        "no Turret can be placed yet, so tech waits: {opening:?}"
    );
    advance_to(&mut state, crate::defenses::SETTLE_TICKS, &[]);
    let settled = built(&state);
    assert!(
        settled.contains(&BuildingKind::Turret)
            && settled.iter().all(|kind| *kind != BuildingKind::Fabricator),
        "{settled:?}"
    );
}

#[test]
fn an_upgraded_gun_holds_what_its_upgrades_paid_for() {
    // Wardens stand on the approach, so a second Turret's score tracks how
    // much of them the first one holds.
    let second = |tier: u8| {
        let mut scenario = settled(0);
        let first = offer(
            &wanted(
                &scenario,
                &scenario.build().unwrap(),
                &Memory::default(),
                85,
            ),
            BuildingKind::Turret,
        )
        .expect("a first Turret is wanted")
        .0;
        scenario
            .buildings
            .push(building(0, BuildingKind::Turret, first.x, first.y));
        scenario.units.push(unit(0, UnitKind::Kestrel, 10, 5));
        for (x, y) in [(13, 4), (13, 5), (13, 6), (14, 4), (14, 5), (14, 6)] {
            scenario.units.push(unit(1, UnitKind::Warden, x, y));
        }
        let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
        for entry in value["buildings"].as_array_mut().unwrap() {
            if entry["kind"] == "turret" {
                entry["tier"] = tier.into();
            }
        }
        let state: State = serde_json::from_value(value).unwrap();
        let mut memory = Memory::default();
        memory.observe(&ObservationData::fog_honest(&state, PlayerId(0)));
        offer(
            &wanted(&scenario, &state, &memory, 85),
            BuildingKind::Turret,
        )
        .map_or(0, |(_, score)| score)
    };
    let base = second(0);
    let bulwark = second(2);
    assert!(base > 0, "premise: a lone Turret leaves the approach short");
    assert!(bulwark < base, "{bulwark} against {base}");
}

#[test]
fn an_army_across_a_chasm_adds_nothing_to_a_raider_on_the_seat_s_ground() {
    // A first Turret covers the way in, so a second one's score tracks the
    // threat the first leaves unheld.
    let staged = |across: bool, first: Option<TilePos>| {
        let mut scenario = settled(0);
        for row in &mut scenario.map[1..11] {
            row.replace_range(11..13, "##");
        }
        scenario.units.push(unit(1, UnitKind::Sentinel, 9, 5));
        scenario.units.push(unit(0, UnitKind::Kestrel, 12, 5));
        if across {
            for (x, y) in [(14, 4), (14, 5), (14, 6), (15, 5)] {
                scenario.units.push(unit(1, UnitKind::Warden, x, y));
            }
        }
        if let Some(first) = first {
            scenario
                .buildings
                .push(building(0, BuildingKind::Turret, first.x, first.y));
        }
        let state = scenario.build().unwrap();
        let mut memory = Memory::default();
        memory.observe(&ObservationData::fog_honest(&state, PlayerId(0)));
        offer(
            &wanted_by(0, false, &scenario, &state, &memory, 85),
            BuildingKind::Turret,
        )
    };
    let first = staged(false, None)
        .expect("premise: the raider draws a Turret")
        .0;
    let alone = staged(false, Some(first)).map(|(_, score)| score);
    assert_eq!(staged(true, Some(first)).map(|(_, score)| score), alone);
}
