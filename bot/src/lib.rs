//! Bots: policies that read [`Observation`] and emit
//! [`oxide_sim::PlayerCommand`]s, exactly like a mouse or the debug socket.
//!
//! The player-facing controller is layered:
//!
//! ```text
//! immutable PublicMapBriefing + fog-honest Observation
//!   -> oriented public priors + StrategicIntelligence + battlefield assessment
//!   -> persistent playbooks + UtilityPolicy proposals
//!   -> AllocationSession (seven domains, bounded experience-adjusted return)
//!   -> admitted Intent[]
//!   -> Executive (exact ground missions and tactical combat)
//!   -> PlayerCommand[]
//! ```
//!
//! [`oxide_sim::observation`] builds what a bot may know; [`StrategicIntelligence`]
//! distinguishes current evidence from memory; persistent planners,
//! Utility policy, defense, and the standing-force policy propose competing
//! work; the allocation session admits a current-funded portfolio; and
//! [`Executive`] owns exact unit reservations and lowers the resulting
//! [`Intent`]s to commands.
//! [`Brain::scripted`] is the observation-only decision core. [`SeatBot`]
//! captures observations and gates inactive matches at the host boundary.

mod allocation;
pub mod battlefield;
pub mod brain;
pub mod briefing;
pub mod checkpoint;
pub mod difficulty;
pub mod executive;
mod experience;
pub mod intelligence;
mod lift;
mod navigation;
pub mod observation;
pub mod observer;
pub mod orient;
mod planning;
mod production;
pub mod profile;
mod query_work;
mod raid;
mod resources;
mod standing_force;
mod strategy;
mod team;
pub mod trace;
mod utility;

pub use brain::Brain;
pub use briefing::{PublicMapBriefing, StartingFoundry};
pub use difficulty::DifficultyTuning;
pub use executive::{Army, ArmyId, ArmyState, Executive, Intent};
pub use intelligence::{
    AirDefenseAssessment, AirDefenseContact, AirDefenseEvidence, AirDefenseSource, BuildingContact,
    ContactEvidence, StrategicIntelligence, UnitContact,
};
pub use lift::{LiftAirSupport, LiftManifest, LiftOperation, LiftPhase};
pub use observation::{BuildingObs, CarriedUnitObs, Observation, UnitObs};
pub use orient::Orientation;
pub use profile::{PersonalityTraits, ResolvedProfile, Specialty};
pub use raid::{RaidExitReason, RaidObjective, RaidOperation, RaidPhase};
pub use strategy::{AirOperation, AirOperationPhase, AirRecoveryReason};
pub use team::{TeamReliefExitReason, TeamReliefOperation, TeamReliefPhase};
pub use trace::{
    AssignedForceTrace, CapabilityTrace, ChannelEffects, ChannelPhase, ChannelState, ChannelTrace,
    ChannelTraces, ConnectedForceStatus, ConnectedForceTrace, ConnectedPackageTrace,
    ConnectedRecoveryReasonTrace, ConnectedRejectionReasonTrace, ConnectedTargetTrace,
    CoreGateTrace, DECISION_TRACE_VERSION, DecisionControlFlow, DecisionTrace, EvidenceTrace,
    ForceDemandsTrace, ForceFamilyTrace, GateTrace, LoweringTrace, ProviderDemandTrace,
    RaidAttentionTrace, RejectedConnectedCandidateTrace, ScrapBudgetTrace, TargetEvidenceTrace,
    TracedBotAct, UtilityTrace,
};
pub use utility::Dials;

mod runtime;
pub use runtime::{SeatBot, seat_bots};

#[cfg(test)]
mod test_support;
