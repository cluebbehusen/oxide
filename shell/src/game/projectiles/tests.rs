use super::*;
use crate::game::Game;
use chassis::grid::TilePos;
use oxide_sim::{Command, PlayerCommand, PlayerId, Scenario, UnitId};

#[test]
fn edge_release_uses_the_firing_pose_before_egress_and_survives_shooter_loss() {
    let kind = UnitKind::Condor;
    let mut map = vec!["........................".to_string(); 20];
    map[1] = ".1......................".into();
    map[17] = ".....................2..".into();
    let scenario: Scenario = serde_json::from_value(serde_json::json!({
        "name": "Edge release", "map": map,
        "players": [
            {"name":"Own", "scrap":0, "bot":false},
            {"name":"Enemy", "scrap":0, "bot":false}
        ],
        "units": [{"player":0, "kind":kind, "x":20, "y":10}],
        "buildings": [{"player":1, "kind":"barricade", "x":22, "y":10}]
    }))
    .unwrap();
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    value["units"][0]["heading"] = serde_json::json!(0);
    let mut state: State = serde_json::from_value(value).unwrap();
    let report = state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Attack {
            units: vec![UnitId(0)],
            target: Target::Building(oxide_sim::BuildingId(2)).into(),
            queue: false,
        },
    }]);
    assert_ne!(
        state.unit(UnitId(0)).unwrap().heading,
        0,
        "egress bends at the edge"
    );
    assert!(!state.shells().is_empty());
    let mut releases = ProjectileReleases::default();
    releases.observe(&state, &report.events);
    let check = |releases: &ProjectileReleases, state: &State| {
        for i in 0..state.shells().len() {
            let pose = releases.release(state.shells(), i).unwrap();
            assert_eq!(pose.heading, vec2(1.0, 0.0));
            assert_eq!(pose.kind, kind);
        }
    };
    check(&releases, &state);
    let mut value = serde_json::to_value(&state).unwrap();
    value["units"] = serde_json::json!([]);
    for view in value["vision"].as_array_mut().unwrap() {
        view["tracking"]["tracks"] = serde_json::json!([]);
    }
    let after_loss: State = serde_json::from_value(value).unwrap();
    let mut releases = ProjectileReleases::default();
    releases.observe(&after_loss, &report.events);
    check(&releases, &after_loss);
}

#[test]
fn replay_projectile_releases_retain_heading_and_simulation_parity() {
    let scenario: Scenario = serde_json::from_value(serde_json::json!({
        "name": "Bomber release", "map": [
            "....................................",
            ".1............................2.....",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "....................................",
            "...................................."
        ],
        "players": [
            {"name":"You", "scrap":0, "bot":false},
            {"name":"Target", "scrap":0, "bot":true}
        ],
        "units": [{"player":0, "kind":"condor", "x":16, "y":8}],
        "buildings": [{"player":1, "kind":"repair_bay", "x":25, "y":7}]
    }))
    .unwrap();
    let mut game = Game::with_viewport(scenario, vec2(1440.0, 900.0)).unwrap();
    game.pending.push(PlayerCommand {
        player: PlayerId(0),
        command: Command::Hunt {
            units: vec![UnitId(0)],
            goal: TilePos::new(27, 8),
            queue: false,
        },
    });
    let mut condor = None;
    for _ in 0..240 {
        game.do_tick();
        let shells = game.state.shells();
        for (index, shell) in shells.iter().enumerate() {
            let pose = game
                .presentation
                .projectile_releases
                .release(shells, index)
                .unwrap();
            if shell.shooter == Target::Unit(UnitId(0)) {
                assert_eq!(pose.kind, UnitKind::Condor);
                condor = Some((shell.clone(), pose));
            }
        }
        if condor.is_some() {
            break;
        }
    }
    let (shell, pose) = condor.expect("Condor completes its approach and releases");
    let mut replay = game.recorder.clone();
    replay.meta.ticks = Some(game.state.current_tick());
    let mut resumed = Game::from_replay(replay).unwrap();
    assert_eq!(resumed.hash_hex(), game.hash_hex());
    let lookup = |game: &Game| {
        let index = game.state.shells().iter().position(|s| s == &shell)?;
        game.presentation
            .projectile_releases
            .release(game.state.shells(), index)
    };
    assert_eq!(lookup(&resumed), Some(pose));
    game.advance_ticks(2);
    assert_eq!(lookup(&game), Some(pose));
    resumed.advance_ticks(2);
    assert_eq!(resumed.hash_hex(), game.hash_hex());
    game.advance_ticks(60);
    assert_eq!(lookup(&game), None);
    resumed.replace_state_after_jump(&game.state);
    assert_eq!(lookup(&resumed), None);
}
