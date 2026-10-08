use super::*;
use crate::game::Game;
use chassis::grid::as_index;
use macroquad::prelude::vec2;
use oxide_kit::recovery::BuildIdentity;
use oxide_sim::{Command, PlayerCommand, Scenario};

#[test]
fn boundary_sight_matches_simulation_discs_on_an_expanded_map() {
    for shared_sight in [false, true] {
        let mut scenario = Scenario::skirmish();
        if shared_sight {
            for player in &mut scenario.players {
                player.team = Some(0);
            }
            let mut enemy = scenario.players[1].clone();
            enemy.team = Some(1);
            scenario.players.push(enemy);
            scenario.map[12].replace_range(2..3, "3");
        }
        let state = scenario.build().unwrap();
        let hash = state.hash();
        let fog = BoundaryFog::new(&state, PlayerId(0));
        let width = state.map().width();
        let height = state.map().height();
        let blank = ".".repeat(as_index(width + PAD * 2));
        let padding = ".".repeat(PAD as usize);
        let mut rows = vec![blank.clone(); PAD as usize];
        rows.extend(
            scenario
                .map
                .iter()
                .map(|row| format!("{padding}{row}{padding}")),
        );
        rows.extend(vec![blank; PAD as usize]);
        scenario.map = rows;
        for unit in &mut scenario.units {
            unit.x += PAD;
            unit.y += PAD;
        }
        for building in &mut scenario.buildings {
            building.x += PAD;
            building.y += PAD;
        }
        let expanded = scenario.build().unwrap();
        for y in -PAD..height + PAD {
            for x in -PAD..width + PAD {
                if (0..width).contains(&x) && (0..height).contains(&y) {
                    continue;
                }
                let tile = TilePos::new(x, y);
                assert_eq!(
                    fog.visible(tile),
                    expanded.vision(PlayerId(0)).visible(tile.offset(PAD, PAD)),
                    "{tile}, shared sight: {shared_sight}"
                );
                assert_eq!(fog.explored(tile), fog.visible(tile));
            }
        }
        assert_eq!(state.hash(), hash);
    }
}

#[test]
fn grazing_sight_reveals_only_the_near_boundary_depth() {
    let state = Scenario::skirmish().build().unwrap();
    let mut fog = BoundaryFog::new(&state, PlayerId(0));
    fog.cells.fill(0);
    fog.visible.clear();
    fog.stamp(TilePos::new(15, 3), (1, 1), 4);
    assert!(fog.visible(TilePos::new(15, -1)));
    assert!(!fog.visible(TilePos::new(15, -2)));
    assert!(!fog.visible(TilePos::new(14, -1)));
    assert!(!fog.visible(TilePos::new(15, -100)));
    fog.stamp(TilePos::new(15, 1), (1, 1), 4);
    assert!(fog.visible(TilePos::new(15, -3)));
    assert!(fog.visible(TilePos::new(14, -2)));
    assert!(!fog.visible(TilePos::new(14, -3)));
}

#[test]
fn boundary_exploration_survives_movement_bulk_ticks_and_resume() {
    let mut scenario = Scenario::skirmish();
    scenario.map = vec!["........................................".to_string(); 40];
    scenario.map[20].replace_range(20..21, "1");
    scenario.map[30].replace_range(30..31, "2");
    scenario.units.retain(|unit| unit.player == 0);
    scenario.units.truncate(1);
    scenario.units[0].x = 3;
    scenario.units[0].y = 3;
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    let tile = TilePos::new(3, -1);
    assert!(game.presentation.boundary_fog.visible(tile));
    let id = game.state.units()[0].id;
    game.pending.push(PlayerCommand {
        player: game.presentation.human,
        command: Command::Run {
            units: vec![id],
            goal: TilePos::new(15, 15),
            queue: false,
        },
    });
    game.advance_ticks(300);
    assert!(!game.presentation.boundary_fog.visible(tile));
    assert!(game.presentation.boundary_fog.explored(tile));
    let mut replay = game.recorder.clone();
    replay.meta.ticks = Some(game.state.current_tick());
    let recovered = crate::game::checkpoint::RestoredGame::recover(
        oxide_kit::recovery::Inspection {
            kind: oxide_kit::recovery::RecordingKind::LiveMatch,
            build: BuildIdentity::default(),
            session: "boundary-exploration".into(),
            replay: replay.clone(),
            checkpoint: None,
            prepared: None,
            issue: None,
            clean: false,
        },
        || false,
    )
    .unwrap()
    .install();
    assert_eq!(game.hash_hex(), recovered.hash_hex());
    assert_eq!(
        game.presentation.boundary_fog,
        recovered.presentation.boundary_fog
    );
    let resumed = Game::from_replay(replay).unwrap();
    assert_eq!(game.hash_hex(), resumed.hash_hex());
    assert_eq!(
        game.presentation.boundary_fog,
        resumed.presentation.boundary_fog
    );
}
