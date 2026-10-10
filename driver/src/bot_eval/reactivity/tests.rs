use super::*;
use chassis::fx::{Fx, Vec2Fx};
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{BuildingId, Scenario, UnitId};

/// A 24-wide field: West's start at (2, 2) and East's at (20, 2), 12 rows
/// deep, or 16 with a third start at (2, 13), allied to West, when `trio`
/// holds.
fn field(
    units: &[(u8, UnitKind, i32, i32)],
    buildings: &[(u8, BuildingKind, i32, i32)],
    trio: bool,
) -> Scenario {
    let ground = ".".repeat(24);
    let mut map = vec![ground.clone(); if trio { 16 } else { 12 }];
    let mut top: Vec<char> = ground.chars().collect();
    top[2] = '1';
    top[20] = '2';
    map[2] = top.into_iter().collect();
    if trio {
        let mut low: Vec<char> = ground.chars().collect();
        low[2] = '3';
        map[13] = low.into_iter().collect();
    }
    Scenario {
        mode: ScenarioMode::Match,
        name: "reactivity".into(),
        map,
        players: (0..if trio { 3 } else { 2 })
            .map(|seat| PlayerSpec {
                name: format!("seat {seat}"),
                team: trio.then_some(u8::from(seat == 1)),
                scrap: 0,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units: units
            .iter()
            .map(|&(player, kind, x, y)| UnitSpec { player, kind, x, y })
            .collect(),
        buildings: buildings
            .iter()
            .map(|&(player, kind, x, y)| BuildingSpec { player, kind, x, y })
            .collect(),
        meta: None,
    }
}

/// `scenario` built, with vision refreshed by one tick.
fn built(scenario: &Scenario) -> State {
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    state
}

fn unit_at(state: &State, x: i32, y: i32) -> UnitId {
    state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(x, y))
        .unwrap()
        .id
}

fn building_of(state: &State, player: u8, kind: BuildingKind) -> BuildingId {
    state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(player) && building.kind == kind)
        .unwrap()
        .id
}

fn hit(attacker: UnitId, target: Target) -> Event {
    Event::AttackHit {
        attacker,
        attacker_kind: UnitKind::Sentinel,
        weapon: 0,
        target: Some(target),
        attacker_pos: Vec2Fx::ZERO,
        target_pos: Vec2Fx::ZERO,
    }
}

fn found(detectors: ReactivityDetectors, seat: usize) -> SeatReactivity {
    detectors.finish()[seat].clone().unwrap()
}

fn counts(reactions: &Reactions) -> (u64, u64, u64, u64) {
    (
        reactions.arose,
        reactions.answered,
        reactions.missed,
        reactions.moot,
    )
}

fn run(detectors: &mut ReactivityDetectors, state: &State, ticks: std::ops::Range<u64>) {
    for now in ticks.step_by(12) {
        detectors.check(state, now);
    }
}

#[test]
fn first_enemy_air_is_answered_by_anti_air_in_time_or_missed() {
    let buzzard = (1, UnitKind::Buzzard, 4, 4);
    let bare = built(&field(&[buzzard], &[], false));
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &bare, 0..ANTI_AIR_TICKS + 12);
    let missed = found(detectors, 0);
    assert_eq!(counts(&missed.anti_air), (1, 0, 1, 0));

    let defended = built(&field(
        &[buzzard, (0, UnitKind::Flakhound, 5, 6)],
        &[],
        false,
    ));
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &bare, 0..24);
    run(&mut detectors, &defended, 24..36);
    let answered = found(detectors, 0);
    assert_eq!(counts(&answered.anti_air), (1, 1, 0, 0));
    assert_eq!(answered.anti_air.answer_ticks, 24);
}

#[test]
fn an_airworks_seen_first_wants_anti_air_before_the_first_aircraft() {
    let airworks = (1, BuildingKind::Airworks, 6, 4);
    let seen = built(&field(&[], &[airworks], false));
    let defended = built(&field(
        &[(0, UnitKind::Flakhound, 5, 7)],
        &[airworks],
        false,
    ));
    let raided = built(&field(&[(1, UnitKind::Buzzard, 4, 4)], &[airworks], false));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &seen, 0..24);
    run(&mut detectors, &defended, 24..36);
    assert_eq!(counts(&found(detectors, 0).airworks), (1, 1, 0, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &seen, 0..24);
    run(&mut detectors, &raided, 24..36);
    let missed = found(detectors, 0);
    assert_eq!(counts(&missed.airworks), (1, 0, 1, 0));
    assert_eq!(missed.anti_air.arose, 1, "the aircraft opens its own case");
}

#[test]
fn a_pressed_foundry_is_answered_by_a_hit_lapses_when_the_enemy_leaves_or_is_missed() {
    let pressing = field(
        &[(1, UnitKind::Sentinel, 6, 3), (0, UnitKind::Sentinel, 2, 6)],
        &[],
        false,
    );
    let state = built(&pressing);
    let presser = unit_at(&state, 6, 3);
    let defender = unit_at(&state, 2, 6);

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &state, 0..24);
    detectors.observe_events(&state, &[hit(defender, Target::Unit(presser))], 30);
    run(&mut detectors, &state, 36..48);
    let answered = found(detectors, 0);
    assert_eq!(counts(&answered.ground_defense), (1, 1, 0, 0));
    assert_eq!(answered.ground_defense.answer_ticks, 30);

    let quiet = built(&field(&[(0, UnitKind::Sentinel, 2, 6)], &[], false));
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &state, 0..24);
    run(&mut detectors, &quiet, 24..36);
    assert_eq!(counts(&found(detectors, 0).ground_defense), (1, 0, 0, 1));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &state, 0..DEFENSE_TICKS + 24);
    assert_eq!(
        counts(&found(detectors, 0).ground_defense),
        (1, 0, 1, 0),
        "one pressing is one case"
    );
}

#[test]
fn a_sapper_pressing_a_foundry_opens_a_ground_defense_case() {
    let state = built(&field(
        &[(1, UnitKind::Sapper, 6, 3), (0, UnitKind::Sentinel, 2, 6)],
        &[],
        false,
    ));
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &state, 0..12);
    assert_eq!(found(detectors, 0).ground_defense.arose, 1);
}

#[test]
fn enemy_aircraft_over_a_foundry_open_an_air_defense_case() {
    let state = built(&field(
        &[(1, UnitKind::Buzzard, 6, 3), (0, UnitKind::Flakhound, 2, 6)],
        &[],
        false,
    ));
    let raider = unit_at(&state, 6, 3);
    let flak = unit_at(&state, 2, 6);
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &state, 0..12);
    detectors.observe_events(&state, &[hit(flak, Target::Unit(raider))], 12);
    let answered = found(detectors, 0);
    assert_eq!(counts(&answered.air_defense), (1, 1, 0, 0));
    assert_eq!(answered.ground_defense.arose, 0);
}

#[test]
fn shelling_is_answered_by_hitting_the_gun_and_lapses_if_it_dies_to_another() {
    let state = built(&field(
        &[(1, UnitKind::Bombard, 9, 3), (0, UnitKind::Sentinel, 6, 6)],
        &[],
        false,
    ));
    let gun = unit_at(&state, 9, 3);
    let sentinel = unit_at(&state, 6, 6);
    let shell = Event::ShellLaunched {
        shooter: Target::Unit(gun),
        unit_pose: None,
        target: Some(Target::Building(building_of(
            &state,
            0,
            BuildingKind::Foundry,
        ))),
        player: PlayerId(1),
        from: Vec2Fx::ZERO,
        to: Vec2Fx::ZERO,
        flight: 20,
    };

    let mut detectors = ReactivityDetectors::new([true, false]);
    detectors.observe_events(&state, std::slice::from_ref(&shell), 12);
    detectors.observe_events(&state, std::slice::from_ref(&shell), 24);
    detectors.observe_events(&state, &[hit(sentinel, Target::Unit(gun))], 100);
    assert_eq!(counts(&found(detectors, 0).artillery), (1, 1, 0, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    detectors.observe_events(&state, std::slice::from_ref(&shell), 12);
    detectors.observe_events(
        &state,
        &[Event::UnitDied {
            unit: gun,
            kind: UnitKind::Bombard,
            player: PlayerId(1),
            pos: Vec2Fx::ZERO,
            grounded: true,
        }],
        60,
    );
    assert_eq!(counts(&found(detectors, 0).artillery), (1, 0, 0, 1));

    let mut detectors = ReactivityDetectors::new([true, false]);
    detectors.observe_events(&state, std::slice::from_ref(&shell), 0);
    run(&mut detectors, &state, 0..ARTILLERY_TICKS + 12);
    let missed = found(detectors, 0);
    assert_eq!(counts(&missed.artillery), (1, 0, 1, 0));
    assert_eq!(missed.artillery.examples[0].detail, "bombard");

    let afield = Event::ShellLaunched {
        shooter: Target::Unit(gun),
        unit_pose: None,
        target: Some(Target::Unit(sentinel)),
        player: PlayerId(1),
        from: Vec2Fx::ZERO,
        to: Vec2Fx::new(Fx::from_num(20), Fx::from_num(10)),
        flight: 20,
    };
    let mut detectors = ReactivityDetectors::new([true, false]);
    detectors.observe_events(&state, &[afield], 12);
    assert_eq!(
        found(detectors, 0).artillery.arose,
        0,
        "a shell at an army far from home is no shelling of assets"
    );
}

#[test]
fn a_stale_hostile_start_is_answered_when_seen_again_or_missed() {
    let hidden = built(&field(&[], &[], false));
    let looking = built(&field(&[(0, UnitKind::Sentinel, 19, 5)], &[], false));
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &hidden, 0..STALE_TICKS + 12);
    run(&mut detectors, &looking, STALE_TICKS + 12..STALE_TICKS + 24);
    assert_eq!(counts(&found(detectors, 0).scouting), (1, 1, 0, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &hidden, 0..STALE_TICKS + SCOUT_TICKS + 12);
    assert_eq!(counts(&found(detectors, 0).scouting), (1, 0, 1, 0));
}

#[test]
fn a_worker_in_reach_must_run_and_one_that_dies_or_stays_is_missed() {
    let enemy = (1, UnitKind::Sentinel, 14, 8);
    let reached = built(&field(
        &[(0, UnitKind::Harvester, 12, 8), enemy],
        &[],
        false,
    ));
    let ran = built(&field(&[(0, UnitKind::Harvester, 8, 8), enemy], &[], false));
    let left = built(&field(&[(0, UnitKind::Harvester, 12, 8)], &[], false));
    let worker = unit_at(&reached, 12, 8);

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &reached, 0..12);
    run(&mut detectors, &ran, 12..24);
    assert_eq!(counts(&found(detectors, 0).evacuation), (1, 1, 0, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &reached, 0..12);
    run(&mut detectors, &left, 12..24);
    assert_eq!(
        counts(&found(detectors, 0).evacuation),
        (1, 0, 0, 1),
        "the enemy left before the worker moved"
    );

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &reached, 0..12);
    detectors.observe_events(
        &reached,
        &[Event::UnitDied {
            unit: worker,
            kind: UnitKind::Harvester,
            player: PlayerId(0),
            pos: Vec2Fx::ZERO,
            grounded: true,
        }],
        20,
    );
    assert_eq!(counts(&found(detectors, 0).evacuation), (1, 0, 1, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &reached, 0..EVACUATE_TICKS + 12);
    assert_eq!(counts(&found(detectors, 0).evacuation), (1, 0, 1, 0));
}

/// `state` with every building of `kind` at `hp`.
fn damaged(state: &State, kind: &str, hp: u32) -> State {
    let mut value = serde_json::to_value(state).unwrap();
    for building in value["buildings"].as_array_mut().unwrap() {
        if building["kind"] == kind {
            building["hp"] = hp.into();
        }
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_damaged_building_with_no_enemy_near_wants_repair() {
    let fabricator = (0, BuildingKind::Fabricator, 5, 6);
    let state = built(&field(&[], &[fabricator], false));
    let hurt = damaged(&state, "fabricator", 100);
    let mending = damaged(&state, "fabricator", 140);

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &hurt, 0..24);
    run(&mut detectors, &mending, 24..36);
    assert_eq!(counts(&found(detectors, 0).repair), (1, 1, 0, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &hurt, 0..REPAIR_TICKS + 12);
    assert_eq!(counts(&found(detectors, 0).repair), (1, 0, 1, 0));

    let contested = damaged(
        &built(&field(
            &[(1, UnitKind::Sentinel, 8, 7)],
            &[fabricator],
            false,
        )),
        "fabricator",
        100,
    );
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &contested, 0..REPAIR_TICKS + 12);
    assert_eq!(found(detectors, 0).repair.arose, 0, "an enemy stands near");

    let barricade = (0, BuildingKind::Barricade, 6, 6);
    let worn = damaged(&built(&field(&[], &[barricade], false)), "barricade", 10);
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &worn, 0..REPAIR_TICKS + 12);
    assert_eq!(
        found(detectors, 0).repair.arose,
        0,
        "obstacles are not patients"
    );
}

#[test]
fn a_destroyed_extractor_wants_another_on_its_site() {
    let extractor = (0, BuildingKind::Extractor, 6, 6);
    let standing = built(&field(&[], &[extractor], false));
    let gone = built(&field(&[], &[], false));
    let lost = Event::BuildingDestroyed {
        building: building_of(&standing, 0, BuildingKind::Extractor),
        player: PlayerId(0),
        tier: 0,
        pos: Vec2Fx::ZERO,
    };

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &standing, 0..12);
    detectors.observe_events(&standing, std::slice::from_ref(&lost), 12);
    run(&mut detectors, &gone, 12..48);
    run(&mut detectors, &standing, 48..60);
    assert_eq!(counts(&found(detectors, 0).restoration), (1, 1, 0, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &standing, 0..12);
    detectors.observe_events(&standing, std::slice::from_ref(&lost), 12);
    run(&mut detectors, &gone, 12..RESTORE_TICKS + 24);
    assert_eq!(counts(&found(detectors, 0).restoration), (1, 0, 1, 0));
}

#[test]
fn an_ally_s_pressed_foundry_is_answered_by_the_seat_s_hit() {
    let state = built(&field(
        &[
            (1, UnitKind::Sentinel, 6, 13),
            (0, UnitKind::Sentinel, 5, 11),
        ],
        &[],
        true,
    ));
    let presser = unit_at(&state, 6, 13);
    let helper = unit_at(&state, 5, 11);
    let mut detectors = ReactivityDetectors::new([true, false, false]);
    run(&mut detectors, &state, 0..12);
    detectors.observe_events(&state, &[hit(helper, Target::Unit(presser))], 20);
    let answered = found(detectors, 0);
    assert_eq!(counts(&answered.relief), (1, 1, 0, 0));
    assert_eq!(
        answered.ground_defense.arose, 0,
        "West's own Foundry is clear"
    );
}

fn mission(id: u64, owner: u8, phase: Phase, units: u32) -> MissionStatus {
    MissionStatus {
        id,
        kind: MissionKind::Attack {
            owner: PlayerId(owner),
            building: BuildingKind::Foundry,
            anchor: TilePos::new(20, 2),
        },
        phase,
        since: 0,
        timeout: 600,
        units,
        goal: TilePos::new(18, 2),
    }
}

#[test]
fn a_losing_fight_should_withdraw_and_new_targets_count_switches() {
    let mut detectors = ReactivityDetectors::new([true, false, false]);
    detectors.check_missions(0, 12, &[mission(0, 1, Phase::Travel, 8)]);
    detectors.check_missions(0, 24, &[mission(0, 1, Phase::Engage, 8)]);
    detectors.check_missions(0, 36, &[mission(0, 1, Phase::Withdraw, 5)]);
    detectors.check_missions(0, 48, &[mission(1, 2, Phase::Engage, 8)]);
    detectors.check_missions(0, 60, &[mission(1, 2, Phase::Engage, 3)]);
    detectors.check_missions(0, 72, &[mission(2, 2, Phase::Engage, 6)]);
    detectors.check_missions(0, 84, &[mission(2, 2, Phase::Recover, 6)]);
    let raid = MissionStatus {
        kind: MissionKind::Raid {
            owner: PlayerId(1),
            building: BuildingKind::Extractor,
            anchor: TilePos::new(14, 8),
        },
        ..mission(3, 1, Phase::Travel, 2)
    };
    detectors.check_missions(0, 96, &[raid]);
    let found = found(detectors, 0);
    let withdrawal = found.withdrawal.unwrap();
    assert_eq!(counts(&withdrawal), (3, 1, 1, 1));
    assert_eq!(withdrawal.examples[0].detail, "attack");
    assert_eq!(found.target_switches, Some(1));
}

#[test]
fn seats_without_missions_report_no_withdrawal() {
    let detectors = ReactivityDetectors::new([true, false]);
    let found = found(detectors, 0);
    assert_eq!(found.withdrawal, None);
    assert_eq!(found.target_switches, None);
}

#[test]
fn cases_open_when_a_seat_falls_or_the_leg_ends_are_moot() {
    let state = built(&field(&[(1, UnitKind::Sentinel, 6, 3)], &[], false));
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &state, 0..12);
    assert_eq!(counts(&found(detectors, 0).ground_defense), (1, 0, 0, 1));
}

/// `state` without `player`'s buildings of `kind`.
fn without(state: &State, player: u8, kind: &str) -> State {
    let mut value = serde_json::to_value(state).unwrap();
    value["buildings"]
        .as_array_mut()
        .unwrap()
        .retain(|building| !(building["player"] == player && building["kind"] == kind));
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_hit_from_a_shooter_killed_that_tick_still_answers() {
    // The defender comes last, so the field without it keeps every other
    // unit's id.
    let state = built(&field(
        &[(1, UnitKind::Sentinel, 6, 3), (0, UnitKind::Sentinel, 2, 6)],
        &[],
        false,
    ));
    let presser = unit_at(&state, 6, 3);
    let defender = unit_at(&state, 2, 6);
    let after = built(&field(&[(1, UnitKind::Sentinel, 6, 3)], &[], false));
    let died = Event::UnitDied {
        unit: defender,
        kind: UnitKind::Sentinel,
        player: PlayerId(0),
        pos: Vec2Fx::ZERO,
        grounded: true,
    };
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &state, 0..24);
    detectors.observe_events(&after, &[hit(defender, Target::Unit(presser)), died], 30);
    assert_eq!(counts(&found(detectors, 0).ground_defense), (1, 1, 0, 0));
}

#[test]
fn an_eliminated_seat_opens_and_answers_no_cases() {
    let state = built(&field(
        &[(1, UnitKind::Bombard, 9, 3), (0, UnitKind::Sentinel, 6, 6)],
        &[(0, BuildingKind::Fabricator, 3, 8)],
        false,
    ));
    let gun = unit_at(&state, 9, 3);
    let sentinel = unit_at(&state, 6, 6);
    let shell = Event::ShellLaunched {
        shooter: Target::Unit(gun),
        unit_pose: None,
        target: Some(Target::Building(building_of(
            &state,
            0,
            BuildingKind::Fabricator,
        ))),
        player: PlayerId(1),
        from: Vec2Fx::ZERO,
        to: TilePos::new(4, 8).center(),
        flight: 20,
    };
    let fallen = without(&state, 0, "foundry");
    assert!(!fallen.accepts_commands(PlayerId(0)), "premise");

    let mut detectors = ReactivityDetectors::new([true, false]);
    detectors.observe_events(&fallen, std::slice::from_ref(&shell), 12);
    assert_eq!(
        found(detectors, 0).artillery.arose,
        0,
        "shelling a fallen seat is no case"
    );

    let mut detectors = ReactivityDetectors::new([true, false]);
    detectors.observe_events(&state, std::slice::from_ref(&shell), 12);
    detectors.observe_events(&fallen, &[hit(sentinel, Target::Unit(gun))], 24);
    assert_eq!(
        counts(&found(detectors, 0).artillery),
        (1, 0, 0, 1),
        "its remnants answer nothing"
    );
}

#[test]
fn a_building_still_damaged_after_its_case_closes_opens_no_second_case() {
    let fabricator = (0, BuildingKind::Fabricator, 5, 6);
    let state = built(&field(&[], &[fabricator], false));
    let hurt = damaged(&state, "fabricator", 100);
    let mending = damaged(&state, "fabricator", 140);
    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &hurt, 0..24);
    run(&mut detectors, &mending, 24..240);
    assert_eq!(counts(&found(detectors, 0).repair), (1, 1, 0, 0));

    let mut detectors = ReactivityDetectors::new([true, false]);
    run(&mut detectors, &hurt, 0..24);
    run(&mut detectors, &mending, 24..36);
    run(&mut detectors, &state, 36..48);
    run(&mut detectors, &hurt, 48..60);
    assert_eq!(
        found(detectors, 0).repair.arose,
        2,
        "damaged again once repaired"
    );
}

#[test]
fn merged_reactions_sum_and_keep_the_first_examples() {
    let mut total = SeatReactivity::default();
    let one = SeatReactivity {
        repair: Reactions {
            arose: 2,
            missed: 2,
            examples: (0..MAX_FAILURE_EXAMPLES as u64)
                .map(|tick| FailureIncident {
                    tick,
                    subject: 1,
                    detail: "fabricator".into(),
                })
                .collect(),
            ..Reactions::default()
        },
        withdrawal: Some(Reactions {
            arose: 1,
            answered: 1,
            answer_ticks: 50,
            ..Reactions::default()
        }),
        target_switches: Some(2),
        ..SeatReactivity::default()
    };
    total.merge(&one);
    total.merge(&one);
    assert_eq!(counts(&total.repair), (4, 0, 4, 0));
    assert_eq!(total.repair.examples.len(), MAX_FAILURE_EXAMPLES);
    assert_eq!(total.withdrawal.unwrap().answer_ticks, 100);
    assert_eq!(total.target_switches, Some(4));
}
