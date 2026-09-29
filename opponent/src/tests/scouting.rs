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
    BotConfig::opponent(BotDifficulty::Veteran, BotStance::Balanced, 11)
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
    assert_eq!(attacks(&commands), [(members, AttackTarget::Unit(weakest))]);
    let standard = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(attacks(&standard).is_empty(), "Standard does not focus");

    let (scenario, state, _) = skirmish([(5, 8), (8, 5)], 20);
    let commands = seat_with(&scenario, 0, veteran()).act(&state, &mut OwnEvents::default());
    assert!(!hunts(&commands).is_empty(), "premise: both defend");
    assert!(
        attacks(&commands).is_empty(),
        "a member out of reach would have to chase"
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
fn mirrored_veterans_focus_alike() {
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
    let west = seat_with(&scenario, 0, veteran()).act(&state, &mut OwnEvents::default());
    let east = seat_with(&scenario, 1, veteran()).act(&state, &mut OwnEvents::default());
    assert_eq!(attacks(&west).len(), 1);
    assert_eq!(mirror(&state, west), east);
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
    assert_eq!(
        rejected(&|json| json["missions"]["list"][0]["focus"] = 3.into()),
        "checkpoint mission is in a phase its kind lacks"
    );
}
