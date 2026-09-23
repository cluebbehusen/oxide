//! Open-ended unit scenes use the ordinary command and persistence boundaries.
use chassis::grid::TilePos;
use oxide_sim::scenario::{ScenarioError, ScenarioMode};
use oxide_sim::{Command, Event, PlayerCommand, PlayerId, Scenario, State};

fn sandbox() -> Scenario {
    Scenario::from_json(
        r#"{
            "name": "Unit sandbox", "seed": 7, "mode": "sandbox",
            "map": ["............", "............", "............", "............", "............", "............"],
            "players": [{"name": "Local", "faction": "ferrous", "scrap": 0, "bot": false}],
            "units": [{"player": 0, "kind": "scuttler", "x": 2, "y": 2}]
        }"#,
    )
    .unwrap()
}

#[test]
fn units_without_foundries_accept_commands_and_keep_running_after_restore() {
    let mut state = sandbox().build().unwrap();
    assert!(state.buildings().is_empty());
    let unit = state.units()[0].id;
    let start = state.units()[0].pos;
    let command = PlayerCommand {
        player: PlayerId(0),
        command: Command::Move {
            units: vec![unit],
            goal: TilePos::new(9, 2),
            queue: false,
        },
    };
    assert!(!state.tick(&[command]).events.iter().any(|event| {
        matches!(
            event,
            Event::CommandRejected { .. } | Event::GameOver { .. }
        )
    }));
    let mut restored: State =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    for _ in 0..200 {
        state.tick(&[]);
        restored.tick(&[]);
        state.validate_invariants().unwrap();
        assert_eq!(state.hash(), restored.hash());
    }
    assert_ne!(state.units()[0].pos, start);
    assert_eq!(state.mode(), ScenarioMode::Sandbox);
    assert_eq!(state.current_tick(), 201);
    assert!(state.result().is_none());
    assert!(
        state
            .players()
            .iter()
            .all(|player| player.eliminated_at.is_none())
    );
}

#[test]
fn sandbox_allows_allied_seats_and_optional_anchors_but_not_invalid_entities() {
    let mut scenario = sandbox();
    scenario.players[0].team = Some(7);
    scenario.players.push(scenario.players[0].clone());
    scenario.map[0] = "1...........".into();
    let state = scenario.build().unwrap();
    assert_eq!(state.buildings().len(), 1);
    assert_eq!(state.buildings()[0].player, PlayerId(0));
    scenario.units[0].player = 2;
    assert!(matches!(scenario.build(), Err(ScenarioError::BadUnit(0))));
    scenario.units[0].player = 0;
    scenario.map[0] = "1.........3.".into();
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::ExtraAnchor(..))
    ));
}

#[test]
fn surrender_stops_a_seats_commands_without_ending_a_sandbox() {
    let mut state = sandbox().build().unwrap();
    let unit = state.units()[0].id;
    state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Surrender,
    }]);
    let report = state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Stop { units: vec![unit] },
    }]);
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. }))
    );
    assert!(state.result().is_none());
    state.validate_invariants().unwrap();
}

#[test]
fn passive_seats_still_fight_without_automatic_elimination() {
    let mut scenario = sandbox();
    scenario.players.push(scenario.players[0].clone());
    let mut enemy = scenario.units[0];
    enemy.player = 1;
    enemy.x = 5;
    scenario.units.push(enemy);
    let mut state = scenario.build().unwrap();
    let starting_hp: u32 = state.units().iter().map(|unit| unit.hp).sum();
    for _ in 0..600 {
        state.tick(&[]);
    }
    let remaining_hp: u32 = state.units().iter().map(|unit| unit.hp).sum();
    assert!(remaining_hp < starting_hp);
    assert_eq!(state.current_tick(), 600);
    assert!(state.result().is_none());
    assert!(
        state
            .players()
            .iter()
            .all(|player| player.eliminated_at.is_none())
    );
    state.validate_invariants().unwrap();
}

#[test]
fn default_matches_retain_their_setup_rules_and_wire_shape() {
    let mut scenario = sandbox();
    scenario.mode = ScenarioMode::Match;
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::MissingAnchor(PlayerId(0)))
    ));
    let mut ordinary = Scenario::skirmish();
    assert!(
        serde_json::to_value(&ordinary)
            .unwrap()
            .get("mode")
            .is_none()
    );
    let state = ordinary.build().unwrap();
    assert!(serde_json::to_value(&state).unwrap().get("mode").is_none());
    ordinary.mode = ScenarioMode::Sandbox;
    assert_ne!(state.hash(), ordinary.build().unwrap().hash());
    ordinary.mode = ScenarioMode::Match;
    for player in &mut ordinary.players {
        player.team = Some(0);
    }
    assert!(matches!(ordinary.build(), Err(ScenarioError::OneTeam)));
}
