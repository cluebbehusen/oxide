//! Opt-in, fog-honest diagnostics for one player-facing bot decision.
//!
//! Traces are observational output. They are not controller memory, simulation
//! state, events, or replay input, and nothing in the policy may read them back.

use super::observation::Observation;
use super::resources::ResourceSnapshot;
use super::strategy::force_package::{ForceFamily, ForcePackageRejection};
use super::strategy::{
    AirOperation, AirOperationOutcome, AirRecoveryReason, ConnectedPackageDiagnostics,
    ConnectedPlanRejection, RejectedConnectedCandidate, StrategicPlanner,
};
use super::{BuildingContact, ContactEvidence, StrategicIntelligence};
use crate::PlayerCommand;
use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::stats::{BuildingKind, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;
use serde::Serialize;
use std::collections::BTreeMap;

/// Schema version for serialized decision traces.
pub const DECISION_TRACE_VERSION: u32 = 3;

const RESOURCE_FORECAST_TICKS: Tick = crate::TICKS_PER_SECOND as Tick * 60;

/// Commands from one bot act plus its optional player-facing diagnostic.
///
/// The trace is absent for the frozen Overseer and for ticks on which the
/// player-facing controller does not think.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracedBotAct {
    /// Ordinary commands emitted by the controller.
    pub commands: Vec<PlayerCommand>,
    /// Observational decision trace, when one was produced.
    pub trace: Option<DecisionTrace>,
}

/// One fixed-shape record of a player-facing decision tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionTrace {
    /// Trace schema version.
    pub version: u32,
    /// Simulation tick observed by the controller.
    pub tick: Tick,
    /// Seat whose fog-honest view produced this decision.
    pub player: PlayerId,
    /// Which top-level path completed the decision.
    pub control_flow: DecisionControlFlow,
    /// Compact facts from the already-built fog-honest observation.
    pub evidence: EvidenceTrace,
    /// Typed current resources and a bounded completed-income forecast.
    pub resources: ResourceTrace,
    /// Coordinator-owned gates. `None` means the full strategic path did not
    /// reach that gate.
    pub gates: GateTrace,
    /// Existing coordinator scrap holds. Absent on an early recovery return.
    pub budget: Option<ScrapBudgetTrace>,
    /// Fixed current planner channels, in schema rather than insertion order.
    pub channels: ChannelTraces,
    /// Bounded evidence for the connected force package and its assigned force.
    pub connected_force: ConnectedForceTrace,
    /// Input and output size of the utility-policy pass.
    pub utility: UtilityTrace,
    /// Final intent-to-command lowering summary.
    pub lowering: LoweringTrace,
}

impl DecisionTrace {
    fn from_observation(observation: &Observation) -> Self {
        let resources = ResourceSnapshot::from_observation(observation);
        Self {
            version: DECISION_TRACE_VERSION,
            tick: observation.tick,
            player: observation.me,
            control_flow: DecisionControlFlow::Policy,
            evidence: EvidenceTrace {
                current_enemy_units: bounded_count(observation.enemy_units.len()),
                current_enemy_buildings: bounded_count(
                    observation
                        .enemy_buildings
                        .iter()
                        .filter(|building| building.seen)
                        .count(),
                ),
                remembered_enemy_buildings: bounded_count(
                    observation
                        .enemy_buildings
                        .iter()
                        .filter(|building| !building.seen)
                        .count(),
                ),
                radar_blips: bounded_count(observation.blips.len()),
            },
            resources: ResourceTrace::from_snapshot(observation.tick, &resources),
            gates: GateTrace::default(),
            budget: None,
            channels: ChannelTraces::default(),
            connected_force: ConnectedForceTrace::default(),
            utility: UtilityTrace::default(),
            lowering: LoweringTrace::default(),
        }
    }
}

/// Bounded resource evidence from the typed, fog-honest snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResourceTrace {
    /// Current bank; forecast income is never added to this amount.
    pub current_scrap: u32,
    /// Bounded future interval used for the completed-income forecast. This is
    /// normally one minute and shortens only near the tick range limit.
    pub forecast_horizon_ticks: Tick,
    /// Income due from currently completed recurring sources in that interval.
    pub forecast_scrap: u32,
    /// Construction-capable units without an observed obligation.
    pub free_builders: u32,
    /// Construction-capable units already building, founding, repairing, or salvaging.
    pub obligated_builders: u32,
    /// Completed, live producers visible to this player.
    pub completed_producers: u32,
    /// Queue positions currently open across those producers.
    pub open_producer_slots: u32,
}

impl ResourceTrace {
    fn from_snapshot(observed_at: Tick, resources: &ResourceSnapshot) -> Self {
        let last_offset = RESOURCE_FORECAST_TICKS
            .saturating_sub(1)
            .min(Tick::MAX.saturating_sub(observed_at));
        let forecast_horizon_ticks = last_offset.saturating_add(1);
        let deadline = observed_at
            .checked_add(last_offset)
            .expect("the forecast offset is bounded by the remaining tick range");
        let free_builders = resources
            .builders()
            .iter()
            .filter(|builder| builder.obligation.is_none())
            .count();
        Self {
            current_scrap: resources.current_scrap().amount(),
            forecast_horizon_ticks,
            forecast_scrap: resources.forecast().income_through(deadline).amount(),
            free_builders: bounded_count(free_builders),
            obligated_builders: bounded_count(resources.builders().len() - free_builders),
            completed_producers: bounded_count(resources.producers().len()),
            open_producer_slots: bounded_count(resources.producer_slots().len()),
        }
    }
}

/// Top-level route through [`super::Brain::act_traced`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionControlFlow {
    /// The strategic and utility policies ran normally.
    Policy,
    /// Executive safety recovery preempted the remaining policy for this tick.
    HarvesterRecovery,
}

/// Fog-honest facts already present in the controller observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EvidenceTrace {
    /// Currently visible hostile units.
    pub current_enemy_units: u32,
    /// Currently visible hostile buildings.
    pub current_enemy_buildings: u32,
    /// Hostile building ghosts currently present in the observation.
    pub remembered_enemy_buildings: u32,
    /// Unidentified hostile radar contacts.
    pub radar_blips: u32,
}

/// Current trace state of the connected-package playbook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectedForceTrace {
    /// Whether the planner is idle, executing, recovering, or just terminated.
    pub status: ConnectedForceStatus,
    /// Current fog-honest evidence for the operation's objective.
    pub target: Option<ConnectedTargetTrace>,
    /// Evidence and demand from the package's current target-driven revision.
    pub package: Option<ConnectedPackageTrace>,
    /// Exact members currently assigned by the operation.
    pub assigned: AssignedForceTrace,
    /// Current connected opportunity considered but not admitted or revised.
    /// This exists for one traced think only and is never planner memory.
    pub rejected_candidate: Option<RejectedConnectedCandidateTrace>,
}

impl Default for ConnectedForceTrace {
    fn default() -> Self {
        Self {
            status: ConnectedForceStatus::NotObserved,
            target: None,
            package: None,
            assigned: AssignedForceTrace::default(),
            rejected_candidate: None,
        }
    }
}

/// One current connected opportunity the planner could not admit or revise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RejectedConnectedCandidateTrace {
    /// Fog-honest identity and evidence of the candidate.
    pub target: ConnectedTargetTrace,
    /// Exact admission boundary that refused it.
    pub reason: ConnectedRejectionReasonTrace,
}

/// Capability family used by package-demand diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForceFamilyTrace {
    /// Vision required to establish current target evidence.
    Recon,
    /// Ground firepower required to remove targetable anti-air defenses.
    Suppression,
    /// Air firepower required to destroy the target cluster.
    Strike,
}

/// Why a current connected opportunity was not admitted or revised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ConnectedRejectionReasonTrace {
    /// The standing army has not reached the protected commitment floor.
    InsufficientStandingForce {
        /// Current eligible combat roster.
        current: u32,
        /// Minimum roster required before this voluntary operation.
        required: u32,
    },
    /// Public terrain and current obstacles provide no ground staging route.
    DisconnectedGroundRoute,
    /// A ground route exists, but the complete suppression group cannot accept
    /// every authoritative spread slot near its staging line.
    UnreachableGroupStaging {
        /// Exact suppression-provider count requested by the package.
        requested: u32,
    },
    /// The configured decision cadence cannot schedule future production.
    InvalidDecisionCadence,
    /// The preparation deadline is before this observation.
    InvalidDeadline {
        /// Tick that supplied the rejected observation.
        observed_at: Tick,
        /// Rejected absolute preparation deadline.
        deadline: Tick,
    },
    /// The candidate no longer has current evidence.
    TargetNotCurrent,
    /// The current contact is not a live, completed, valuable target.
    TargetNotActionable,
    /// Earlier accepted commitments own enough real capital to explain the
    /// package's funding shortfall.
    ProtectedFunds {
        /// Provider family whose common minimum could not be funded.
        family: ForceFamilyTrace,
        /// Cumulative scrap needed through the rejected provider.
        required_scrap: u32,
        /// Spendable current and forecast scrap available by the deadline.
        available_scrap: u32,
        /// Difference between required and available scrap.
        deadline_shortfall: u32,
        /// Current bank withheld by older commitments.
        protected_current_scrap: u32,
        /// Completed-source forecast actually reachable but withheld.
        protected_forecast_scrap: u32,
    },
    /// Even without enough older protected capital to explain the gap, the
    /// spendable economy cannot fund the common minimum.
    InsufficientSpendableScrap {
        /// Provider family whose common minimum could not be funded.
        family: ForceFamilyTrace,
        /// Cumulative scrap needed through the rejected provider.
        required_scrap: u32,
        /// Spendable current and forecast scrap available by the deadline.
        available_scrap: u32,
        /// Difference between required and available scrap.
        deadline_shortfall: u32,
    },
    /// No completed producer can train a legal provider for this family.
    MissingProviderCapability {
        /// Family with no completed production path.
        family: ForceFamilyTrace,
    },
    /// Legal production exists but cannot expose the provider before expiry.
    PreparationWindowTooShort {
        /// Family whose provider misses the fixed window.
        family: ForceFamilyTrace,
        /// Tick that supplied the rejected observation.
        observed_at: Tick,
        /// Immutable package deadline.
        deadline: Tick,
    },
    /// Current air-domain anti-air covers the cluster and ground suppression
    /// cannot target it.
    UntargetableCurrentAirDefense {
        /// Observed anti-air damage per 100 ticks.
        firepower: u64,
        /// Observed hit points that the package cannot suppress.
        hit_points: u64,
    },
}

/// Lifecycle state that can be justified without reading policy-private state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum ConnectedForceStatus {
    /// The full policy path did not inspect this planner.
    NotObserved,
    /// Focused tests disabled the planner.
    Disabled,
    /// No air operation is active.
    Idle,
    /// An island or target-reacquisition operation has no connected package.
    OtherAirOperation,
    /// A connected package is active outside recovery.
    Active,
    /// The operation is withdrawing for the recorded reason.
    Recovering(ConnectedRecoveryReasonTrace),
    /// The operation released its corridor this think.
    Released,
    /// The operation aborted its corridor this think.
    Aborted,
}

/// Fog-honest target identity and current evidence at the decision boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConnectedTargetTrace {
    /// Last known owner.
    pub player: PlayerId,
    /// Last known building kind.
    pub kind: BuildingKind,
    /// Stable footprint anchor.
    pub anchor: TilePos,
    /// Last live target id retained by the operation.
    pub last_live_id: Option<BuildingId>,
    /// Whether current memory still has live, remembered, or no contact.
    pub evidence: TargetEvidenceTrace,
}

/// Evidence strength for a connected operation's objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetEvidenceTrace {
    /// The objective is currently visible.
    Current,
    /// Only a remembered structure contact remains.
    Remembered,
    /// Current intelligence has no matching structure contact.
    Missing,
}

/// Evidence and opportunity-scaled demand from the current package revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectedPackageTrace {
    /// Tick on which the planner first owned this operation.
    pub admitted_at: Tick,
    /// Tick of the current package derivation or target-driven revision.
    pub derived_at: Tick,
    /// Fixed latest tick at which the package may finish preparation.
    pub preparation_deadline: Tick,
    /// Canonical footprint anchors admitted as the operation's target cluster.
    pub target_anchors: Vec<TilePos>,
    /// Strategic value of the target cluster at the current derivation.
    pub target_value: u64,
    /// Spendable bank exposed when this package version was derived.
    pub current_scrap: u32,
    /// Completed-source income forecast from derivation through the deadline.
    pub forecast_scrap: u32,
    /// Personality-independent minimum for a viable connected operation.
    pub minimum_capability: CapabilityTrace,
    /// Maximum capability useful against the observed opportunity.
    pub useful_capability: CapabilityTrace,
    /// Capability supplied by the selected indivisible providers.
    pub chosen_capability: CapabilityTrace,
    /// Current-visible non-suppression collateral opportunity for attack-run
    /// bombers.
    pub useful_bombing: u64,
    /// Collateral value supplied by selected attack-run bombers.
    pub chosen_bombing: u64,
    /// Exact-kind provider counts selected by the current package revision.
    pub demands: ForceDemandsTrace,
    /// All currently observed operational anti-air covering the target cluster.
    pub observed_aa_firepower: u64,
    /// Observed anti-air attached to completed structures or live grounded
    /// units that artillery can suppress.
    pub suppressible_aa_firepower: u64,
}

/// Reconnaissance, suppression, and strike capability in normalized units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CapabilityTrace {
    /// Reconnaissance capability.
    pub recon: u64,
    /// Ground-targetable anti-air suppression capability.
    pub suppression: u64,
    /// Ground-strike capability.
    pub strike: u64,
}

impl From<[u64; 3]> for CapabilityTrace {
    fn from([recon, suppression, strike]: [u64; 3]) -> Self {
        Self {
            recon,
            suppression,
            strike,
        }
    }
}

/// Selected provider counts in fixed capability-family order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForceDemandsTrace {
    /// Reconnaissance providers.
    pub recon: Vec<ProviderDemandTrace>,
    /// Ground-targetable anti-air suppression providers.
    pub suppression: Vec<ProviderDemandTrace>,
    /// Ground-strike providers.
    pub strike: Vec<ProviderDemandTrace>,
}

/// One exact-kind provider demand with a saturating diagnostic count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderDemandTrace {
    /// Requested unit kind.
    pub kind: UnitKind,
    /// Requested count, saturated at the trace representation boundary.
    pub count: u32,
}

/// Exact operation assignments, grouped by package role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct AssignedForceTrace {
    /// Whether the assignments have crossed the operation's commitment boundary.
    pub membership_frozen: bool,
    /// Assigned reconnaissance aircraft.
    pub scout: Option<UnitId>,
    /// Assigned artillery ids in canonical order.
    pub suppression: Vec<UnitId>,
    /// Assigned strike-aircraft ids in canonical order.
    pub strike: Vec<UnitId>,
}

/// Why a connected operation is recovering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectedRecoveryReasonTrace {
    /// Current sight confirmed the objective was gone.
    Complete,
    /// A required assigned provider died.
    RequiredUnitLost,
    /// Current sight found anti-air the operation cannot suppress.
    NewAirDefense,
    /// The fixed preparation deadline or a bounded tactical phase expired.
    Timeout,
    /// Current sight disproved the objective before the strike.
    ObjectiveLost,
    /// Remembered target evidence aged out.
    StaleIntelligence,
    /// No honestly plausible ground route reaches artillery staging.
    UnreachableStaging,
    /// Known peaks seal the air route.
    UnreachableAirRoute,
    /// Available resources and completed production cannot finish the package.
    PreparationInfeasible,
}

/// Coordinator-owned admission and rollback gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct GateTrace {
    /// Whether the existing core allowed a new team-relief commitment.
    pub team_relief_core_ready: Option<bool>,
    /// Opening core measured after current planner claims.
    pub opening_core: Option<CoreGateTrace>,
    /// Whether a speculative team-relief admission was rolled back.
    pub team_relief_rolled_back: bool,
    /// Whether a speculative lift admission was rolled back.
    pub lift_rolled_back: bool,
    /// Optional-raid attention calculation.
    pub raid_attention: Option<RaidAttentionTrace>,
}

/// Exact values behind the opening-core admission gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CoreGateTrace {
    /// Current projected core strength after exclusions.
    pub projected_strength: u64,
    /// Required core strength.
    pub target_strength: u64,
    /// Remaining strength shortfall.
    pub missing_strength: u64,
    /// Scrap cost of the shortfall under the current core model.
    pub missing_scrap: u32,
    /// Whether voluntary strategic work may proceed.
    pub ready: bool,
}

/// Exact inputs and result of the current optional-raid attention gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RaidAttentionTrace {
    /// Other active strategic channels.
    pub strategic_load: u32,
    /// Difficulty-owned attention capacity.
    pub attention_slots: u32,
    /// Whether a new raid may be considered.
    pub admitted: bool,
}

/// Existing per-think scrap holds and the amounts exposed downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScrapBudgetTrace {
    /// Own bank before coordinator holds.
    pub bank: u32,
    /// Capital owned by a validated persistent Foundry expansion.
    pub foundry_saving: u32,
    /// Scrap promised to accepted deferred construction.
    pub deferred_construction: u32,
    /// Capital held for an additional Airworks.
    pub airworks_capacity: u32,
    /// Capital held for one shallow Sentinel.
    pub shallow_sentinel: u32,
    /// Capital held for the authored home Extractor opening.
    pub opening_bootstrap: u32,
    /// Whether the unmet opening core closed voluntary spending.
    pub frozen: bool,
    /// Scrap exposed to an operation that predates the saved Foundry plan.
    pub prior_operation_spendable: u32,
    /// Scrap exposed to a newly admitted strategic operation after those holds.
    pub strategic_spendable: u32,
    /// Scrap claimed by strategic planners during this think.
    pub strategic_committed: u32,
    /// Of that commitment, the prospective first-carrier hold.
    pub prospective_carrier: u32,
    /// Scrap exposed to the utility policy after strategic commitments.
    pub utility_spendable: u32,
}

/// Fixed planner slots. Their shape does not depend on which channels ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ChannelTraces {
    /// Allied-base relief planner.
    pub team_relief: ChannelTrace,
    /// Connected air-and-siege planner.
    pub connected_air: ChannelTrace,
    /// Severed-ground transport planner.
    pub lift: ChannelTrace,
    /// Harassment planner.
    pub raid: ChannelTrace,
}

/// One planner's lifecycle transition and returned effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelTrace {
    /// State before this think.
    pub before: ChannelState,
    /// State after this think and any coordinator rollback.
    pub after: ChannelState,
    /// Effects returned by the planner after rollback.
    pub effects: ChannelEffects,
}

impl Default for ChannelTrace {
    fn default() -> Self {
        Self {
            before: ChannelState::NotObserved,
            after: ChannelState::NotObserved,
            effects: ChannelEffects::default(),
        }
    }
}

/// Compact current lifecycle of one legacy strategic channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "phase", rename_all = "snake_case")]
pub enum ChannelState {
    /// The full policy path did not observe this channel.
    NotObserved,
    /// Focused tests removed this optional planner.
    Disabled,
    /// The planner owns no active operation or preparing unit claim.
    Idle,
    /// The planner owns units while preparing an operation.
    Preparing,
    /// An operation is active in the named phase.
    Active(ChannelPhase),
}

/// Stable phase vocabulary shared by the fixed trace schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPhase {
    /// Team relief is moving toward its ally.
    TeamDeploying,
    /// Team relief is holding the pressured asset.
    TeamHolding,
    /// Team relief is returning home.
    TeamWithdrawing,
    /// Connected operation is reacquiring its objective.
    AirRecon,
    /// Connected operation is assembling its force.
    AirAssemble,
    /// Connected operation is suppressing known anti-air.
    AirSuppressAa,
    /// Connected operation is verifying its corridor.
    AirVerify,
    /// Connected operation committed its strike aircraft.
    AirStrike,
    /// Connected operation is recovering survivors.
    AirRecover,
    /// Lift is accumulating payload and carrier demand.
    LiftProvision,
    /// Lift is boarding exact manifests.
    LiftBoarding,
    /// Lift is waiting for coordinated support.
    LiftAwaitSupport,
    /// Lift is crossing to its landing sites.
    LiftLanding,
    /// Lift is recovering carriers and landed units.
    LiftRecover,
    /// Raid is approaching its objective.
    RaidIngress,
    /// Raid is attacking its objective.
    RaidStrike,
    /// Raid is returning home.
    RaidEgress,
}

/// Effects a strategic planner returned to the coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ChannelEffects {
    /// Ordered intents returned this think.
    pub intents: u32,
    /// Exact canonical unit claims returned this think.
    pub unit_claims: Vec<UnitId>,
    /// Current scrap spent or held by this planner.
    pub committed_scrap: u32,
}

/// Utility pass summary without pretending its current sequential channels are
/// structured investment proposals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct UtilityTrace {
    /// Strategic prelude intents supplied to Utility.
    pub input_intents: u32,
    /// Combined intents returned by Utility.
    pub output_intents: u32,
    /// Final exact unit reservations supplied to the Executive.
    pub reserved_units: u32,
}

/// Final lowering summary for this decision tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct LoweringTrace {
    /// Commands emitted by executive maintenance before policy runs.
    pub maintenance_commands: u32,
    /// Commands emitted by recovery or policy lowering.
    pub decision_commands: u32,
    /// Total commands returned by the bot.
    pub total_commands: u32,
}

/// Captures only planner diagnostics and fog-honest intelligence.
pub(super) fn connected_force_trace(
    planner: Option<&StrategicPlanner>,
    intelligence: &StrategicIntelligence,
    rejected_candidate: Option<&RejectedConnectedCandidate>,
) -> ConnectedForceTrace {
    let Some(planner) = planner else {
        return ConnectedForceTrace {
            status: ConnectedForceStatus::Disabled,
            ..ConnectedForceTrace::default()
        };
    };
    let operation = planner.air_operation();
    let package = planner
        .connected_package_diagnostics()
        .map(ConnectedPackageTrace::from_diagnostics);
    let status = match (planner.terminal_outcome(), operation) {
        (Some(AirOperationOutcome::Released { .. }), _) => ConnectedForceStatus::Released,
        (Some(AirOperationOutcome::Aborted { .. }), _) => ConnectedForceStatus::Aborted,
        (_, Some(operation)) => operation.recovery_reason.map_or_else(
            || {
                if package.is_some() {
                    ConnectedForceStatus::Active
                } else {
                    ConnectedForceStatus::OtherAirOperation
                }
            },
            |reason| ConnectedForceStatus::Recovering(reason.into()),
        ),
        (None, None) => ConnectedForceStatus::Idle,
    };
    ConnectedForceTrace {
        status,
        target: operation
            .map(|operation| ConnectedTargetTrace::from_operation(operation, intelligence)),
        package,
        assigned: operation.map_or_else(
            AssignedForceTrace::default,
            AssignedForceTrace::from_operation,
        ),
        rejected_candidate: rejected_candidate.map(RejectedConnectedCandidateTrace::from),
    }
}

impl ConnectedForceTrace {
    /// Retains the just-finished package across the one-think terminal signal.
    /// Island operations have no connected package and remain outside this
    /// diagnostic rather than masquerading as a connected release or abort.
    pub(super) fn preserve_terminal_package(&mut self, previous: Self) {
        if !matches!(
            self.status,
            ConnectedForceStatus::Released | ConnectedForceStatus::Aborted
        ) {
            return;
        }
        if previous.package.is_none() {
            self.status = ConnectedForceStatus::Idle;
            return;
        }
        if self.target.is_none() {
            self.target = previous.target;
        }
        if self.package.is_none() {
            self.package = previous.package;
        }
        if self.assigned == AssignedForceTrace::default() {
            self.assigned = previous.assigned;
        }
    }
}

impl ConnectedTargetTrace {
    fn from_operation(operation: &AirOperation, intelligence: &StrategicIntelligence) -> Self {
        let evidence = intelligence
            .buildings()
            .iter()
            .find(|contact| {
                contact.player == operation.target_player
                    && contact.kind == operation.target_kind
                    && contact.anchor == operation.target
            })
            .map_or(TargetEvidenceTrace::Missing, |contact| {
                match contact.evidence {
                    ContactEvidence::Current => TargetEvidenceTrace::Current,
                    ContactEvidence::Remembered => TargetEvidenceTrace::Remembered,
                }
            });
        Self {
            player: operation.target_player,
            kind: operation.target_kind,
            anchor: operation.target,
            last_live_id: operation.target_id,
            evidence,
        }
    }

    fn from_contact(contact: &BuildingContact) -> Self {
        Self {
            player: contact.player,
            kind: contact.kind,
            anchor: contact.anchor,
            last_live_id: contact.id,
            evidence: match contact.evidence {
                ContactEvidence::Current => TargetEvidenceTrace::Current,
                ContactEvidence::Remembered => TargetEvidenceTrace::Remembered,
            },
        }
    }
}

impl From<&RejectedConnectedCandidate> for RejectedConnectedCandidateTrace {
    fn from(candidate: &RejectedConnectedCandidate) -> Self {
        Self {
            target: ConnectedTargetTrace::from_contact(&candidate.target),
            reason: candidate.reason.into(),
        }
    }
}

impl From<ConnectedPlanRejection> for ConnectedRejectionReasonTrace {
    fn from(rejection: ConnectedPlanRejection) -> Self {
        match rejection {
            ConnectedPlanRejection::InsufficientStandingForce { current, required } => {
                Self::InsufficientStandingForce {
                    current: bounded_count(current),
                    required: bounded_count(required),
                }
            }
            ConnectedPlanRejection::DisconnectedGroundRoute => Self::DisconnectedGroundRoute,
            ConnectedPlanRejection::UnreachableGroupStaging { requested } => {
                Self::UnreachableGroupStaging {
                    requested: bounded_count(requested),
                }
            }
            ConnectedPlanRejection::Package {
                reason,
                protected_current_scrap,
                protected_forecast_scrap,
            } => package_rejection_trace(reason, protected_current_scrap, protected_forecast_scrap),
        }
    }
}

fn package_rejection_trace(
    rejection: ForcePackageRejection,
    protected_current_scrap: u32,
    protected_forecast_scrap: u32,
) -> ConnectedRejectionReasonTrace {
    match rejection {
        ForcePackageRejection::InvalidDecisionCadence => {
            ConnectedRejectionReasonTrace::InvalidDecisionCadence
        }
        ForcePackageRejection::InvalidDeadline {
            observed_at,
            deadline,
        } => ConnectedRejectionReasonTrace::InvalidDeadline {
            observed_at,
            deadline,
        },
        ForcePackageRejection::TargetNotCurrent => ConnectedRejectionReasonTrace::TargetNotCurrent,
        ForcePackageRejection::TargetNotActionable => {
            ConnectedRejectionReasonTrace::TargetNotActionable
        }
        ForcePackageRejection::InsufficientResources {
            family,
            required_scrap,
            available_scrap,
            deadline_shortfall,
        } => {
            if protected_current_scrap.saturating_add(protected_forecast_scrap)
                >= deadline_shortfall
            {
                ConnectedRejectionReasonTrace::ProtectedFunds {
                    family: family.into(),
                    required_scrap,
                    available_scrap,
                    deadline_shortfall,
                    protected_current_scrap,
                    protected_forecast_scrap,
                }
            } else {
                ConnectedRejectionReasonTrace::InsufficientSpendableScrap {
                    family: family.into(),
                    required_scrap,
                    available_scrap,
                    deadline_shortfall,
                }
            }
        }
        ForcePackageRejection::MissingCompletedProviderCapability { family } => {
            ConnectedRejectionReasonTrace::MissingProviderCapability {
                family: family.into(),
            }
        }
        ForcePackageRejection::PreparationWindowTooShort {
            family,
            observed_at,
            deadline,
        } => ConnectedRejectionReasonTrace::PreparationWindowTooShort {
            family: family.into(),
            observed_at,
            deadline,
        },
        ForcePackageRejection::UntargetableCurrentAirDefense {
            firepower,
            hit_points,
        } => ConnectedRejectionReasonTrace::UntargetableCurrentAirDefense {
            firepower,
            hit_points,
        },
    }
}

impl From<ForceFamily> for ForceFamilyTrace {
    fn from(family: ForceFamily) -> Self {
        match family {
            ForceFamily::Recon => Self::Recon,
            ForceFamily::Suppression => Self::Suppression,
            ForceFamily::Strike => Self::Strike,
        }
    }
}

impl ConnectedPackageTrace {
    fn from_diagnostics(diagnostics: ConnectedPackageDiagnostics) -> Self {
        let mut target_anchors = diagnostics.target_anchors;
        target_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
        target_anchors.dedup();
        Self {
            admitted_at: diagnostics.admitted_at,
            derived_at: diagnostics.derived_at,
            preparation_deadline: diagnostics.preparation_deadline,
            target_anchors,
            target_value: diagnostics.target_value,
            current_scrap: diagnostics.current_scrap,
            forecast_scrap: diagnostics.forecast_scrap,
            minimum_capability: diagnostics.minimum_capability.into(),
            useful_capability: diagnostics.useful_capability.into(),
            chosen_capability: diagnostics.chosen_capability.into(),
            useful_bombing: diagnostics.useful_bombing,
            chosen_bombing: diagnostics.chosen_bombing,
            demands: ForceDemandsTrace {
                recon: provider_demands(diagnostics.recon),
                suppression: provider_demands(diagnostics.suppression),
                strike: provider_demands(diagnostics.strike),
            },
            observed_aa_firepower: diagnostics.observed_aa_firepower,
            suppressible_aa_firepower: diagnostics.suppressible_aa_firepower,
        }
    }
}

impl AssignedForceTrace {
    fn from_operation(operation: &AirOperation) -> Self {
        Self {
            membership_frozen: operation.membership_frozen_at.is_some(),
            scout: operation.scout,
            suppression: canonical_ids(&operation.artillery),
            strike: canonical_ids(&operation.strike_aircraft),
        }
    }
}

impl From<AirRecoveryReason> for ConnectedRecoveryReasonTrace {
    fn from(reason: AirRecoveryReason) -> Self {
        match reason {
            AirRecoveryReason::Complete => Self::Complete,
            AirRecoveryReason::RequiredUnitLost => Self::RequiredUnitLost,
            AirRecoveryReason::NewAirDefense => Self::NewAirDefense,
            AirRecoveryReason::Timeout => Self::Timeout,
            AirRecoveryReason::ObjectiveLost => Self::ObjectiveLost,
            AirRecoveryReason::StaleIntelligence => Self::StaleIntelligence,
            AirRecoveryReason::UnreachableStaging => Self::UnreachableStaging,
            AirRecoveryReason::UnreachableAirRoute => Self::UnreachableAirRoute,
            AirRecoveryReason::PreparationInfeasible => Self::PreparationInfeasible,
        }
    }
}

fn provider_demands(demands: Vec<(UnitKind, usize)>) -> Vec<ProviderDemandTrace> {
    let mut canonical = BTreeMap::<UnitKind, usize>::new();
    for (kind, count) in demands.into_iter().filter(|(_, count)| *count > 0) {
        let total = canonical.entry(kind).or_default();
        *total = total.saturating_add(count);
    }
    canonical
        .into_iter()
        .map(|(kind, count)| ProviderDemandTrace {
            kind,
            count: bounded_count(count),
        })
        .collect()
}

fn canonical_ids(ids: &[UnitId]) -> Vec<UnitId> {
    let mut canonical = ids.to_vec();
    canonical.sort_unstable();
    canonical.dedup();
    canonical
}

/// Stack-local recorder used only by the opt-in traced act path.
#[derive(Default)]
pub(super) struct DecisionTraceRecorder {
    trace: Option<DecisionTrace>,
}

impl DecisionTraceRecorder {
    pub(super) fn begin(&mut self, observation: &Observation) {
        self.trace = Some(DecisionTrace::from_observation(observation));
    }

    pub(super) fn trace_mut(&mut self) -> &mut DecisionTrace {
        self.trace
            .as_mut()
            .expect("a decision trace must begin from an observation")
    }

    pub(super) fn finish(self) -> Option<DecisionTrace> {
        self.trace
    }
}

pub(super) fn channel_effects(
    intent_count: usize,
    claims: &[UnitId],
    committed_scrap: u32,
) -> ChannelEffects {
    let mut unit_claims = claims.to_vec();
    unit_claims.sort_unstable();
    unit_claims.dedup();
    ChannelEffects {
        intents: bounded_count(intent_count),
        unit_claims,
        committed_scrap,
    }
}

pub(super) fn bounded_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::super::strategy::AirOperationPhase;
    use super::*;

    #[test]
    fn channel_effects_canonicalize_and_bound_diagnostic_data() {
        let effects = channel_effects(
            usize::MAX,
            &[UnitId(7), UnitId(2), UnitId(7), UnitId(4)],
            91,
        );

        assert_eq!(effects.intents, u32::MAX);
        assert_eq!(effects.unit_claims, [UnitId(2), UnitId(4), UnitId(7)]);
        assert_eq!(effects.committed_scrap, 91);
    }

    #[test]
    fn package_diagnostics_are_canonicalized_at_the_trace_boundary() {
        let package = ConnectedPackageTrace::from_diagnostics(ConnectedPackageDiagnostics {
            admitted_at: 10,
            derived_at: 20,
            preparation_deadline: 30,
            target_anchors: vec![TilePos::new(9, 8), TilePos::new(12, 7)],
            target_value: 40,
            current_scrap: 50,
            forecast_scrap: 60,
            minimum_capability: [1, 2, 3],
            useful_capability: [4, 5, 6],
            chosen_capability: [7, 8, 9],
            useful_bombing: 10,
            chosen_bombing: 11,
            recon: vec![(UnitKind::Kestrel, usize::MAX), (UnitKind::Kestrel, 1)],
            suppression: vec![(UnitKind::Avalanche, 2), (UnitKind::Bombard, 1)],
            strike: vec![(UnitKind::Buzzard, 3), (UnitKind::Buzzard, 0)],
            observed_aa_firepower: 70,
            suppressible_aa_firepower: 65,
        });

        assert_eq!(
            package.minimum_capability,
            CapabilityTrace {
                recon: 1,
                suppression: 2,
                strike: 3,
            }
        );
        assert_eq!(package.useful_bombing, 10);
        assert_eq!(package.chosen_bombing, 11);
        assert_eq!(
            package.target_anchors,
            [TilePos::new(12, 7), TilePos::new(9, 8)]
        );
        assert_eq!(
            package.demands.recon,
            [ProviderDemandTrace {
                kind: UnitKind::Kestrel,
                count: u32::MAX,
            }]
        );
        assert_eq!(
            package.demands.suppression,
            [
                ProviderDemandTrace {
                    kind: UnitKind::Bombard,
                    count: 1,
                },
                ProviderDemandTrace {
                    kind: UnitKind::Avalanche,
                    count: 2,
                },
            ]
        );
        assert_eq!(
            package.demands.strike,
            [ProviderDemandTrace {
                kind: UnitKind::Buzzard,
                count: 3,
            }]
        );
    }

    #[test]
    fn assigned_force_reports_the_suppression_commitment_boundary() {
        let mut operation = AirOperation {
            target_player: PlayerId(1),
            target_kind: BuildingKind::Foundry,
            target: TilePos::new(9, 8),
            target_id: Some(BuildingId(7)),
            assault_admitted: true,
            phase: AirOperationPhase::Assemble,
            started_at: 10,
            phase_started_at: 20,
            scout: Some(UnitId(3)),
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: vec![UnitId(5), UnitId(4), UnitId(5)],
            strike_aircraft: vec![UnitId(8), UnitId(6), UnitId(8)],
            strike_issued_at: None,
            membership_frozen_at: None,
            recovery_reason: None,
        };

        let revisable = AssignedForceTrace::from_operation(&operation);
        assert!(!revisable.membership_frozen);
        assert_eq!(revisable.suppression, [UnitId(4), UnitId(5)]);
        assert_eq!(revisable.strike, [UnitId(6), UnitId(8)]);

        operation.phase = AirOperationPhase::Recover;
        assert!(
            !AssignedForceTrace::from_operation(&operation).membership_frozen,
            "pre-commit recovery must not fabricate a crossed commitment boundary"
        );

        operation.phase = AirOperationPhase::SuppressAa;
        operation.membership_frozen_at = Some(30);
        let committed = AssignedForceTrace::from_operation(&operation);
        assert!(committed.membership_frozen);
        assert_eq!(committed.scout, revisable.scout);
        assert_eq!(committed.suppression, revisable.suppression);
        assert_eq!(committed.strike, revisable.strike);

        operation.phase = AirOperationPhase::Recover;
        assert!(
            AssignedForceTrace::from_operation(&operation).membership_frozen,
            "recovery after suppression must retain the real commitment history"
        );
    }

    #[test]
    fn rejected_candidate_distinguishes_real_protected_capital_from_nominal_holds() {
        let target = BuildingContact {
            id: Some(BuildingId(7)),
            player: PlayerId(1),
            kind: BuildingKind::Foundry,
            anchor: TilePos::new(9, 8),
            hp: 800,
            built: true,
            tier: 0,
            last_seen: Some(120),
            evidence: ContactEvidence::Current,
        };
        let rejection = ForcePackageRejection::InsufficientResources {
            family: ForceFamily::Strike,
            required_scrap: 100,
            available_scrap: 70,
            deadline_shortfall: 30,
        };

        let protected = RejectedConnectedCandidateTrace::from(&RejectedConnectedCandidate {
            target: target.clone(),
            reason: ConnectedPlanRejection::Package {
                reason: rejection,
                protected_current_scrap: 20,
                protected_forecast_scrap: 10,
            },
        });
        assert_eq!(
            protected.reason,
            ConnectedRejectionReasonTrace::ProtectedFunds {
                family: ForceFamilyTrace::Strike,
                required_scrap: 100,
                available_scrap: 70,
                deadline_shortfall: 30,
                protected_current_scrap: 20,
                protected_forecast_scrap: 10,
            }
        );
        assert_eq!(protected.target.last_live_id, target.id);
        assert_eq!(protected.target.evidence, TargetEvidenceTrace::Current);

        let genuinely_short = package_rejection_trace(rejection, 20, 9);
        assert_eq!(
            genuinely_short,
            ConnectedRejectionReasonTrace::InsufficientSpendableScrap {
                family: ForceFamilyTrace::Strike,
                required_scrap: 100,
                available_scrap: 70,
                deadline_shortfall: 30,
            }
        );

        assert_eq!(
            ConnectedRejectionReasonTrace::from(ConnectedPlanRejection::UnreachableGroupStaging {
                requested: usize::MAX,
            }),
            ConnectedRejectionReasonTrace::UnreachableGroupStaging {
                requested: u32::MAX,
            },
            "trace counts remain bounded on wider hosts"
        );
    }

    #[test]
    fn terminal_trace_preserves_only_a_connected_package() {
        let package = ConnectedPackageTrace {
            admitted_at: 10,
            derived_at: 20,
            preparation_deadline: 30,
            target_anchors: vec![TilePos::new(9, 8), TilePos::new(12, 7)],
            target_value: 40,
            current_scrap: 50,
            forecast_scrap: 60,
            minimum_capability: [1, 2, 3].into(),
            useful_capability: [4, 5, 6].into(),
            chosen_capability: [7, 8, 9].into(),
            useful_bombing: 10,
            chosen_bombing: 11,
            demands: ForceDemandsTrace {
                recon: Vec::new(),
                suppression: Vec::new(),
                strike: Vec::new(),
            },
            observed_aa_firepower: 70,
            suppressible_aa_firepower: 65,
        };
        let previous = ConnectedForceTrace {
            status: ConnectedForceStatus::Active,
            target: Some(ConnectedTargetTrace {
                player: PlayerId(1),
                kind: BuildingKind::Foundry,
                anchor: TilePos::new(9, 8),
                last_live_id: Some(BuildingId(7)),
                evidence: TargetEvidenceTrace::Current,
            }),
            package: Some(package.clone()),
            assigned: AssignedForceTrace {
                membership_frozen: true,
                scout: Some(UnitId(3)),
                suppression: vec![UnitId(4)],
                strike: vec![UnitId(5)],
            },
            rejected_candidate: None,
        };
        let mut terminal = ConnectedForceTrace {
            status: ConnectedForceStatus::Aborted,
            ..ConnectedForceTrace::default()
        };

        terminal.preserve_terminal_package(previous.clone());

        assert_eq!(terminal.status, ConnectedForceStatus::Aborted);
        assert_eq!(terminal.target, previous.target);
        assert_eq!(terminal.package, Some(package));
        assert_eq!(terminal.assigned, previous.assigned);

        let mut island_terminal = ConnectedForceTrace {
            status: ConnectedForceStatus::Released,
            ..ConnectedForceTrace::default()
        };
        island_terminal.preserve_terminal_package(ConnectedForceTrace {
            status: ConnectedForceStatus::OtherAirOperation,
            ..ConnectedForceTrace::default()
        });
        assert_eq!(island_terminal.status, ConnectedForceStatus::Idle);
        assert!(island_terminal.package.is_none());
    }

    #[test]
    fn serialized_trace_has_a_fixed_schema() {
        let mut trace = DecisionTrace::from_observation(&Observation::default());
        trace.gates.opening_core = Some(CoreGateTrace {
            projected_strength: 1,
            target_strength: 2,
            missing_strength: 1,
            missing_scrap: 3,
            ready: false,
        });
        trace.gates.raid_attention = Some(RaidAttentionTrace {
            strategic_load: 1,
            attention_slots: 2,
            admitted: true,
        });
        trace.budget = Some(ScrapBudgetTrace {
            bank: 1,
            foundry_saving: 2,
            deferred_construction: 3,
            airworks_capacity: 4,
            shallow_sentinel: 5,
            opening_bootstrap: 6,
            frozen: false,
            prior_operation_spendable: 7,
            strategic_spendable: 8,
            strategic_committed: 9,
            prospective_carrier: 10,
            utility_spendable: 11,
        });
        trace.channels.connected_air = ChannelTrace {
            before: ChannelState::Preparing,
            after: ChannelState::Active(ChannelPhase::AirStrike),
            effects: ChannelEffects {
                intents: 1,
                unit_claims: vec![UnitId(2)],
                committed_scrap: 3,
            },
        };
        trace.connected_force = ConnectedForceTrace {
            status: ConnectedForceStatus::Recovering(ConnectedRecoveryReasonTrace::NewAirDefense),
            target: Some(ConnectedTargetTrace {
                player: PlayerId(1),
                kind: BuildingKind::Foundry,
                anchor: TilePos::new(12, 7),
                last_live_id: Some(BuildingId(9)),
                evidence: TargetEvidenceTrace::Current,
            }),
            package: Some(ConnectedPackageTrace {
                admitted_at: 100,
                derived_at: 112,
                preparation_deadline: 2_500,
                target_anchors: vec![TilePos::new(12, 7), TilePos::new(15, 7)],
                target_value: 12_000,
                current_scrap: 900,
                forecast_scrap: 300,
                minimum_capability: CapabilityTrace {
                    recon: 1_000,
                    suppression: 1_000,
                    strike: 1_000,
                },
                useful_capability: CapabilityTrace {
                    recon: 1_000,
                    suppression: 3_000,
                    strike: 4_000,
                },
                chosen_capability: CapabilityTrace {
                    recon: 1_000,
                    suppression: 3_100,
                    strike: 4_200,
                },
                useful_bombing: 2_000,
                chosen_bombing: 1_500,
                demands: ForceDemandsTrace {
                    recon: vec![ProviderDemandTrace {
                        kind: UnitKind::Kestrel,
                        count: 1,
                    }],
                    suppression: vec![ProviderDemandTrace {
                        kind: UnitKind::Avalanche,
                        count: 2,
                    }],
                    strike: vec![ProviderDemandTrace {
                        kind: UnitKind::Condor,
                        count: 3,
                    }],
                },
                observed_aa_firepower: 240,
                suppressible_aa_firepower: 180,
            }),
            assigned: AssignedForceTrace {
                membership_frozen: true,
                scout: Some(UnitId(2)),
                suppression: vec![UnitId(3), UnitId(4)],
                strike: vec![UnitId(5), UnitId(6), UnitId(7)],
            },
            rejected_candidate: Some(RejectedConnectedCandidateTrace {
                target: ConnectedTargetTrace {
                    player: PlayerId(1),
                    kind: BuildingKind::Foundry,
                    anchor: TilePos::new(14, 7),
                    last_live_id: Some(BuildingId(10)),
                    evidence: TargetEvidenceTrace::Current,
                },
                reason: ConnectedRejectionReasonTrace::ProtectedFunds {
                    family: ForceFamilyTrace::Strike,
                    required_scrap: 80,
                    available_scrap: 50,
                    deadline_shortfall: 30,
                    protected_current_scrap: 20,
                    protected_forecast_scrap: 10,
                },
            }),
        };
        let Value::Object(trace) = serde_json::to_value(trace).expect("the trace serializes")
        else {
            panic!("a decision trace serializes as an object");
        };

        assert_eq!(trace.get("version"), Some(&Value::from(3)));

        assert_eq!(
            trace.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "budget",
                "channels",
                "connected_force",
                "control_flow",
                "evidence",
                "gates",
                "lowering",
                "player",
                "resources",
                "tick",
                "utility",
                "version",
            ])
        );
        let Some(Value::Object(channels)) = trace.get("channels") else {
            panic!("trace channels serialize as an object");
        };
        assert_eq!(
            channels.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["connected_air", "lift", "raid", "team_relief"])
        );

        let expected_objects = [
            (
                "resources",
                BTreeSet::from([
                    "completed_producers",
                    "current_scrap",
                    "forecast_horizon_ticks",
                    "forecast_scrap",
                    "free_builders",
                    "obligated_builders",
                    "open_producer_slots",
                ]),
            ),
            (
                "evidence",
                BTreeSet::from([
                    "current_enemy_buildings",
                    "current_enemy_units",
                    "radar_blips",
                    "remembered_enemy_buildings",
                ]),
            ),
            (
                "gates",
                BTreeSet::from([
                    "lift_rolled_back",
                    "opening_core",
                    "raid_attention",
                    "team_relief_core_ready",
                    "team_relief_rolled_back",
                ]),
            ),
            (
                "budget",
                BTreeSet::from([
                    "airworks_capacity",
                    "bank",
                    "deferred_construction",
                    "foundry_saving",
                    "frozen",
                    "opening_bootstrap",
                    "prior_operation_spendable",
                    "prospective_carrier",
                    "shallow_sentinel",
                    "strategic_committed",
                    "strategic_spendable",
                    "utility_spendable",
                ]),
            ),
            (
                "utility",
                BTreeSet::from(["input_intents", "output_intents", "reserved_units"]),
            ),
            (
                "lowering",
                BTreeSet::from([
                    "decision_commands",
                    "maintenance_commands",
                    "total_commands",
                ]),
            ),
        ];
        for (field, expected) in expected_objects {
            let Some(Value::Object(object)) = trace.get(field) else {
                panic!("trace field {field} serializes as an object");
            };
            assert_eq!(
                object.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                expected,
                "trace field {field} changed schema"
            );
        }

        let Some(Value::Object(connected_air)) = channels.get("connected_air") else {
            panic!("a channel trace serializes as an object");
        };
        assert_eq!(
            connected_air
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["after", "before", "effects"])
        );
        let Some(Value::Object(effects)) = connected_air.get("effects") else {
            panic!("channel effects serialize as an object");
        };
        assert_eq!(
            effects.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["committed_scrap", "intents", "unit_claims"])
        );

        let Some(Value::Object(connected_force)) = trace.get("connected_force") else {
            panic!("the connected force trace serializes as an object");
        };
        assert_eq!(
            connected_force
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "assigned",
                "package",
                "rejected_candidate",
                "status",
                "target",
            ])
        );
        let Some(Value::Object(package)) = connected_force.get("package") else {
            panic!("the connected package serializes as an object");
        };
        assert_eq!(
            package.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "admitted_at",
                "chosen_bombing",
                "chosen_capability",
                "current_scrap",
                "demands",
                "derived_at",
                "forecast_scrap",
                "minimum_capability",
                "observed_aa_firepower",
                "preparation_deadline",
                "suppressible_aa_firepower",
                "target_anchors",
                "target_value",
                "useful_bombing",
                "useful_capability",
            ])
        );
        assert_eq!(
            package.get("target_anchors"),
            Some(&serde_json::json!([
                { "x": 12, "y": 7 },
                { "x": 15, "y": 7 }
            ]))
        );
        let Some(Value::Object(demands)) = package.get("demands") else {
            panic!("package demands serialize as an object");
        };
        assert_eq!(
            demands.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["recon", "strike", "suppression"])
        );
        for capability in [
            "minimum_capability",
            "useful_capability",
            "chosen_capability",
        ] {
            let Some(Value::Object(capability)) = package.get(capability) else {
                panic!("package capability serializes as an object");
            };
            assert_eq!(
                capability
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["recon", "strike", "suppression"])
            );
        }

        let Some(Value::Object(assigned)) = connected_force.get("assigned") else {
            panic!("the assigned force serializes as an object");
        };
        assert_eq!(
            assigned.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["membership_frozen", "scout", "strike", "suppression"])
        );
        let Some(Value::Object(rejected)) = connected_force.get("rejected_candidate") else {
            panic!("the rejected candidate serializes as an object");
        };
        assert_eq!(
            rejected.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["reason", "target"])
        );
        let Some(Value::Object(reason)) = rejected.get("reason") else {
            panic!("the rejection reason serializes as an object");
        };
        assert_eq!(
            reason.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "available_scrap",
                "deadline_shortfall",
                "family",
                "protected_current_scrap",
                "protected_forecast_scrap",
                "reason",
                "required_scrap",
            ])
        );

        let Some(Value::Object(gates)) = trace.get("gates") else {
            unreachable!("gates were asserted above");
        };
        let Some(Value::Object(opening_core)) = gates.get("opening_core") else {
            panic!("opening core serializes as an object");
        };
        assert_eq!(
            opening_core
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "missing_scrap",
                "missing_strength",
                "projected_strength",
                "ready",
                "target_strength",
            ])
        );
        let Some(Value::Object(raid_attention)) = gates.get("raid_attention") else {
            panic!("raid attention serializes as an object");
        };
        assert_eq!(
            raid_attention
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["admitted", "attention_slots", "strategic_load"])
        );
    }

    #[test]
    fn resource_trace_shortens_an_unrepresentable_horizon() {
        let observation = Observation {
            tick: Tick::MAX,
            scrap: 17,
            ..Observation::default()
        };

        let trace = DecisionTrace::from_observation(&observation);

        assert_eq!(trace.resources.current_scrap, 17);
        assert_eq!(trace.resources.forecast_horizon_ticks, 1);
        assert_eq!(trace.resources.forecast_scrap, 0);
    }
}
