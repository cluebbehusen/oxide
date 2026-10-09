use super::*;
use crate::frame::gap;

/// Ticks after which an unseen point is worth a scout.
const STALE: u64 = 1_800;

/// The field with a West Scuttler, at tick `tick`.
fn scouting(tick: u64) -> (Scenario, State) {
    let mut scenario = field();
    scenario.units.push(unit(0, UnitKind::Scuttler, 6, 9));
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, tick, &[]);
    (scenario, state)
}

fn scouts(missions: &[MissionStatus]) -> Vec<MissionStatus> {
    missions
        .iter()
        .copied()
        .filter(|mission| matches!(mission.kind, MissionKind::Scout { .. }))
        .collect()
}

#[test]
fn a_stale_hostile_start_draws_the_scout() {
    let (scenario, state) = scouting(STALE);
    let scuttler = at(&state, 6, 9);
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let [mission] = scouts(&trace.unwrap().missions)[..] else {
        panic!("one scout");
    };
    assert_eq!(mission.kind, MissionKind::Scout { point: 0 });
    assert_eq!(runs(&commands), [(vec![scuttler], mission.goal)]);
    assert_eq!(
        gap(TilePos::new(43, 11), (2, 2), mission.goal, (1, 1)),
        0,
        "the scout goes beside the East start"
    );
}

/// Three starts: West at (3, 2) and two seats far to the east, out of its
/// sight.
const FAR_PAIR: [&str; 16] = [
    "########################################",
    "#......................................#",
    "#..1...............................2...#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#..................................3...#",
    "#......................................#",
    "########################################",
];

/// Three seats without teams on `FAR_PAIR` at tick `STALE`, West holding
/// `scrap` and `units`: two stale hostile starts.
fn stale_trio(units: Vec<UnitSpec>, scrap: u32) -> (Scenario, State) {
    let mut scenario = super::teams::trio([None; 3]);
    scenario.map = FAR_PAIR.map(str::to_owned).to_vec();
    scenario.units = units;
    scenario.players[0].scrap = scrap;
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    (scenario, state)
}

#[test]
fn each_stale_start_draws_its_own_scout() {
    let kestrel = oxide_sim::stats::Role::Scout.unit_for(Faction::Ferrous);
    let (scenario, state) = stale_trio(vec![unit(0, kestrel, 8, 9), unit(0, kestrel, 9, 9)], 0);
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let missions = scouts(&trace.unwrap().missions);
    let [first, second] = missions[..] else {
        panic!("a scout for each start: {missions:?}");
    };
    assert_ne!(first.kind, second.kind, "two distinct points");
    let sent = runs(&commands);
    for mission in [first, second] {
        assert!(
            sent.iter()
                .any(|(units, goal)| units.len() == 1 && *goal == mission.goal),
            "{sent:?}"
        );
    }
}

#[test]
fn a_stale_start_no_scout_holds_trains_another() {
    let (scenario, state) = stale_trio(vec![unit(0, UnitKind::Scuttler, 8, 9)], 1_000);
    let foundry = foundries(&state, PlayerId(0))[0];
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let missions = scouts(&trace.unwrap().missions);
    assert_eq!(missions.len(), 1, "premise: {missions:?}");
    assert!(
        trains(&commands).contains(&(foundry, UnitKind::Scuttler)),
        "{commands:?}"
    );
}

#[test]
fn a_seen_point_waits_until_stale() {
    let (scenario, state) = scouting(3_600);
    let staged = |seen: u64| {
        let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
        json["memory"]["scouted"] = serde_json::json!([seen]);
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        let mut opponent =
            Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
        runs(&opponent.act(&state, &mut OwnEvents::default()))
    };
    assert!(staged(3_600 - STALE + 12).is_empty(), "seen too recently");
    assert_eq!(staged(3_600 - STALE).len(), 1, "stale again");
}

#[test]
fn a_seat_without_scouts_trains_one() {
    let mut scenario = field();
    scenario.players[0].scrap = 1_000;
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    let foundry = foundries(&state, PlayerId(0))[0];
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(
        trains(&commands).contains(&(foundry, UnitKind::Scuttler)),
        "{commands:?}"
    );

    scenario.buildings.extend([
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 6,
            y: 16,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Airworks,
            x: 10,
            y: 16,
        },
    ]);
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    let airworks = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Airworks)
        .unwrap()
        .id;
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let trained = trains(&commands);
    assert!(
        trained.contains(&(airworks, UnitKind::Kestrel)),
        "{trained:?}"
    );
    assert!(!trained.iter().any(|(_, kind)| *kind == UnitKind::Scuttler));
}

#[test]
fn mirrored_seats_scout_alike() {
    let mut scenario = field();
    scenario.units.extend([
        unit(0, UnitKind::Scuttler, 6, 9),
        unit(1, UnitKind::Scuttler, 41, 14),
    ]);
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    let west = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let east = seat(&scenario, 1).act(&state, &mut OwnEvents::default());
    assert_eq!(runs(&west).len(), 1);
    assert_eq!(mirror(&state, west), east);
}

/// The arena with two West Sentinels at `defenders` and two East Sentinels
/// raiding beside them, the first raider down to `hp`.
fn skirmish(defenders: [(i32, i32); 2], hp: u32) -> (Scenario, State, UnitId) {
    let mut scenario = arena(0);
    for (x, y) in defenders {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
    scenario.units.extend([
        unit(1, UnitKind::Sentinel, 10, 5),
        unit(1, UnitKind::Sentinel, 10, 6),
    ]);
    let state = scenario.build().unwrap();
    let weakest = at(&state, 10, 5);
    let state = wounded(&state, weakest, hp);
    (scenario, state, weakest)
}

fn veteran() -> BotConfig {
    BotConfig::new(BotDifficulty::Veteran, BotStance::Balanced, 11)
}

fn scrapheap() -> BotConfig {
    BotConfig::new(BotDifficulty::Scrapheap, BotStance::Balanced, 11)
}

fn attacks(commands: &[PlayerCommand]) -> Vec<(Vec<UnitId>, AttackTarget)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Attack { units, target, .. } => Some((units.clone(), *target)),
            _ => None,
        })
        .collect()
}

#[test]
fn veteran_focuses_the_weakest_enemy_every_member_reaches() {
    let (scenario, state, weakest) = skirmish([(8, 5), (8, 6)], 20);
    let mut members = vec![at(&state, 8, 5), at(&state, 8, 6)];
    members.sort_unstable();
    let commands = seat_with(&scenario, 0, veteran()).act(&state, &mut OwnEvents::default());
    assert_eq!(
        attacks(&commands),
        [(members.clone(), AttackTarget::Unit(weakest))]
    );
    let standard = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(
        attacks(&standard),
        [(members, AttackTarget::Unit(weakest))],
        "a wound this deep shows on Standard's health bars too"
    );

    let (scenario, state, _) = skirmish([(5, 8), (8, 5)], 20);
    let commands = seat_with(&scenario, 0, veteran()).act(&state, &mut OwnEvents::default());
    assert!(!hunts(&commands).is_empty(), "premise: both defend");
    assert!(
        attacks(&commands).is_empty(),
        "a member out of reach would have to chase"
    );
}

/// Two West Sentinels beside the Foundry and two East raiders beside them:
/// a Warden down to `warden` health at (10, 5) and a Sentinel down to
/// `sentinel` at (10, 6), with each raider's id.
fn wounded_raiders(warden: u32, sentinel: u32) -> (Scenario, State, [UnitId; 2]) {
    let mut scenario = arena(0);
    scenario.units.extend([
        unit(0, UnitKind::Sentinel, 8, 5),
        unit(0, UnitKind::Sentinel, 8, 6),
        unit(1, UnitKind::Warden, 10, 5),
        unit(1, UnitKind::Sentinel, 10, 6),
    ]);
    let state = scenario.build().unwrap();
    let raiders = [at(&state, 10, 5), at(&state, 10, 6)];
    let state = wounded(&state, raiders[0], warden);
    let state = wounded(&state, raiders[1], sentinel);
    (scenario, state, raiders)
}

/// The enemy `config`'s first decision focuses.
fn focused(scenario: &Scenario, state: &State, config: BotConfig) -> AttackTarget {
    let commands = seat_with(scenario, 0, config).act(state, &mut OwnEvents::default());
    let [(_, target)] = attacks(&commands)[..] else {
        panic!("{commands:?}");
    };
    target
}

#[test]
fn standard_focuses_by_health_bar_quarters() {
    // The Warden shows under half a bar at 78 of 260; the Sentinel shows
    // two thirds at 40 of 60, though it is nearer to dying.
    let (scenario, state, [warden, sentinel]) = wounded_raiders(78, 40);
    assert_eq!(
        focused(&scenario, &state, veteran()),
        AttackTarget::Unit(sentinel)
    );
    assert_eq!(
        focused(&scenario, &state, config()),
        AttackTarget::Unit(warden)
    );
}

#[test]
fn scrapheap_focuses_the_nearest_threat_whatever_its_wounds() {
    let near = |hp: [u32; 2]| {
        let (scenario, state, raiders) = wounded_raiders(hp[0], hp[1]);
        let target = focused(&scenario, &state, scrapheap());
        assert!(raiders.iter().any(|id| target == AttackTarget::Unit(*id)));
        target
    };
    assert_eq!(near([20, 60]), near([260, 5]), "wounds do not move it");
    let (scenario, state, [warden, _]) = wounded_raiders(20, 60);
    assert_eq!(
        focused(&scenario, &state, veteran()),
        AttackTarget::Unit(warden)
    );
}

#[test]
fn a_focus_is_not_reissued_while_it_holds() {
    let (scenario, mut state, weakest) = skirmish([(8, 5), (8, 6)], 55);
    let mut opponent = seat_with(&scenario, 0, veteran());
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert_eq!(attacks(&commands).len(), 1);
    advance_to(&mut state, 12, &commands);
    assert!(
        state.units().iter().any(|unit| unit.id == weakest),
        "premise: the focus lives"
    );
    assert!(attacks(&opponent.act(&state, &mut OwnEvents::default())).is_empty());
}

#[test]
fn mirrored_seats_focus_alike_at_every_rung() {
    let mut scenario = arena(0);
    scenario.units.extend([
        unit(0, UnitKind::Sentinel, 8, 5),
        unit(0, UnitKind::Sentinel, 8, 6),
        unit(1, UnitKind::Sentinel, 15, 6),
        unit(1, UnitKind::Sentinel, 15, 5),
        unit(1, UnitKind::Sentinel, 10, 5),
        unit(1, UnitKind::Sentinel, 10, 6),
        unit(0, UnitKind::Sentinel, 13, 6),
        unit(0, UnitKind::Sentinel, 13, 5),
    ]);
    let state = scenario.build().unwrap();
    let state = wounded(&state, at(&state, 10, 5), 20);
    let state = wounded(&state, at(&state, 13, 6), 20);
    for config in [veteran(), config(), scrapheap()] {
        let west = seat_with(&scenario, 0, config).act(&state, &mut OwnEvents::default());
        let east = seat_with(&scenario, 1, config).act(&state, &mut OwnEvents::default());
        assert_eq!(attacks(&west).len(), 1);
        assert_eq!(mirror(&state, west), east);
    }
}

#[test]
fn checkpoints_reject_foreign_scouting_and_stray_focus() {
    let (scenario, state) = scouting(STALE);
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let rejected = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut json = json.clone();
        edit(&mut json);
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario))
            .err()
            .unwrap()
    };
    assert_eq!(
        rejected(&|json| json["memory"]["scouted"] = serde_json::json!([0, 0])),
        "checkpoint scouting memory does not fit the map"
    );
    let mut focused = json.clone();
    focused["missions"]["list"][0]["task"]["focus"] = 3.into();
    assert!(
        serde_json::from_value::<Checkpoint>(focused).is_err(),
        "a scout has no focus"
    );
    assert_eq!(
        rejected(&|json| json["missions"]["list"][0]["task"]["point"] = 99.into()),
        "checkpoint mission could not have been recorded",
        "a point the map does not have"
    );
}

#[test]
fn a_defense_that_stops_fighting_trades_its_focus_for_its_hunt() {
    let (scenario, state, _) = skirmish([(8, 5), (8, 6)], 55);
    let mut opponent = seat_with(&scenario, 0, veteran());
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert_eq!(attacks(&commands).len(), 1, "premise: focused");

    let mut calm = scenario.clone();
    calm.units.truncate(calm.units.len() - 2);
    let mut state = calm.build().unwrap();
    advance_to(&mut state, 12, &[]);
    let mut members = vec![at(&state, 8, 5), at(&state, 8, 6)];
    members.sort_unstable();
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let sent = hunts(&commands);
    assert_eq!(sent.len(), 1, "{commands:?}");
    assert_eq!(sent[0].0, members);
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    assert_eq!(json["missions"]["list"][0]["task"]["phase"], "recover");
}

#[test]
fn air_defenders_guard_the_building_a_flyer_raids() {
    let mut scenario = arena(0);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 12,
        y: 8,
    });
    scenario.units.extend([
        unit(0, UnitKind::Sentinel, 11, 9),
        unit(1, UnitKind::Darter, 17, 10),
    ]);
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let [(_, goal)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    let size = BuildingKind::Fabricator.base_stats().size;
    assert_eq!(
        gap(TilePos::new(12, 8), size, *goal, (1, 1)),
        0,
        "the defenders wait beside the raided Fabricator, not the Foundry"
    );
}

#[test]
fn an_air_scout_replaces_a_scuttler_that_cannot_reach() {
    let mut scenario = field();
    scenario.players[0].scrap = 1_000;
    for (row, cols) in [(19, "###"), (20, "#.#"), (21, "###")] {
        scenario.map[row].replace_range(19..22, cols);
    }
    scenario.units.push(unit(0, UnitKind::Scuttler, 20, 20));
    scenario.buildings.extend([
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 6,
            y: 16,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Airworks,
            x: 10,
            y: 16,
        },
    ]);
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    let trained = trains(&seat(&scenario, 0).act(&state, &mut OwnEvents::default()));
    assert!(
        trained.iter().any(|(_, kind)| *kind == UnitKind::Kestrel),
        "{trained:?}"
    );
}

#[test]
fn the_mission_cap_holds_back_a_scout() {
    let cap = crate::missions::MISSION_CAP;
    let mut scenario = field();
    scenario.players[0].scrap = 1_000;
    scenario.units.push(unit(0, UnitKind::Scuttler, 6, 9));
    for index in 0..cap {
        let (x, y) = cap_spot(index);
        scenario.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    let home = foundries(&state, PlayerId(0))[0];
    let list: Vec<serde_json::Value> = (0..cap)
        .map(|index| {
            let (x, y) = cap_spot(index);
            serde_json::json!({
                "id": index,
                "task": {"task": "defend", "asset": home.0, "phase": {"engage": {"focus": null}}},
                "since": STALE,
                "units": [at(&state, x, y)],
                "goal": {"x": 6, "y": 11},
            })
        })
        .collect();
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["missions"] = serde_json::json!({"next": cap, "list": list, "waiting": null});
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let missions = trace.unwrap().missions;
    assert_eq!(missions.len(), cap);
    assert!(scouts(&missions).is_empty());
    assert!(
        trains(&commands)
            .iter()
            .all(|(_, kind)| *kind != UnitKind::Scuttler),
        "a scout no mission could take is not trained: {commands:?}"
    );
    assert!(Opponent::restore(&opponent.checkpoint(), &scenario, &state, map(&scenario)).is_ok());
}

#[test]
fn a_scout_at_many_points_leaves_room_to_defend() {
    let scouting = 16;
    let mut scenario = field();
    garrison(&mut scenario);
    for index in 0..scouting {
        scenario
            .units
            .push(unit(0, UnitKind::Kestrel, 20 + index % 8, 2 + index / 8));
    }
    scenario.units.push(unit(1, UnitKind::Sentinel, 8, 11));
    let state = scenario.build().unwrap();
    let list: Vec<serde_json::Value> = (0..scouting)
        .map(|index: i32| {
            serde_json::json!({
                "id": index,
                "task": {"task": "scout", "point": 0},
                "since": 0,
                "units": [at(&state, 20 + index % 8, 2 + index / 8)],
                "goal": {"x": 42, "y": 11},
            })
        })
        .collect();
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["missions"] = serde_json::json!({"next": scouting, "list": list, "waiting": null});
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let missions = trace.unwrap().missions;
    assert!(
        missions
            .iter()
            .any(|mission| matches!(mission.kind, MissionKind::Defend { .. })),
        "{missions:?}"
    );
}

/// The field with two West Kestrels and East Flakhounds across the middle,
/// out of sight of the East start.
fn flak_crossing() -> Scenario {
    let mut scenario = field();
    scenario.units.push(unit(0, UnitKind::Kestrel, 6, 9));
    scenario.units.push(unit(0, UnitKind::Kestrel, 6, 13));
    for y in 9..=13 {
        scenario.units.push(unit(1, UnitKind::Flakhound, 24, y));
    }
    scenario
}

#[test]
fn a_scout_lost_on_the_way_waits_out_its_point_before_another_goes() {
    let scenario = flak_crossing();
    let mut state = scenario.build().unwrap();
    let kestrels = [at(&state, 6, 9), at(&state, 6, 13)];
    advance_to(&mut state, STALE, &[]);
    let mut opponent = seat(&scenario, 0);
    let alive = |state: &State, id: UnitId| state.units().iter().any(|unit| unit.id == id);
    let mut sent = Vec::new();
    while kestrels.iter().all(|id| alive(&state, *id)) {
        assert!(
            state.current_tick() < STALE + 1_200,
            "premise: the scout is shot down"
        );
        let commands = opponent.act(&state, &mut OwnEvents::default());
        sent.extend(runs(&commands).into_iter().flat_map(|(units, _)| units));
        state.tick(&commands);
    }
    let lost = state.current_tick();
    let survivor = *kestrels.iter().find(|id| alive(&state, **id)).unwrap();
    assert!(!sent.contains(&survivor), "premise: one scout went");

    while state.current_tick() < lost + STALE - 24 {
        let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
        assert!(
            runs(&commands)
                .iter()
                .all(|(units, _)| !units.contains(&survivor)),
            "tick {}: the other scout is sent after it",
            state.current_tick()
        );
        assert!(trace.is_none_or(|trace| scouts(&trace.missions).is_empty()));
        state.tick(&commands);
    }
}

#[test]
fn an_air_scout_flies_around_remembered_anti_air() {
    let scenario = flak_crossing();
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    let staged = |remember: bool| {
        let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
        if remember {
            json["memory"]["units"] = (9..=13)
                .map(|y| {
                    serde_json::json!({
                        "id": at(&state, 24, y).0,
                        "kind": "flakhound",
                        "tile": {"x": 24, "y": y},
                        "seen": STALE,
                    })
                })
                .collect();
        }
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        let mut opponent =
            Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
        let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
        let [mission] = scouts(&trace.unwrap().missions)[..] else {
            panic!("one scout");
        };
        (commands, mission.goal)
    };

    let (commands, goal) = staged(false);
    let [(ref scout, straight)] = runs(&commands)[..] else {
        panic!("premise: one order straight there: {commands:?}");
    };
    assert_eq!(straight, goal);
    let kestrel = scout[0];

    let (commands, goal) = staged(true);
    let sent: Vec<(TilePos, bool)> = commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Run { units, goal, queue } if *units == [kestrel] => Some((*goal, *queue)),
            _ => None,
        })
        .collect();
    let [(via, false), (last, true)] = sent[..] else {
        panic!("{sent:?}");
    };
    assert_eq!(last, goal);
    assert!(
        (via.y - 11).abs() > 7,
        "the detour passes the Flakhounds out of reach: {via:?}"
    );
}

#[test]
fn no_scuttler_is_trained_for_a_point_across_a_chasm() {
    let mut scenario = field();
    scenario.players[0].scrap = 1_000;
    for row in &mut scenario.map[1..23] {
        row.replace_range(16..32, &"~".repeat(16));
    }
    scenario.units.push(unit(0, UnitKind::Scuttler, 6, 9));
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, STALE, &[]);
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert!(
        scouts(&trace.unwrap().missions).is_empty(),
        "premise: the Scuttler cannot go"
    );
    assert!(
        trains(&commands)
            .iter()
            .all(|(_, kind)| *kind != UnitKind::Scuttler),
        "another could not go either: {commands:?}"
    );
}
