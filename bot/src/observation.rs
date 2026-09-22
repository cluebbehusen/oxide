//! Player knowledge with bot-owned derived navigation inputs.
pub use oxide_sim::observation::{
    BuildingObs, CarriedUnitObs, OBSERVATION_VERSION, ObservationData, UnitObs,
};
use serde::{Deserialize, Serialize};
/// Player knowledge with lazily prepared, immutable navigation inputs.
/// Mutable access invalidates derived inputs before any field can change.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Observation {
    data: ObservationData,
    #[serde(skip)]
    navigation: std::sync::OnceLock<std::sync::Arc<super::navigation::inputs::NavigationInputs>>,
}

impl Observation {
    /// Own an observation snapshot. Derived inputs are prepared only when queried.
    pub fn from_data(data: ObservationData) -> Self {
        Self {
            data,
            navigation: Default::default(),
        }
    }

    pub(super) fn navigation(&self) -> &super::navigation::inputs::NavigationInputs {
        self.navigation.get_or_init(Default::default)
    }
}

impl std::ops::Deref for Observation {
    type Target = ObservationData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl std::ops::DerefMut for Observation {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.navigation.take();
        &mut self.data
    }
}

impl std::fmt::Debug for Observation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.data.fmt(f)
    }
}
impl PartialEq for Observation {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}
impl Eq for Observation {}

#[cfg(test)]
impl Default for Observation {
    fn default() -> Self {
        Self::from_data(crate::test_support::observation_data())
    }
}

impl Observation {
    /// Capture current player knowledge and prepare navigation only on demand.
    pub fn fog_honest(state: &oxide_sim::State, player: oxide_sim::PlayerId) -> Self {
        Self::from_data(ObservationData::fog_honest(state, player))
    }
    /// Capture an omniscient diagnostic view. Never use for player-facing decisions.
    pub fn omniscient(state: &oxide_sim::State, player: oxide_sim::PlayerId) -> Self {
        Self::from_data(ObservationData::omniscient(state, player))
    }
}
