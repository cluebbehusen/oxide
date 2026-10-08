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
mod tests;
