//! World-only recording origins. Controller memory belongs to live recovery.

use crate::GameReplay;
use chassis::replay::{RecordingOrigin, ReplayError};
use oxide_sim::{Scenario, State};
use serde::{Deserialize, Serialize};

/// A world at the first available absolute recording tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldOrigin {
    state: State,
    snapshot_binding: u64,
}

impl WorldOrigin {
    /// Captures a world without controllers, pending inputs, or historical ticks.
    /// The host is responsible for supplying this world's original scenario.
    pub fn capture(scenario: &Scenario, state: &State) -> Result<Self, ReplayError> {
        let origin = Self {
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

/// Builds the scenario start, or clones the validated world origin when the
/// replay has one.
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
