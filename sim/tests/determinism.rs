//! Command replay and serialized state determinism.
mod common;
use chassis::grid::TilePos;
use chassis::replay::Replay;
use common::{cmd, open_arena, unit};
use oxide_sim::{Command, Order, PlayerCommand, SIM_VERSION, Scenario, State, UnitKind};

#[test]
fn replay_roundtrip_executes_and_reproduces_advance() {
    let scenario = open_arena(24, 12, vec![unit(0, UnitKind::Sentinel, 3, 6)]);
    let mut live = scenario.build().unwrap();
    let mover = live.units()[0].id;
    let goal = TilePos::new(18, 6);
    let advance = cmd(
        0,
        Command::Advance {
            units: vec![mover],
            goal,
            queue: false,
        },
    );
    let mut replay = Replay::new(SIM_VERSION, scenario);
    replay.record(live.current_tick(), advance.clone());
    live.tick(&[advance]);
    assert_eq!(live.unit(mover).unwrap().order, Order::Advance { goal });
    for _ in 1..100 {
        live.tick(&[]);
    }
    let live_hash = live.hash();

    let replay: Replay<Scenario, PlayerCommand> =
        serde_json::from_str(&serde_json::to_string(&replay).unwrap()).unwrap();
    assert!(matches!(
        replay.commands.as_slice(),
        [stamped] if matches!(
            stamped.command.command,
            Command::Advance { goal: recorded, .. } if recorded == goal
        )
    ));

    let mut replayed = replay.setup.build().unwrap();
    let mut cursor = replay.cursor();
    for tick in 0..100 {
        let commands: Vec<PlayerCommand> = cursor
            .take_tick(replayed.current_tick())
            .iter()
            .map(|stamped| stamped.command.clone())
            .collect();
        replayed.tick(&commands);
        if tick == 0 {
            assert_eq!(replayed.unit(mover).unwrap().order, Order::Advance { goal });
        }
    }
    assert!(cursor.is_finished());
    assert_eq!(replayed.hash(), live_hash);
    assert_eq!(replayed.unit(mover).unwrap().order, Order::Advance { goal });
}

#[test]
fn serde_roundtrip_preserves_queued_programs() {
    // Non-empty queues serialize through skip-if-default fields — a shape
    // no 0.4 state ever had. A mid-patrol snapshot must survive losslessly
    // and keep ticking identically.
    use chassis::grid::TilePos;
    use oxide_sim::{Command, PlayerCommand, PlayerId};
    let mut state = Scenario::skirmish().build().unwrap();
    let movers: Vec<_> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Patrol {
            units: movers,
            waypoints: vec![TilePos::new(10, 10), TilePos::new(14, 6)],
        },
    }]);
    for _ in 0..100 {
        state.tick(&[]);
    }
    assert!(
        state.units().iter().any(|u| u.looping),
        "test premise: someone is patrolling"
    );
    let json = serde_json::to_string(&state).unwrap();
    let mut restored: State = serde_json::from_str(&json).unwrap();
    assert_eq!(state.hash(), restored.hash(), "roundtrip must be lossless");
    for _ in 0..100 {
        state.tick(&[]);
        restored.tick(&[]);
    }
    assert_eq!(state.hash(), restored.hash());
}
