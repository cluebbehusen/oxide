use super::*;
use oxide_sim::{Command, PlayerCommand, PlayerId, SIM_VERSION};

fn segment() -> (GameReplay, State, State) {
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    for _ in 0..37 {
        state.tick(&[]);
    }
    let start = state.clone();
    let mut replay = GameReplay::with_origin(
        SIM_VERSION,
        scenario.clone(),
        WorldOrigin::capture(&scenario, &state).unwrap(),
    )
    .unwrap();
    let command = PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: oxide_sim::BuildingId(0),
            kind: oxide_sim::UnitKind::Harvester,
        },
    };
    replay.record(37, command.clone());
    state.tick(&[command]);
    for _ in 38..46 {
        state.tick(&[]);
    }
    replay.meta.ticks = Some(46);
    (replay, start, state)
}

#[test]
fn checkpoint_origin_replays_seeks_and_samples_only_its_suffix() {
    let (replay, start, end) = segment();
    let replay: GameReplay = serde_json::from_slice(&serde_json::to_vec(&replay).unwrap()).unwrap();
    assert_eq!(
        crate::runner::run_replay(&replay, None).unwrap().hash(),
        end.hash()
    );
    assert!(crate::runner::run_replay(&replay, Some(36)).is_err());
    let stats = crate::stats::compute(&replay, 3).unwrap();
    assert_eq!(stats.sample_ticks, [37, 40, 43, 46]);
    let partial = crate::stats::compute(&replay, 4).unwrap();
    assert_eq!(partial.sample_ticks, [37, 41, 45, 46]);
    assert_eq!(stats.final_tick, 46);
    let mut playback = crate::playback::Playback::load(replay).unwrap();
    assert_eq!(playback.position(), 37);
    playback.seek(46);
    assert_eq!(playback.state.hash(), end.hash());
    playback.seek(0);
    assert_eq!(playback.state.hash(), start.hash());
    while !playback.seek_step(46, 2) {}
    assert_eq!(playback.state.hash(), end.hash());
}

#[test]
fn checkpoint_origin_validates_absolute_bounds_and_world_identity() {
    let (replay, _, _) = segment();
    let mut empty = replay.clone();
    empty.commands.clear();
    empty.meta.ticks = None;
    assert_eq!(crate::replay_duration(&empty), 37);
    assert!(empty.validate(Some(SIM_VERSION)).is_ok());
    let mut bad = replay.clone();
    bad.commands[0].tick = 36;
    assert!(bad.validate(None).is_err());
    let mut bad = empty.clone();
    bad.meta.ticks = Some(36);
    assert!(bad.validate(None).is_err());
    let mut bad = replay.clone();
    bad.setup.seed += 1;
    assert!(bad.validate(None).is_err());
    let mut bad = replay.clone();
    bad.origin.as_mut().unwrap().state.tick(&[]);
    assert!(bad.validate(None).is_err());
}
