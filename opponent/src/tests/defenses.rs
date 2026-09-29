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
    scenario.units.extend([
        harvester(0, 5, 7),
        harvester(0, 4, 7),
        harvester(1, 18, 4),
        harvester(1, 19, 4),
    ]);
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

/// West's defense and defense-upgrade investments in `state`.
fn wanted(
    scenario: &Scenario,
    state: &State,
    memory: &Memory,
    fortification: u8,
) -> Vec<(Investment, u32)> {
    let observation = ObservationData::fog_honest(state, PlayerId(0));
    let model = map(scenario);
    defenses::investments(
        &observation,
        &model,
        memory,
        traits(fortification),
        true,
        true,
    )
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
    let ground = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        wanted(scenario, &state, &Memory::default(), 85)
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
    let mut scenario = settled(0);
    for row in &mut scenario.map[1..11] {
        row.replace_range(11..13, "##");
    }
    assert!(
        !ground(&scenario),
        "the hostile start is across a chasm from every asset"
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
        wanted(scenario, &state, &Memory::default(), 85)
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
fn a_defense_upgrades_only_with_its_prerequisite_and_no_threat_near() {
    let staged = |fabricator: bool, raider: bool| {
        let mut scenario = settled(0);
        scenario
            .buildings
            .push(building(0, BuildingKind::Turret, 7, 5));
        if fabricator {
            scenario
                .buildings
                .push(building(0, BuildingKind::Fabricator, 3, 1));
        }
        if raider {
            scenario.units.push(unit(1, UnitKind::Sentinel, 11, 5));
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
    assert!(staged(true, false));
    assert!(!staged(false, false), "a Heavy Turret needs a Fabricator");
    assert!(
        !staged(true, true),
        "an upgrade under fire would be caught down"
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
