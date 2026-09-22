use chassis::replay::Replay;
use oxide_kit::GameReplay;
use oxide_sim::{Command, PlayerCommand, PlayerId, SIM_VERSION, Scenario, UnitId};

pub(crate) fn replay_fixture() -> GameReplay {
    let scenario = Scenario::skirmish();
    let state = scenario.build().expect("skirmish builds");
    let unit = |seat| {
        state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(seat))
            .expect("each skirmish seat starts with a unit")
            .id
    };
    let mut replay = Replay::new(SIM_VERSION, scenario);
    replay.record(0, stop(0, unit(0)));
    replay.record(3, stop(1, unit(1)));
    replay.record(5, stop(0, unit(0)));
    replay.record(8, stop(1, unit(1)));
    replay.record(10, stop(0, unit(0)));
    replay.meta.ticks = Some(12);
    replay
}

pub(crate) fn stop(player: u8, unit: UnitId) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command: Command::Stop { units: vec![unit] },
    }
}
