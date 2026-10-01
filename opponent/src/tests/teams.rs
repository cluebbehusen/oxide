use super::*;
use crate::frame::HomeFrame;
use crate::memory::Memory;
use crate::missions::{Missions, Scratch};
use crate::profile::PersonalityTraits;
use oxide_sim::observation::ObservationData;

/// Three starts: West at (3, 3), a second seat at (3, 11) below it, and a
/// third far to the east.
const TRIO: [&str; 16] = [
    "########################################",
    "#......................................#",
    "#......................................#",
    "#..1...................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#.................................3....#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#..2...................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "########################################",
];

/// The trio with each seat on the given team, and four West Sentinels.
fn trio(teams: [Option<u8>; 3]) -> Scenario {
    let seat = |name: &str, faction, team| PlayerSpec {
        name: name.into(),
        faction,
        team,
        scrap: 0,
        bot: true,
        bot_config: Some(config()),
    };
    Scenario {
        mode: ScenarioMode::Match,
        name: "opponent trio".into(),
        seed: 5,
        map: TRIO.map(str::to_owned).to_vec(),
        players: vec![
            seat("west", Faction::Ferrous, teams[0]),
            seat("south", Faction::Cupric, teams[1]),
            seat("east", Faction::Cupric, teams[2]),
        ],
        units: [(8, 9), (8, 10), (9, 9), (9, 10)]
            .into_iter()
            .map(|(x, y)| unit(0, UnitKind::Sentinel, x, y))
            .collect(),
        buildings: Vec::new(),
        meta: None,
    }
}

fn defends(missions: &[MissionStatus]) -> Vec<BuildingId> {
    missions
        .iter()
        .filter_map(|mission| match mission.kind {
            MissionKind::Defend { asset } => Some(asset),
            _ => None,
        })
        .collect()
}

#[test]
fn free_units_relieve_an_ally_under_ground_attack() {
    let mut scenario = trio([Some(0), Some(0), Some(1)]);
    scenario.units.push(unit(2, UnitKind::Sentinel, 4, 14));
    let state = scenario.build().unwrap();
    let ally = foundries(&state, PlayerId(1))[0];
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert_eq!(defends(&trace.unwrap().missions), [ally]);
    let sent = hunts(&commands);
    assert!(
        sent.iter().any(|(_, goal)| *goal == TilePos::new(4, 14)),
        "{sent:?}"
    );
}

#[test]
fn the_seat_answers_its_own_threat_before_an_ally_s() {
    let mut scenario = trio([Some(0), Some(0), Some(1)]);
    scenario.units.extend([
        unit(2, UnitKind::Sentinel, 4, 14),
        unit(2, UnitKind::Warden, 6, 3),
    ]);
    let state = scenario.build().unwrap();
    let own = foundries(&state, PlayerId(0))[0];
    let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let defended = defends(&trace.unwrap().missions);
    assert_eq!(defended.first(), Some(&own), "{defended:?}");
}

#[test]
fn an_enemy_threat_to_a_neutral_seat_is_not_the_seat_s_to_answer() {
    let mut scenario = trio([None, None, None]);
    scenario.units.push(unit(2, UnitKind::Sentinel, 4, 14));
    let state = scenario.build().unwrap();
    let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert!(defends(&trace.unwrap().missions).is_empty());
}

fn traits(guile: u8) -> PersonalityTraits {
    PersonalityTraits {
        air: 50,
        siege: 50,
        support: 50,
        fortification: 50,
        greed: 50,
        guile,
    }
}

/// The decision scratch a seat builds for `observation`.
fn scratch(observation: &ObservationData, model: &MapModel) -> Scratch {
    let frame = HomeFrame::of(observation, model).unwrap();
    Scratch::new(
        observation,
        model,
        frame,
        &Memory::default(),
        BotStance::Balanced,
        &Missions::default(),
    )
}

#[test]
fn among_several_enemies_the_one_pressing_the_seat_is_the_rival() {
    let scenario = trio([None, None, None]);
    let rival = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        let observation = ObservationData::fog_honest(&state, PlayerId(0));
        let model = map(scenario);
        let scratch = scratch(&observation, &model);
        Missions::default().rival(&scratch, &observation, &model, traits(50))
    };
    assert_eq!(rival(&scenario), Some(PlayerId(1)), "the nearer enemy");
    let mut pressed = scenario.clone();
    pressed.units.push(unit(2, UnitKind::Sentinel, 7, 3));
    assert_eq!(rival(&pressed), Some(PlayerId(2)), "the enemy at the gate");
}

#[test]
fn a_duel_has_no_rival() {
    let scenario = arena(0);
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let model = map(&scenario);
    let scratch = scratch(&observation, &model);
    assert_eq!(
        Missions::default().rival(&scratch, &observation, &model, traits(50)),
        None
    );
}

#[test]
fn an_ally_s_defenders_come_home_when_home_is_attacked() {
    let mut scenario = trio([Some(0), Some(0), Some(1)]);
    scenario.units.extend([
        unit(2, UnitKind::Sentinel, 4, 14),
        unit(2, UnitKind::Warden, 6, 3),
    ]);
    let mut state = scenario.build().unwrap();
    let mut relievers: Vec<UnitId> = [(8, 9), (8, 10), (9, 9), (9, 10)]
        .into_iter()
        .map(|(x, y)| at(&state, x, y))
        .collect();
    relievers.sort_unstable();
    let ally = foundries(&state, PlayerId(1))[0];
    let own = foundries(&state, PlayerId(0))[0];
    advance_to(&mut state, 12, &[]);
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["missions"] = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {"task": "defend", "asset": ally.0, "phase": {"engage": {"focus": null}}},
            "since": 0,
            "units": relievers,
            "goal": {"x": 4, "y": 14},
        }],
    });
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(
        defends(&trace.unwrap().missions).contains(&own),
        "{commands:?}"
    );
    assert!(
        hunts(&commands)
            .iter()
            .any(|(units, _)| units.iter().any(|unit| relievers.contains(unit))),
        "the ally's defenders are sent home: {commands:?}"
    );
}

/// Four starts in the corners of a square field.
const CORNERS: [&str; 24] = [
    "########################",
    "#......................#",
    "#......................#",
    "#..1...............2...#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#......................#",
    "#..3...............4...#",
    "#......................#",
    "#......................#",
    "#......................#",
    "########################",
];

#[test]
fn mirrored_seats_pick_mirrored_rivals_among_equals() {
    let mut scenario = trio([None, None, None]);
    scenario.map = CORNERS.map(str::to_owned).to_vec();
    scenario.players.push(PlayerSpec {
        name: "far".into(),
        ..scenario.players[0].clone()
    });
    scenario.units.clear();
    let state = scenario.build().unwrap();
    let model = map(&scenario);
    let rival = |seat: u8| {
        let observation = ObservationData::fog_honest(&state, PlayerId(seat));
        let scratch = scratch(&observation, &model);
        Missions::default().rival(&scratch, &observation, &model, traits(50))
    };
    let mirrored = |seat: PlayerId| PlayerId(3 - seat.0);
    assert!(
        matches!(rival(0), Some(PlayerId(1 | 2))),
        "premise: a neighbour, not the far corner"
    );
    assert_eq!(rival(3), rival(0).map(mirrored));
    assert_eq!(rival(2), rival(1).map(mirrored));
}
