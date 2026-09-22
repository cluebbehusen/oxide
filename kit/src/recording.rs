//! World-only recording origins. Controller memory belongs to live recovery.

use crate::GameReplay;
use chassis::replay::{RecordingOrigin, ReplayError};
use oxide_sim::{SIM_VERSION, Scenario, State};
use serde::{Deserialize, Serialize};

/// A same-version world at the first available absolute recording tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldOrigin {
    version: u32,
    sim_version: String,
    state: State,
    snapshot_binding: u64,
}

impl WorldOrigin {
    /// Captures a world without controllers, pending inputs, or historical ticks.
    /// The host is responsible for supplying this world's original scenario.
    pub fn capture(scenario: &Scenario, state: &State) -> Result<Self, ReplayError> {
        let origin = Self {
            version: 1,
            sim_version: SIM_VERSION.into(),
            state: state.clone(),
            snapshot_binding: crate::checkpoint::snapshot_binding(scenario, state),
        };
        origin.validate(scenario)?;
        Ok(origin)
    }
}

impl RecordingOrigin<Scenario> for WorldOrigin {
    fn start_tick(&self) -> u64 {
        self.state.current_tick()
    }

    fn validate(&self, scenario: &Scenario) -> Result<(), ReplayError> {
        if self.version != 1 || self.sim_version != SIM_VERSION {
            return Err(ReplayError::Invalid(
                "unsupported world origin revision or simulation version".into(),
            ));
        }
        self.state
            .validate_invariants()
            .map_err(|error| ReplayError::Invalid(error.to_string()))?;
        crate::checkpoint::validate_setup(scenario, &self.state)
            .map_err(|error| ReplayError::Invalid(error.to_string()))?;
        if self.snapshot_binding != crate::checkpoint::snapshot_binding(scenario, &self.state) {
            return Err(ReplayError::Invalid(
                "recording scenario/world binding mismatch".into(),
            ));
        }
        Ok(())
    }
}

/// Builds a legacy scenario start or clones the validated world origin.
pub fn initial_state(replay: &GameReplay) -> anyhow::Result<State> {
    if let Some(origin) = &replay.origin {
        origin.validate(&replay.setup)?;
        Ok(origin.state.clone())
    } else {
        Ok(replay.setup.build()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::{Command, PlayerCommand, PlayerId};

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
        let replay: GameReplay =
            serde_json::from_slice(&serde_json::to_vec(&replay).unwrap()).unwrap();
        assert_eq!(
            crate::runner::run_replay(&replay, None, false)
                .unwrap()
                .hash(),
            end.hash()
        );
        assert!(crate::runner::run_replay(&replay, Some(36), false).is_err());
        let stats = crate::stats::compute(&replay, 3).unwrap();
        assert_eq!(stats.sample_ticks.first(), Some(&37));
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
        for change in [0, 1] {
            let mut bad = replay.clone();
            let origin = bad.origin.as_mut().unwrap();
            if change == 0 {
                origin.version += 1;
            } else {
                origin.sim_version = "other".into();
            }
            assert!(bad.validate(None).is_err());
        }
    }
}
