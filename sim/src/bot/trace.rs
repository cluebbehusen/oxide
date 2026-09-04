//! Opt-in, fog-honest diagnostics for one player-facing bot decision.
//!
//! Traces are observational output. They are not controller memory, simulation
//! state, events, or replay input, and nothing in the policy may read them back.

use super::allocation::{
    AllocationConflict, AllocationError, AllocationResult, CapitalFundingAssignment, ClaimBundle,
    ClaimBundleError, ClaimOwner, Confidence, ConnectedOffenseKey, CoordinatorInputError,
    ExecutionSafety, ImportedObligation, InvestmentProposal, LegacyChannel, ObligationClass,
    ObligationKey, OutrankingBasis, ProducerJobClaim, ProposalCase, ProposalDecision,
    ProposalDisposition, ProposalKey, ProposalRejection, ScheduledProducerJob, StrategicValue,
    TimeToImpact, Urgency,
};
use super::observation::Observation;
use super::resources::{
    PlanningProjectionError, ProducerLaneReservationError, ResourceSnapshot, SiteFootprint,
};
use super::strategy::force_package::{ForceFamily, ForcePackageRejection};
use super::strategy::{
    AirOperation, AirOperationOutcome, AirRecoveryReason, ConnectedPackageDiagnostics,
    ConnectedPlanRejection, ConnectedProducerBindingError, ConnectedProposalCommitError,
    RejectedConnectedCandidate, StrategicPlanner,
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
pub const DECISION_TRACE_VERSION: u32 = 5;

const RESOURCE_FORECAST_TICKS: Tick = crate::TICKS_PER_SECOND as Tick * 60;
const ALLOCATION_TRACE_ENTRY_LIMIT: usize = 32;

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
    /// Cross-domain obligations, proposals, dispositions, and exact lane plan.
    pub allocation: AllocationTrace,
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
            allocation: AllocationTrace::default(),
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

/// Fixed, bounded record of cross-domain allocation on one decision tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct AllocationTrace {
    /// Prior accepted work imported before fresh portfolio selection.
    pub obligations: BoundedTraceEntries<AllocationObligationTrace>,
    /// Fresh domain proposals in canonical structural order.
    pub proposals: BoundedTraceEntries<AllocationProposalTrace>,
    /// Final exact producer assignments, including imported obligations.
    pub producer_schedule: BoundedTraceEntries<ScheduledProducerJobTrace>,
    /// Final bank-versus-income splits for flexible non-production capital.
    pub capital_assignments: BoundedTraceEntries<CapitalFundingAssignmentTrace>,
    /// Optional allocator input failure. Valid production decisions leave this absent.
    pub error: Option<AllocationErrorTrace>,
    /// Failure outside portfolio selection that prevented an exact coordinated commit.
    pub coordinator_failure: Option<AllocationCoordinatorFailureTrace>,
    /// Optional connected-offense scale step considered after its minimum won.
    pub connected_marginal: Option<ConnectedMarginalTrace>,
}

impl AllocationTrace {
    /// Captures allocator inputs before their exact domain payloads move into selection.
    pub(super) fn from_inputs<FoundryPayload, OffensePayload>(
        obligations: &[ImportedObligation],
        proposals: &[InvestmentProposal<FoundryPayload, OffensePayload>],
    ) -> Self {
        let mut obligations = obligations.iter().collect::<Vec<_>>();
        obligations.sort_unstable_by_key(|obligation| obligation.owner());
        let obligations = obligations
            .into_iter()
            .map(AllocationObligationTrace::from)
            .collect();

        let mut proposals = proposals.iter().collect::<Vec<_>>();
        proposals.sort_unstable_by_key(|proposal| proposal.key());
        let proposals = proposals
            .into_iter()
            .map(AllocationProposalTrace::from)
            .collect();

        Self {
            obligations: BoundedTraceEntries::from_vec(obligations),
            proposals: BoundedTraceEntries::from_vec(proposals),
            ..Self::default()
        }
    }

    /// Applies final proposal dispositions and the selected lane schedule.
    pub(super) fn record_result<FoundryPayload, OffensePayload>(
        &mut self,
        result: &AllocationResult<FoundryPayload, OffensePayload>,
    ) {
        self.record_decisions(&result.decisions);
        for accepted in &result.accepted {
            if let Some(proposal) = self
                .proposals
                .entries
                .iter_mut()
                .find(|proposal| proposal.key == accepted.key().into())
            {
                proposal.claims = accepted.claims().into();
            }
        }
        self.error = None;
        self.record_producer_schedule(&result.producer_schedule);
        self.capital_assignments = BoundedTraceEntries::from_vec(
            result
                .capital_assignments
                .iter()
                .copied()
                .map(CapitalFundingAssignmentTrace::from)
                .collect(),
        );
    }

    fn record_decisions(&mut self, decisions: &[ProposalDecision]) {
        for decision in decisions {
            if let Some(proposal) = self
                .proposals
                .entries
                .iter_mut()
                .find(|proposal| proposal.key == decision.key.into())
            {
                proposal.case = decision.case.into();
                proposal.personality_weight = Some(decision.personality_weight);
                proposal.disposition = decision.disposition.clone().into();
            }
        }
    }

    /// Records malformed or mutually inconsistent allocator inputs without changing policy.
    pub(super) fn record_error(&mut self, error: &AllocationError) {
        self.error = Some(error.clone().into());
    }

    /// Records a typed integration failure instead of collapsing it into a frozen budget.
    pub(super) fn record_coordinator_failure(
        &mut self,
        stage: AllocationCoordinatorStageTrace,
        reason: AllocationCoordinatorFailureReasonTrace,
    ) {
        self.coordinator_failure = Some(AllocationCoordinatorFailureTrace { stage, reason });
    }

    /// Records an accepted connected scale step and the resulting final lane schedule.
    pub(super) fn record_connected_marginal_accepted(
        &mut self,
        key: ConnectedOffenseKey,
        claims: &ClaimBundle,
        schedule: &[ScheduledProducerJob],
    ) {
        self.connected_marginal = Some(ConnectedMarginalTrace {
            key: key.into(),
            claims: claims.into(),
            disposition: ConnectedMarginalDispositionTrace::Accepted,
        });
        self.record_producer_schedule(schedule);
    }

    /// Records the exact claim that prevented a connected scale step.
    pub(super) fn record_connected_marginal_rejected(
        &mut self,
        key: ConnectedOffenseKey,
        claims: &ClaimBundle,
        conflict: &AllocationConflict,
    ) {
        self.connected_marginal = Some(ConnectedMarginalTrace {
            key: key.into(),
            claims: claims.into(),
            disposition: ConnectedMarginalDispositionTrace::Rejected {
                conflict: conflict.into(),
            },
        });
    }

    fn record_producer_schedule(&mut self, schedule: &[ScheduledProducerJob]) {
        self.producer_schedule = BoundedTraceEntries::from_vec(
            schedule
                .iter()
                .copied()
                .map(ScheduledProducerJobTrace::from)
                .collect(),
        );
    }
}

/// A canonical prefix plus exact accounting for entries omitted by a trace bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundedTraceEntries<T> {
    /// Total entries before applying the diagnostic bound.
    pub total: u32,
    /// Canonical entries retained in the trace.
    pub entries: Vec<T>,
    /// Entries omitted after the canonical prefix.
    pub omitted: u32,
}

impl<T> Default for BoundedTraceEntries<T> {
    fn default() -> Self {
        Self {
            total: 0,
            entries: Vec::new(),
            omitted: 0,
        }
    }
}

impl<T> BoundedTraceEntries<T> {
    fn from_vec(mut entries: Vec<T>) -> Self {
        let total = bounded_count(entries.len());
        entries.truncate(ALLOCATION_TRACE_ENTRY_LIMIT);
        let omitted = total.saturating_sub(bounded_count(entries.len()));
        Self {
            total,
            entries,
            omitted,
        }
    }
}

/// One imported allocator obligation with its exact remaining claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AllocationObligationTrace {
    /// Priority class assigned to this accepted work.
    pub class: ObligationClassTrace,
    /// Tick on which the work originally gained ownership.
    pub accepted_at: Tick,
    /// Stable typed identity of the work.
    pub key: ObligationKeyTrace,
    /// Exact bounded shared-resource claims still owned.
    pub claims: AllocationClaimsTrace,
}

impl From<&ImportedObligation> for AllocationObligationTrace {
    fn from(obligation: &ImportedObligation) -> Self {
        Self {
            class: obligation.class.into(),
            accepted_at: obligation.accepted_at,
            key: obligation.key.into(),
            claims: (&obligation.claims).into(),
        }
    }
}

/// One fresh proposal and the allocator's disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AllocationProposalTrace {
    /// Stable proposal identity.
    pub key: ProposalKeyTrace,
    /// Five named comparison bands supplied by the owning domain.
    pub case: ProposalCaseTrace,
    /// Positive personality weight, absent only if allocation rejected its inputs.
    pub personality_weight: Option<u128>,
    /// Exact bounded shared-resource claims required by the proposal.
    pub claims: AllocationClaimsTrace,
    /// Final selection result.
    pub disposition: ProposalDispositionTrace,
}

impl<FoundryPayload, OffensePayload> From<&InvestmentProposal<FoundryPayload, OffensePayload>>
    for AllocationProposalTrace
{
    fn from(proposal: &InvestmentProposal<FoundryPayload, OffensePayload>) -> Self {
        Self {
            key: proposal.key().into(),
            case: proposal.case().into(),
            personality_weight: None,
            claims: proposal.claims().into(),
            disposition: ProposalDispositionTrace::NotEvaluated,
        }
    }
}

/// Stable public diagnostic identity for one fresh proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "domain", rename_all = "snake_case")]
pub enum ProposalKeyTrace {
    /// A fresh Foundry expansion at an exact anchor.
    FoundryExpansion {
        /// Proposed top-left Foundry tile.
        anchor: TilePos,
    },
    /// The minimum viable connected air-and-siege operation.
    ConnectedOffenseMinimum {
        /// Current primary target id.
        objective: BuildingId,
        /// Current primary target footprint anchor.
        anchor: TilePos,
    },
}

impl From<ProposalKey> for ProposalKeyTrace {
    fn from(key: ProposalKey) -> Self {
        match key {
            ProposalKey::FoundryExpansion(key) => Self::FoundryExpansion { anchor: key.anchor },
            ProposalKey::ConnectedOffenseMinimum(key) => Self::ConnectedOffenseMinimum {
                objective: key.objective,
                anchor: key.anchor,
            },
        }
    }
}

impl From<ConnectedOffenseKey> for ProposalKeyTrace {
    fn from(key: ConnectedOffenseKey) -> Self {
        ProposalKey::ConnectedOffenseMinimum(key).into()
    }
}

/// Domain-independent comparison bands for one proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProposalCaseTrace {
    /// How quickly the situation calls for action.
    pub urgency: UrgencyTrace,
    /// Strength of the fog-honest supporting evidence.
    pub confidence: ConfidenceTrace,
    /// Consequence of successful execution.
    pub value: StrategicValueTrace,
    /// Delay before the investment can affect the match.
    pub time_to_impact: TimeToImpactTrace,
    /// Confidence that the investment can be executed safely.
    pub safety: ExecutionSafetyTrace,
}

impl From<ProposalCase> for ProposalCaseTrace {
    fn from(case: ProposalCase) -> Self {
        Self {
            urgency: case.urgency.into(),
            confidence: case.confidence.into(),
            value: case.value.into(),
            time_to_impact: case.time_to_impact.into(),
            safety: case.safety.into(),
        }
    }
}

/// Proposal urgency band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UrgencyTrace {
    /// Useful long-term development.
    Developmental,
    /// A current opportunity that should not drift.
    Timely,
    /// Immediate pressure or a perishable opportunity.
    Pressing,
}

impl From<Urgency> for UrgencyTrace {
    fn from(value: Urgency) -> Self {
        match value {
            Urgency::Developmental => Self::Developmental,
            Urgency::Timely => Self::Timely,
            Urgency::Pressing => Self::Pressing,
        }
    }
}

/// Proposal evidence-confidence band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceTrace {
    /// Public priors or remembered evidence only.
    Prior,
    /// Multiple observations support the case.
    Supported,
    /// Direct current evidence supports the case.
    Current,
}

impl From<Confidence> for ConfidenceTrace {
    fn from(value: Confidence) -> Self {
        match value {
            Confidence::Prior => Self::Prior,
            Confidence::Supported => Self::Supported,
            Confidence::Current => Self::Current,
        }
    }
}

/// Proposal strategic-value band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategicValueTrace {
    /// Improves the position without changing its shape.
    Incremental,
    /// Creates or protects a meaningful advantage.
    Material,
    /// Can decide the current strategic contest.
    Decisive,
}

impl From<StrategicValue> for StrategicValueTrace {
    fn from(value: StrategicValue) -> Self {
        match value {
            StrategicValue::Incremental => Self::Incremental,
            StrategicValue::Material => Self::Material,
            StrategicValue::Decisive => Self::Decisive,
        }
    }
}

/// Proposal time-to-impact band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeToImpactTrace {
    /// Pays off beyond the immediate tactical window.
    Patient,
    /// Can affect the next planned contest.
    Near,
    /// Can affect the current contest.
    Immediate,
}

impl From<TimeToImpact> for TimeToImpactTrace {
    fn from(value: TimeToImpact) -> Self {
        match value {
            TimeToImpact::Patient => Self::Patient,
            TimeToImpact::Near => Self::Near,
            TimeToImpact::Immediate => Self::Immediate,
        }
    }
}

/// Proposal execution-safety band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSafetyTrace {
    /// Important execution risks remain unresolved.
    Speculative,
    /// Known risks have a credible mitigation.
    Managed,
    /// Current evidence supports protected execution.
    Secure,
}

impl From<ExecutionSafety> for ExecutionSafetyTrace {
    fn from(value: ExecutionSafety) -> Self {
        match value {
            ExecutionSafety::Speculative => Self::Speculative,
            ExecutionSafety::Managed => Self::Managed,
            ExecutionSafety::Secure => Self::Secure,
        }
    }
}

/// Exact shared-resource requirements summarized at a fixed diagnostic bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct AllocationClaimsTrace {
    /// Non-production capital required from the current bank.
    pub current_scrap: u32,
    /// Total non-production capital reserved from future completed-source income.
    pub forecast_scrap_total: u128,
    /// Flexible non-production capital awaiting an allocator-owned split.
    pub deferrable_capital: Option<ForecastClaimTrace>,
    /// Total future production cost represented by the job requests.
    pub producer_job_scrap_total: u128,
    /// Current, forecast, and production capital claimed exactly once.
    pub claimed_capital: u128,
    /// Future non-production capital claims by fixed deadline.
    pub forecast_scrap: BoundedTraceEntries<ForecastClaimTrace>,
    /// Exact construction-capable units.
    pub builders: BoundedTraceEntries<UnitId>,
    /// Exact non-builder force members.
    pub units: BoundedTraceEntries<UnitId>,
    /// Exact construction footprints.
    pub sites: BoundedTraceEntries<SiteFootprintTrace>,
    /// Ordered future production requests.
    pub producer_jobs: BoundedTraceEntries<ProducerJobClaimTrace>,
}

impl From<&ClaimBundle> for AllocationClaimsTrace {
    fn from(claims: &ClaimBundle) -> Self {
        let forecast_scrap_total = claims
            .forecast_scrap()
            .iter()
            .map(|claim| u128::from(claim.amount))
            .sum::<u128>();
        let producer_job_scrap_total = claims
            .producer_jobs()
            .iter()
            .map(|job| u128::from(job.kind().stats().cost))
            .sum::<u128>();
        Self {
            current_scrap: claims.current_scrap(),
            forecast_scrap_total,
            deferrable_capital: claims.deferrable_capital().map(|claim| ForecastClaimTrace {
                through: claim.through,
                amount: claim.amount,
            }),
            producer_job_scrap_total,
            claimed_capital: claims.claimed_capital(),
            forecast_scrap: BoundedTraceEntries::from_vec(
                claims
                    .forecast_scrap()
                    .iter()
                    .map(|claim| ForecastClaimTrace {
                        through: claim.through,
                        amount: claim.amount,
                    })
                    .collect(),
            ),
            builders: BoundedTraceEntries::from_vec(claims.builders().to_vec()),
            units: BoundedTraceEntries::from_vec(claims.units().to_vec()),
            sites: BoundedTraceEntries::from_vec(
                claims
                    .sites()
                    .iter()
                    .copied()
                    .map(SiteFootprintTrace::from)
                    .collect(),
            ),
            producer_jobs: BoundedTraceEntries::from_vec(
                claims
                    .producer_jobs()
                    .iter()
                    .map(ProducerJobClaimTrace::from)
                    .collect(),
            ),
        }
    }
}

/// One future non-production capital claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ForecastClaimTrace {
    /// Fixed deadline through which income may be used.
    pub through: Tick,
    /// Capital reserved from completed-source income.
    pub amount: u32,
}

/// One exact rectangular construction claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SiteFootprintTrace {
    /// Top-left tile.
    pub anchor: TilePos,
    /// Width in tiles.
    pub width: i32,
    /// Height in tiles.
    pub height: i32,
}

impl From<SiteFootprint> for SiteFootprintTrace {
    fn from(site: SiteFootprint) -> Self {
        let (width, height) = site.size();
        Self {
            anchor: site.anchor(),
            width,
            height,
        }
    }
}

/// One fixed-horizon future production request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProducerJobClaimTrace {
    /// Requested unit kind.
    pub kind: UnitKind,
    /// Exact ordinary scrap cost of the requested unit.
    pub cost: u32,
    /// First decision tick on which the request may enter a queue.
    pub enqueue_not_before: Tick,
    /// Observation deadline, strictly after readiness.
    pub ready_before: Tick,
    /// Fresh flexible lanes or the exact retained obligation lane.
    pub access: ProducerJobAccessTrace,
}

impl From<&ProducerJobClaim> for ProducerJobClaimTrace {
    fn from(job: &ProducerJobClaim) -> Self {
        let access = if let Some((producer, enqueued_at, starts_at, ready_at)) = job.fixed_timing()
        {
            ProducerJobAccessTrace::Fixed {
                producer,
                enqueued_at,
                starts_at,
                ready_at,
            }
        } else {
            ProducerJobAccessTrace::Flexible {
                eligible_producers: BoundedTraceEntries::from_vec(
                    job.eligible_producers().to_vec(),
                ),
            }
        };
        Self {
            kind: job.kind(),
            cost: job.kind().stats().cost,
            enqueue_not_before: job.enqueue_not_before(),
            ready_before: job.ready_before(),
            access,
        }
    }
}

/// Producer access retained by one production request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ProducerJobAccessTrace {
    /// Allocation may choose among these preflighted completed producers.
    Flexible {
        /// Canonical eligible producer ids.
        eligible_producers: BoundedTraceEntries<BuildingId>,
    },
    /// A persistent typed plan retains one exact lane and FIFO schedule.
    Fixed {
        /// Exact completed producer.
        producer: BuildingId,
        /// Exact command boundary retained by the plan.
        enqueued_at: Tick,
        /// Exact FIFO production start.
        starts_at: Tick,
        /// Exact production completion tick.
        ready_at: Tick,
    },
}

/// Priority class of imported accepted work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationClassTrace {
    /// Immediate survival or protected ordinary-core work.
    Survival,
    /// Already-paid construction or production.
    PaidWork,
    /// A previously accepted domain plan.
    PersistentPlan,
    /// An explicit adapter for an unmigrated channel.
    Legacy,
}

impl From<ObligationClass> for ObligationClassTrace {
    fn from(value: ObligationClass) -> Self {
        match value {
            ObligationClass::Survival => Self::Survival,
            ObligationClass::PaidWork => Self::PaidWork,
            ObligationClass::PersistentPlan => Self::PersistentPlan,
            ObligationClass::Legacy => Self::Legacy,
        }
    }
}

/// Stable typed identity of one imported obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObligationKeyTrace {
    /// One exact opening defense admitted before ordinary core recovery.
    EmergencyDefense {
        /// Defensive structure selected by the utility scorer.
        building: BuildingKind,
        /// Exact scorer-selected anchor.
        anchor: TilePos,
    },
    /// One protected opening- or recovery-core tranche.
    OpeningCore {
        /// Stable tranche sequence.
        sequence: u16,
    },
    /// One already-paid construction site.
    PaidConstruction {
        /// Exact paid building.
        building: BuildingId,
    },
    /// One observed builder occupation without a construction footprint.
    ObservedBuilderWork {
        /// Exact occupied builder.
        builder: UnitId,
    },
    /// One accepted deferred foundation.
    DeferredFoundation {
        /// Exact builder carrying the promise.
        builder: UnitId,
        /// Exact promised anchor.
        anchor: TilePos,
    },
    /// One accepted but not yet dispatched Foundry plan.
    SavedFoundry {
        /// Exact promised anchor.
        anchor: TilePos,
    },
    /// One already-active connected operation.
    ConnectedOffense {
        /// Exact primary objective at admission.
        objective: BuildingId,
        /// Exact objective anchor at admission.
        anchor: TilePos,
    },
    /// One explicit not-yet-migrated owner.
    Legacy {
        /// Strategic channel behind the adapter.
        channel: LegacyChannelTrace,
        /// Stable channel-local identity.
        sequence: u32,
    },
}

impl From<ObligationKey> for ObligationKeyTrace {
    fn from(value: ObligationKey) -> Self {
        match value {
            ObligationKey::EmergencyDefense { kind, anchor } => Self::EmergencyDefense {
                building: kind,
                anchor,
            },
            ObligationKey::OpeningCore { sequence } => Self::OpeningCore { sequence },
            ObligationKey::PaidConstruction(building) => Self::PaidConstruction { building },
            ObligationKey::ObservedBuilderWork { builder } => Self::ObservedBuilderWork { builder },
            ObligationKey::DeferredFoundation { builder, anchor } => {
                Self::DeferredFoundation { builder, anchor }
            }
            ObligationKey::SavedFoundry { anchor } => Self::SavedFoundry { anchor },
            ObligationKey::ConnectedOffense { objective, anchor } => {
                Self::ConnectedOffense { objective, anchor }
            }
            ObligationKey::Legacy { channel, sequence } => Self::Legacy {
                channel: channel.into(),
                sequence,
            },
        }
    }
}

/// Explicit unmigrated channel behind a legacy obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyChannelTrace {
    /// Units already enlisted by the Executive's standing army.
    StandingArmy,
    /// Allied-base relief.
    TeamRelief,
    /// Severed-ground transport.
    Lift,
    /// Harassment or resource raid.
    Raid,
    /// Already-admitted air operation without a connected force package.
    StrategicAir,
    /// Operation-driven Airworks construction.
    AirworksCapacity,
}

impl From<LegacyChannel> for LegacyChannelTrace {
    fn from(value: LegacyChannel) -> Self {
        match value {
            LegacyChannel::StandingArmy => Self::StandingArmy,
            LegacyChannel::TeamRelief => Self::TeamRelief,
            LegacyChannel::Lift => Self::Lift,
            LegacyChannel::Raid => Self::Raid,
            LegacyChannel::StrategicAir => Self::StrategicAir,
            LegacyChannel::AirworksCapacity => Self::AirworksCapacity,
        }
    }
}

/// Stable owner identity used by conflicts and producer assignments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum ClaimOwnerTrace {
    /// Mandatory work accepted before this allocation pass.
    Obligation {
        /// Priority class of the obligation.
        class: ObligationClassTrace,
        /// Original acceptance tick.
        accepted_at: Tick,
        /// Stable typed identity.
        key: ObligationKeyTrace,
    },
    /// Fresh selectable work.
    Proposal {
        /// Stable proposal identity.
        key: ProposalKeyTrace,
    },
}

impl From<ClaimOwner> for ClaimOwnerTrace {
    fn from(value: ClaimOwner) -> Self {
        match value {
            ClaimOwner::Obligation {
                class,
                accepted_at,
                key,
            } => Self::Obligation {
                class: class.into(),
                accepted_at,
                key: key.into(),
            },
            ClaimOwner::Proposal(key) => Self::Proposal { key: key.into() },
        }
    }
}

/// Final disposition of one fresh allocation proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProposalDispositionTrace {
    /// Allocation did not run because its input set was invalid.
    NotEvaluated,
    /// The exact proposal won selection.
    Accepted,
    /// The proposal could not fit even without the competing fresh domain.
    Infeasible {
        /// First exact failed claim.
        conflict: AllocationConflictTrace,
    },
    /// A selected proposal owns a conflicting shared resource.
    ConflictsWithSelected {
        /// Exact selected proposal identities.
        selected: BoundedTraceEntries<ProposalKeyTrace>,
        /// First exact failed claim against the selected portfolio.
        conflict: AllocationConflictTrace,
    },
    /// The proposal fit but lost the documented semantic and personality rank.
    Outranked {
        /// Exact proposal identities in the winning portfolio.
        selected: BoundedTraceEntries<ProposalKeyTrace>,
        /// First rank component that favored the winning portfolio.
        basis: OutrankingBasisTrace,
    },
}

impl From<ProposalDisposition> for ProposalDispositionTrace {
    fn from(value: ProposalDisposition) -> Self {
        match value {
            ProposalDisposition::Accepted => Self::Accepted,
            ProposalDisposition::Rejected(ProposalRejection::Infeasible(conflict)) => {
                Self::Infeasible {
                    conflict: conflict.into(),
                }
            }
            ProposalDisposition::Rejected(ProposalRejection::ConflictsWithSelected {
                selected,
                conflict,
            }) => Self::ConflictsWithSelected {
                selected: proposal_keys_trace(selected),
                conflict: conflict.into(),
            },
            ProposalDisposition::Rejected(ProposalRejection::Outranked { selected, basis }) => {
                Self::Outranked {
                    selected: proposal_keys_trace(selected),
                    basis: basis.into(),
                }
            }
        }
    }
}

/// First deterministic rank component that favored the selected portfolio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutrankingBasisTrace {
    /// The selected portfolio had the stronger urgency histogram.
    Urgency,
    /// The selected portfolio had the stronger evidence-confidence histogram.
    Confidence,
    /// The selected portfolio had the stronger strategic-value histogram.
    StrategicValue,
    /// The selected portfolio could affect the match sooner.
    TimeToImpact,
    /// The selected portfolio had the stronger execution-safety histogram.
    Safety,
    /// Positive personality emphasis broke a semantic tie.
    Personality,
    /// Lower claimed capital broke every higher-order tie.
    LowerCapital,
    /// Canonical structural identity broke the final tie.
    StructuralKey,
}

impl From<OutrankingBasis> for OutrankingBasisTrace {
    fn from(value: OutrankingBasis) -> Self {
        match value {
            OutrankingBasis::Urgency => Self::Urgency,
            OutrankingBasis::Confidence => Self::Confidence,
            OutrankingBasis::StrategicValue => Self::StrategicValue,
            OutrankingBasis::TimeToImpact => Self::TimeToImpact,
            OutrankingBasis::Safety => Self::Safety,
            OutrankingBasis::Personality => Self::Personality,
            OutrankingBasis::LowerCapital => Self::LowerCapital,
            OutrankingBasis::StructuralKey => Self::StructuralKey,
        }
    }
}

fn proposal_keys_trace(mut keys: Vec<ProposalKey>) -> BoundedTraceEntries<ProposalKeyTrace> {
    keys.sort_unstable();
    keys.dedup();
    BoundedTraceEntries::from_vec(keys.into_iter().map(ProposalKeyTrace::from).collect())
}

/// One exact allocator conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum AllocationConflictTrace {
    /// A marginal claim names an unselected connected proposal.
    InactiveProposal {
        /// Missing selected proposal.
        proposal: ProposalKeyTrace,
    },
    /// Current-bank claims exceed observed capacity.
    CurrentScrap {
        /// Total requested capital.
        requested: u64,
        /// Observed current-bank capacity.
        available: u32,
    },
    /// Forecast claims exceed completed-source income by a deadline.
    ForecastScrap {
        /// First failing deadline.
        through: Tick,
        /// Cumulative requested future capital.
        requested: u64,
        /// Cumulative completed-source income.
        available: u64,
    },
    /// A forecast claim extends beyond the bounded projection.
    ForecastHorizon {
        /// Requested deadline.
        through: Tick,
        /// Last projected tick.
        horizon: Tick,
    },
    /// A force member is absent from the current own-unit roster.
    UnknownUnit {
        /// Missing unit.
        unit: UnitId,
    },
    /// A builder is not currently available.
    UnknownBuilder {
        /// Missing builder.
        unit: UnitId,
    },
    /// Two owners claim the same actor.
    Actor {
        /// Contested actor.
        unit: UnitId,
        /// Owner that already holds it.
        existing: ClaimOwnerTrace,
    },
    /// Two owners claim overlapping construction footprints.
    Site {
        /// Requested footprint.
        requested: SiteFootprintTrace,
        /// Existing overlapping footprint.
        existing: SiteFootprintTrace,
        /// Owner of the existing footprint.
        owner: ClaimOwnerTrace,
    },
    /// A requested producer is absent from completed capacity.
    UnknownProducer {
        /// Missing producer.
        producer: BuildingId,
    },
    /// No named producer can train the requested kind.
    ProducerAccess {
        /// Requested unit kind.
        kind: UnitKind,
        /// Canonical producer set supplied by the domain.
        eligible_producers: BoundedTraceEntries<BuildingId>,
    },
    /// No lane ordering can preserve every claim and deadline.
    ProducerSchedule {
        /// Producers involved in the failed schedule.
        producers: BoundedTraceEntries<BuildingId>,
        /// Owners involved in the failed schedule.
        owners: BoundedTraceEntries<ClaimOwnerTrace>,
    },
    /// Production costs cannot be paid by their latest legal start ticks.
    ProductionFunding {
        /// First failing payment tick.
        through: Tick,
        /// Capital and production cost required through that tick.
        requested: u128,
        /// Current bank and completed-source income available through that tick.
        available: u128,
    },
}

impl From<AllocationConflict> for AllocationConflictTrace {
    fn from(value: AllocationConflict) -> Self {
        (&value).into()
    }
}

impl From<&AllocationConflict> for AllocationConflictTrace {
    fn from(value: &AllocationConflict) -> Self {
        match value {
            AllocationConflict::InactiveProposal(proposal) => Self::InactiveProposal {
                proposal: (*proposal).into(),
            },
            AllocationConflict::CurrentScrap {
                requested,
                available,
            } => Self::CurrentScrap {
                requested: *requested,
                available: *available,
            },
            AllocationConflict::ForecastScrap {
                through,
                requested,
                available,
            } => Self::ForecastScrap {
                through: *through,
                requested: *requested,
                available: *available,
            },
            AllocationConflict::ForecastHorizon { through, horizon } => Self::ForecastHorizon {
                through: *through,
                horizon: *horizon,
            },
            AllocationConflict::UnknownUnit(unit) => Self::UnknownUnit { unit: *unit },
            AllocationConflict::UnknownBuilder(unit) => Self::UnknownBuilder { unit: *unit },
            AllocationConflict::Actor { unit, existing } => Self::Actor {
                unit: *unit,
                existing: (*existing).into(),
            },
            AllocationConflict::Site {
                requested,
                existing,
                owner,
            } => Self::Site {
                requested: (*requested).into(),
                existing: (*existing).into(),
                owner: (*owner).into(),
            },
            AllocationConflict::UnknownProducer(producer) => Self::UnknownProducer {
                producer: *producer,
            },
            AllocationConflict::ProducerAccess {
                kind,
                eligible_producers,
            } => Self::ProducerAccess {
                kind: *kind,
                eligible_producers: BoundedTraceEntries::from_vec(eligible_producers.clone()),
            },
            AllocationConflict::ProducerSchedule { producers, owners } => Self::ProducerSchedule {
                producers: BoundedTraceEntries::from_vec(producers.clone()),
                owners: BoundedTraceEntries::from_vec(
                    owners.iter().copied().map(ClaimOwnerTrace::from).collect(),
                ),
            },
            AllocationConflict::ProductionFunding {
                through,
                requested,
                available,
            } => Self::ProductionFunding {
                through: *through,
                requested: *requested,
                available: *available,
            },
        }
    }
}

/// Invalid allocator input that prevented portfolio selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum AllocationErrorTrace {
    /// More than the two currently migrated domains submitted proposals.
    TooManyProposals {
        /// Submitted proposal count.
        count: u32,
    },
    /// One structural proposal identity was repeated.
    DuplicateProposalKey {
        /// Repeated identity.
        key: ProposalKeyTrace,
    },
    /// One migrated domain submitted more than one proposal.
    DuplicateProposalDomain {
        /// Second identity from that domain.
        key: ProposalKeyTrace,
    },
    /// One imported obligation identity was repeated.
    DuplicateObligation {
        /// Repeated owner identity.
        owner: ClaimOwnerTrace,
    },
    /// Mandatory accepted work did not fit the current resource basis.
    ObligationConflict {
        /// Exact accepted owner that failed import.
        obligation: ClaimOwnerTrace,
        /// First failed claim.
        conflict: AllocationConflictTrace,
    },
    /// The selected schedule named a producer absent from its resource basis.
    ProducerReservationUnknownProducer {
        /// Exact missing producer.
        producer: BuildingId,
    },
    /// The selected current-tick append could no longer be replayed.
    ProducerReservationAppendUnavailable {
        /// Exact producer.
        producer: BuildingId,
        /// Unit whose append failed.
        kind: UnitKind,
    },
    /// Replaying a selected current-tick append changed its FIFO timing.
    ProducerReservationTimingMismatch {
        /// Exact producer.
        producer: BuildingId,
        /// Unit whose timing changed.
        kind: UnitKind,
    },
}

impl From<AllocationError> for AllocationErrorTrace {
    fn from(value: AllocationError) -> Self {
        match value {
            AllocationError::TooManyProposals(count) => Self::TooManyProposals {
                count: bounded_count(count),
            },
            AllocationError::DuplicateProposalKey(key) => {
                Self::DuplicateProposalKey { key: key.into() }
            }
            AllocationError::DuplicateProposalDomain(key) => {
                Self::DuplicateProposalDomain { key: key.into() }
            }
            AllocationError::DuplicateObligation(owner) => Self::DuplicateObligation {
                owner: owner.into(),
            },
            AllocationError::ObligationConflict {
                obligation,
                conflict,
            } => Self::ObligationConflict {
                obligation: obligation.into(),
                conflict: conflict.into(),
            },
            AllocationError::ProducerReservation(error) => match error {
                ProducerLaneReservationError::UnknownProducer { producer } => {
                    Self::ProducerReservationUnknownProducer { producer }
                }
                ProducerLaneReservationError::CurrentAppendUnavailable { producer, kind } => {
                    Self::ProducerReservationAppendUnavailable { producer, kind }
                }
                ProducerLaneReservationError::CurrentTimingMismatch { producer, kind } => {
                    Self::ProducerReservationTimingMismatch { producer, kind }
                }
            },
        }
    }
}

/// Coordinator phase that failed before an exact allocation could be committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationCoordinatorStageTrace {
    /// Existing accepted work could not be represented as exact claims.
    ObligationCollection,
    /// The current resource basis could not produce a bounded allocation horizon.
    CapacityProjection,
    /// The selected Foundry candidate could not be adapted into shared claims.
    FoundryProposalAdaptation,
    /// The selected connected-operation candidate could not be adapted into shared claims.
    ConnectedProposalAdaptation,
    /// A retained saved Foundry could not emit its exact build command.
    SavedFoundryDispatch,
    /// An active connected operation could not retain its accepted producer schedule.
    ActiveConnectedRefresh,
    /// A fresh connected payload could not bind the allocator's exact producer schedule.
    ConnectedProposalBinding,
    /// A bound connected payload could not be installed into its domain planner.
    ConnectedProposalCommit,
    /// A selected Foundry payload could not be installed into its domain planner.
    FoundryProposalCommit,
}

/// One typed failure outside the allocator's portfolio search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AllocationCoordinatorFailureTrace {
    /// Exact phase whose transactional integration failed.
    pub stage: AllocationCoordinatorStageTrace,
    /// Exact reason reported by the shared-resource or domain boundary.
    pub reason: AllocationCoordinatorFailureReasonTrace,
}

/// Exact integration error retained when the coordinator rolls back a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum AllocationCoordinatorFailureReasonTrace {
    /// One exact shared-resource bundle was internally inconsistent.
    Claims {
        /// Structural claim error.
        error: ClaimBundleErrorTrace,
    },
    /// The observed resource basis could not support the requested projection.
    Projection {
        /// Projection error.
        error: PlanningProjectionErrorTrace,
    },
    /// A same-think legacy production command no longer had its observed lane.
    ImmediateProducerUnavailable {
        /// Exact producer named by the command.
        producer: BuildingId,
        /// Unit the command attempted to enqueue.
        kind: UnitKind,
    },
    /// A retained or selected connected schedule failed exact binding.
    ConnectedProducerBinding {
        /// Binding error.
        error: ConnectedProducerBindingErrorTrace,
    },
    /// A bound connected proposal could not be committed unchanged.
    ConnectedProposalCommit {
        /// Commit error.
        error: ConnectedProposalCommitErrorTrace,
    },
    /// A retained payload no longer matched the exact plan it was meant to dispatch.
    ExactDispatchRejected,
    /// Another saved Foundry appeared after proposal derivation.
    ExistingFoundryCommitment,
}

impl From<&CoordinatorInputError> for AllocationCoordinatorFailureReasonTrace {
    fn from(value: &CoordinatorInputError) -> Self {
        match value {
            CoordinatorInputError::Claims(error) => Self::Claims {
                error: (*error).into(),
            },
            CoordinatorInputError::Projection(error) => Self::Projection {
                error: (*error).into(),
            },
            CoordinatorInputError::ImmediateProducerUnavailable { producer, kind } => {
                Self::ImmediateProducerUnavailable {
                    producer: *producer,
                    kind: *kind,
                }
            }
        }
    }
}

impl From<ClaimBundleError> for AllocationCoordinatorFailureReasonTrace {
    fn from(value: ClaimBundleError) -> Self {
        Self::Claims {
            error: value.into(),
        }
    }
}

impl From<ConnectedProducerBindingError> for AllocationCoordinatorFailureReasonTrace {
    fn from(value: ConnectedProducerBindingError) -> Self {
        Self::ConnectedProducerBinding {
            error: value.into(),
        }
    }
}

impl From<ConnectedProposalCommitError> for AllocationCoordinatorFailureReasonTrace {
    fn from(value: ConnectedProposalCommitError) -> Self {
        Self::ConnectedProposalCommit {
            error: value.into(),
        }
    }
}

/// Why a shared claim bundle was structurally invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaimBundleErrorTrace {
    /// The force roster repeated one unit.
    DuplicateUnit {
        /// Repeated unit.
        unit: UnitId,
    },
    /// The builder roster repeated one unit.
    DuplicateBuilder {
        /// Repeated builder.
        builder: UnitId,
    },
    /// One unit appeared as both a builder and a force member.
    ActorRoleOverlap {
        /// Overlapping actor.
        unit: UnitId,
    },
    /// Two construction footprints inside one atomic claim overlapped.
    OverlappingSites {
        /// First canonical footprint.
        first: SiteFootprintTrace,
        /// Second canonical footprint.
        second: SiteFootprintTrace,
    },
    /// Same-deadline future-capital rows overflowed the simulation scrap type.
    ForecastScrapOverflow {
        /// Deadline whose merged amount overflowed.
        through: Tick,
    },
    /// Fixed and allocator-owned capital appeared in the same bundle.
    MixedCapitalFunding,
    /// More than one allocator-owned capital deadline appeared in one bundle.
    DuplicateDeferrableCapital,
}

impl From<ClaimBundleError> for ClaimBundleErrorTrace {
    fn from(value: ClaimBundleError) -> Self {
        match value {
            ClaimBundleError::DuplicateUnit(unit) => Self::DuplicateUnit { unit },
            ClaimBundleError::DuplicateBuilder(builder) => Self::DuplicateBuilder { builder },
            ClaimBundleError::ActorRoleOverlap(unit) => Self::ActorRoleOverlap { unit },
            ClaimBundleError::OverlappingSites { first, second } => Self::OverlappingSites {
                first: first.into(),
                second: second.into(),
            },
            ClaimBundleError::ForecastScrapOverflow(through) => {
                Self::ForecastScrapOverflow { through }
            }
            ClaimBundleError::MixedCapitalFunding => Self::MixedCapitalFunding,
            ClaimBundleError::DuplicateDeferrableCapital => Self::DuplicateDeferrableCapital,
        }
    }
}

/// Why the current fog-honest resource snapshot could not be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanningProjectionErrorTrace {
    /// Decision cadence was zero.
    ZeroCadence,
    /// The observation did not occur on the configured cadence.
    ObservationOffCadence {
        /// Current observation tick.
        observed_at: Tick,
        /// Configured decision cadence.
        cadence: Tick,
    },
    /// The requested horizon did not extend beyond the observation.
    EmptyHorizon {
        /// Current observation tick.
        observed_at: Tick,
        /// Requested inclusive horizon.
        horizon: Tick,
    },
    /// An observed queue exceeded simulation capacity.
    QueueBeyondCapacity {
        /// Exact producer.
        producer: BuildingId,
        /// Observed queue length.
        queued: u32,
    },
    /// Owner-visible front progress exceeded the front unit's train time.
    MalformedFrontProgress {
        /// Exact producer.
        producer: BuildingId,
        /// Observed progress.
        progress: u32,
        /// Complete train time.
        train_ticks: u32,
    },
    /// Queue, cadence, or horizon arithmetic overflowed.
    TickOverflow,
    /// Completed-source income overflowed the simulation scrap type.
    ForecastOverflow {
        /// Last included production tick.
        through: Tick,
    },
    /// A recurring source had a zero payment period.
    ZeroIncomePeriod {
        /// Exact completed income source.
        source: BuildingId,
    },
}

impl From<PlanningProjectionError> for PlanningProjectionErrorTrace {
    fn from(value: PlanningProjectionError) -> Self {
        match value {
            PlanningProjectionError::ZeroCadence => Self::ZeroCadence,
            PlanningProjectionError::ObservationOffCadence {
                observed_at,
                cadence,
            } => Self::ObservationOffCadence {
                observed_at,
                cadence,
            },
            PlanningProjectionError::EmptyHorizon {
                observed_at,
                horizon,
            } => Self::EmptyHorizon {
                observed_at,
                horizon,
            },
            PlanningProjectionError::QueueBeyondCapacity { producer, queued } => {
                Self::QueueBeyondCapacity {
                    producer,
                    queued: bounded_count(queued),
                }
            }
            PlanningProjectionError::MalformedFrontProgress {
                producer,
                progress,
                train_ticks,
            } => Self::MalformedFrontProgress {
                producer,
                progress,
                train_ticks,
            },
            PlanningProjectionError::TickOverflow => Self::TickOverflow,
            PlanningProjectionError::ForecastOverflow { through } => {
                Self::ForecastOverflow { through }
            }
            PlanningProjectionError::ZeroIncomePeriod { source } => {
                Self::ZeroIncomePeriod { source }
            }
        }
    }
}

/// Why exact connected producer assignments could not be retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectedProducerBindingErrorTrace {
    /// A fresh proposal was already bound.
    AlreadyBound,
    /// The active operation changed before its schedule was refreshed.
    StaleActiveOperation,
    /// The allocator returned the wrong number of assignments.
    JobCount {
        /// Expected job count.
        expected: u32,
        /// Actual job count.
        actual: u32,
    },
    /// One assignment belonged to another operation.
    Owner {
        /// Request position that failed.
        request_ordinal: u32,
    },
    /// One assignment had the wrong request position.
    RequestOrdinal {
        /// Expected request position.
        expected: u32,
        /// Actual request position.
        actual: u32,
    },
    /// One assignment trained the wrong unit kind.
    Kind {
        /// Request position that failed.
        request_ordinal: u32,
    },
    /// One assignment used a producer outside the proposal's access set.
    Producer {
        /// Request position that failed.
        request_ordinal: u32,
    },
    /// One assignment changed its exact queue timing.
    Timing {
        /// Request position that failed.
        request_ordinal: u32,
    },
    /// One assignment no longer attributed exactly one unit cost.
    Funding {
        /// Request position that failed.
        request_ordinal: u32,
    },
}

impl From<ConnectedProducerBindingError> for ConnectedProducerBindingErrorTrace {
    fn from(value: ConnectedProducerBindingError) -> Self {
        match value {
            ConnectedProducerBindingError::AlreadyBound => Self::AlreadyBound,
            ConnectedProducerBindingError::StaleActiveOperation => Self::StaleActiveOperation,
            ConnectedProducerBindingError::JobCount { expected, actual } => Self::JobCount {
                expected: bounded_count(expected),
                actual: bounded_count(actual),
            },
            ConnectedProducerBindingError::Owner { request_ordinal } => Self::Owner {
                request_ordinal: bounded_count(request_ordinal),
            },
            ConnectedProducerBindingError::RequestOrdinal { expected, actual } => {
                Self::RequestOrdinal {
                    expected: bounded_count(expected),
                    actual: bounded_count(actual),
                }
            }
            ConnectedProducerBindingError::Kind { request_ordinal } => Self::Kind {
                request_ordinal: bounded_count(request_ordinal),
            },
            ConnectedProducerBindingError::Producer { request_ordinal } => Self::Producer {
                request_ordinal: bounded_count(request_ordinal),
            },
            ConnectedProducerBindingError::Timing { request_ordinal } => Self::Timing {
                request_ordinal: bounded_count(request_ordinal),
            },
            ConnectedProducerBindingError::Funding { request_ordinal } => Self::Funding {
                request_ordinal: bounded_count(request_ordinal),
            },
        }
    }
}

/// Why a bound connected proposal could not enter planner state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectedProposalCommitErrorTrace {
    /// Planner state changed after derivation.
    StalePlanner,
    /// Another connected assault already owned the planner.
    ExistingAssault,
    /// The proposal lacked the allocator's exact producer schedule.
    UnboundProducerSchedule,
}

impl From<ConnectedProposalCommitError> for ConnectedProposalCommitErrorTrace {
    fn from(value: ConnectedProposalCommitError) -> Self {
        match value {
            ConnectedProposalCommitError::StalePlanner => Self::StalePlanner,
            ConnectedProposalCommitError::ExistingAssault => Self::ExistingAssault,
            ConnectedProposalCommitError::UnboundProducerSchedule => Self::UnboundProducerSchedule,
        }
    }
}

/// One exact producer assignment chosen for accepted work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduledProducerJobTrace {
    /// Exact owner of this job.
    pub owner: ClaimOwnerTrace,
    /// Exact completed producer.
    pub producer: BuildingId,
    /// Unit trained by this request.
    pub kind: UnitKind,
    /// Position in the owning plan's ordered request.
    pub request_ordinal: u32,
    /// Tick on which the command pays and enters the queue.
    pub enqueued_at: Tick,
    /// First production tick occupied by this job.
    pub starts_at: Tick,
    /// Tick on which production completes.
    pub ready_at: Tick,
    /// Observation deadline, strictly after readiness.
    pub ready_before: Tick,
    /// Cost funded from the observed current bank.
    pub current_scrap: u32,
    /// Cost funded from completed-source income available at enqueue.
    pub forecast_scrap: u32,
}

impl From<ScheduledProducerJob> for ScheduledProducerJobTrace {
    fn from(job: ScheduledProducerJob) -> Self {
        Self {
            owner: job.owner.into(),
            producer: job.producer,
            kind: job.kind,
            request_ordinal: bounded_count(job.request_ordinal),
            enqueued_at: job.enqueued_at,
            starts_at: job.starts_at,
            ready_at: job.ready_at,
            ready_before: job.ready_before,
            current_scrap: job.current_scrap,
            forecast_scrap: job.forecast_scrap,
        }
    }
}

/// One exact allocator-owned split for deadline-bound non-production capital.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapitalFundingAssignmentTrace {
    /// Exact owner of this capital.
    pub owner: ClaimOwnerTrace,
    /// Last tick by which the assigned capital must be available.
    pub through: Tick,
    /// Capital funded from the observed bank.
    pub current_scrap: u32,
    /// Capital funded from completed-source income.
    pub forecast_scrap: u32,
}

impl From<CapitalFundingAssignment> for CapitalFundingAssignmentTrace {
    fn from(assignment: CapitalFundingAssignment) -> Self {
        Self {
            owner: assignment.owner.into(),
            through: assignment.through,
            current_scrap: assignment.current_scrap,
            forecast_scrap: assignment.forecast_scrap,
        }
    }
}

/// One optional scale step applied after the connected minimum won allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectedMarginalTrace {
    /// Connected proposal being extended.
    pub key: ProposalKeyTrace,
    /// Exact additional claims, excluding the already-selected minimum.
    pub claims: AllocationClaimsTrace,
    /// Whether the additional claims fit the shared residual capacity.
    pub disposition: ConnectedMarginalDispositionTrace,
}

/// Result of one connected marginal scale attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConnectedMarginalDispositionTrace {
    /// The extension fit atomically.
    Accepted,
    /// The extension left the selected minimum unchanged.
    Rejected {
        /// First exact failed claim.
        conflict: AllocationConflictTrace,
    },
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
    fn allocation_claim_trace_preserves_exact_capital_actors_sites_and_job_access() {
        let first_site = SiteFootprint::new(TilePos::new(8, 3), (2, 3)).unwrap();
        let second_site = SiteFootprint::new(TilePos::new(2, 9), (1, 1)).unwrap();
        let claims = ClaimBundle::new(
            77,
            vec![
                super::super::allocation::ForecastClaim {
                    through: 80,
                    amount: 11,
                },
                super::super::allocation::ForecastClaim {
                    through: 40,
                    amount: 7,
                },
            ],
            vec![UnitId(9), UnitId(2)],
            vec![UnitId(8), UnitId(4)],
            vec![first_site, second_site],
            vec![ProducerJobClaim::flexible(
                UnitKind::Sentinel,
                12,
                130,
                vec![BuildingId(8), BuildingId(3), BuildingId(8)],
            )],
        )
        .unwrap();

        let trace = AllocationClaimsTrace::from(&claims);

        assert_eq!(trace.current_scrap, 77);
        assert_eq!(trace.forecast_scrap_total, 18);
        assert_eq!(
            trace.producer_job_scrap_total,
            u128::from(UnitKind::Sentinel.stats().cost)
        );
        assert_eq!(
            trace.claimed_capital,
            77 + 18 + trace.producer_job_scrap_total
        );
        assert_eq!(
            trace.forecast_scrap.entries,
            [
                ForecastClaimTrace {
                    through: 40,
                    amount: 7,
                },
                ForecastClaimTrace {
                    through: 80,
                    amount: 11,
                },
            ]
        );
        assert_eq!(trace.builders.entries, [UnitId(2), UnitId(9)]);
        assert_eq!(trace.units.entries, [UnitId(4), UnitId(8)]);
        assert_eq!(
            trace.sites.entries,
            [
                SiteFootprintTrace {
                    anchor: TilePos::new(8, 3),
                    width: 2,
                    height: 3,
                },
                SiteFootprintTrace {
                    anchor: TilePos::new(2, 9),
                    width: 1,
                    height: 1,
                },
            ]
        );
        assert_eq!(trace.producer_jobs.total, 1);
        assert_eq!(
            trace.producer_jobs.entries[0],
            ProducerJobClaimTrace {
                kind: UnitKind::Sentinel,
                cost: UnitKind::Sentinel.stats().cost,
                enqueue_not_before: 12,
                ready_before: 130,
                access: ProducerJobAccessTrace::Flexible {
                    eligible_producers: BoundedTraceEntries::from_vec(vec![
                        BuildingId(3),
                        BuildingId(8),
                    ]),
                },
            }
        );
    }

    #[test]
    fn allocation_claim_trace_distinguishes_unassigned_flexible_capital() {
        let claims = ClaimBundle::new(
            0,
            Vec::new(),
            vec![UnitId(2)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
        .with_deferrable_capital(super::super::allocation::DeferrableCapitalClaim {
            through: 240,
            amount: 300,
        })
        .unwrap();

        let trace = AllocationClaimsTrace::from(&claims);

        assert_eq!(trace.current_scrap, 0);
        assert_eq!(trace.forecast_scrap_total, 0);
        assert_eq!(trace.producer_job_scrap_total, 0);
        assert_eq!(trace.claimed_capital, 300);
        assert_eq!(
            trace.deferrable_capital,
            Some(ForecastClaimTrace {
                through: 240,
                amount: 300,
            })
        );
    }

    #[test]
    fn fixed_producer_claim_trace_retains_the_immutable_fifo_schedule() {
        let claims = ClaimBundle::new(
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![ProducerJobClaim::fixed(
                BuildingId(5),
                UnitKind::Moth,
                120,
                181,
                420,
                480,
            )],
        )
        .unwrap();

        let trace = AllocationClaimsTrace::from(&claims);

        assert_eq!(
            trace.producer_jobs.entries[0].access,
            ProducerJobAccessTrace::Fixed {
                producer: BuildingId(5),
                enqueued_at: 120,
                starts_at: 181,
                ready_at: 420,
            }
        );
    }

    #[test]
    fn coordinator_failure_trace_preserves_stage_and_exact_reason() {
        let mut trace = AllocationTrace::default();
        let input = CoordinatorInputError::ImmediateProducerUnavailable {
            producer: BuildingId(7),
            kind: UnitKind::Condor,
        };
        trace.record_coordinator_failure(
            AllocationCoordinatorStageTrace::ObligationCollection,
            (&input).into(),
        );
        assert_eq!(
            trace.coordinator_failure,
            Some(AllocationCoordinatorFailureTrace {
                stage: AllocationCoordinatorStageTrace::ObligationCollection,
                reason: AllocationCoordinatorFailureReasonTrace::ImmediateProducerUnavailable {
                    producer: BuildingId(7),
                    kind: UnitKind::Condor,
                },
            })
        );

        trace.record_coordinator_failure(
            AllocationCoordinatorStageTrace::ConnectedProposalBinding,
            ConnectedProducerBindingError::RequestOrdinal {
                expected: usize::MAX,
                actual: 4,
            }
            .into(),
        );
        assert_eq!(
            trace.coordinator_failure,
            Some(AllocationCoordinatorFailureTrace {
                stage: AllocationCoordinatorStageTrace::ConnectedProposalBinding,
                reason: AllocationCoordinatorFailureReasonTrace::ConnectedProducerBinding {
                    error: ConnectedProducerBindingErrorTrace::RequestOrdinal {
                        expected: u32::MAX,
                        actual: 4,
                    },
                },
            })
        );
    }

    #[test]
    fn coordinator_claim_and_projection_errors_retain_their_evidence() {
        let first = SiteFootprint::new(TilePos::new(2, 3), (2, 2)).unwrap();
        let second = SiteFootprint::new(TilePos::new(3, 4), (1, 1)).unwrap();
        let claim_cases = [
            (
                ClaimBundleError::DuplicateUnit(UnitId(1)),
                ClaimBundleErrorTrace::DuplicateUnit { unit: UnitId(1) },
            ),
            (
                ClaimBundleError::DuplicateBuilder(UnitId(2)),
                ClaimBundleErrorTrace::DuplicateBuilder { builder: UnitId(2) },
            ),
            (
                ClaimBundleError::ActorRoleOverlap(UnitId(3)),
                ClaimBundleErrorTrace::ActorRoleOverlap { unit: UnitId(3) },
            ),
            (
                ClaimBundleError::OverlappingSites { first, second },
                ClaimBundleErrorTrace::OverlappingSites {
                    first: first.into(),
                    second: second.into(),
                },
            ),
            (
                ClaimBundleError::ForecastScrapOverflow(90),
                ClaimBundleErrorTrace::ForecastScrapOverflow { through: 90 },
            ),
        ];
        for (error, expected) in claim_cases {
            assert_eq!(ClaimBundleErrorTrace::from(error), expected);
            assert_eq!(
                AllocationCoordinatorFailureReasonTrace::from(error),
                AllocationCoordinatorFailureReasonTrace::Claims { error: expected }
            );
            assert_eq!(
                AllocationCoordinatorFailureReasonTrace::from(&CoordinatorInputError::Claims(
                    error,
                )),
                AllocationCoordinatorFailureReasonTrace::Claims { error: expected }
            );
        }

        let projection_cases = [
            (
                PlanningProjectionError::ZeroCadence,
                PlanningProjectionErrorTrace::ZeroCadence,
            ),
            (
                PlanningProjectionError::ObservationOffCadence {
                    observed_at: 13,
                    cadence: 12,
                },
                PlanningProjectionErrorTrace::ObservationOffCadence {
                    observed_at: 13,
                    cadence: 12,
                },
            ),
            (
                PlanningProjectionError::EmptyHorizon {
                    observed_at: 20,
                    horizon: 20,
                },
                PlanningProjectionErrorTrace::EmptyHorizon {
                    observed_at: 20,
                    horizon: 20,
                },
            ),
            (
                PlanningProjectionError::QueueBeyondCapacity {
                    producer: BuildingId(4),
                    queued: usize::MAX,
                },
                PlanningProjectionErrorTrace::QueueBeyondCapacity {
                    producer: BuildingId(4),
                    queued: u32::MAX,
                },
            ),
            (
                PlanningProjectionError::MalformedFrontProgress {
                    producer: BuildingId(5),
                    progress: 31,
                    train_ticks: 30,
                },
                PlanningProjectionErrorTrace::MalformedFrontProgress {
                    producer: BuildingId(5),
                    progress: 31,
                    train_ticks: 30,
                },
            ),
            (
                PlanningProjectionError::TickOverflow,
                PlanningProjectionErrorTrace::TickOverflow,
            ),
            (
                PlanningProjectionError::ForecastOverflow { through: 300 },
                PlanningProjectionErrorTrace::ForecastOverflow { through: 300 },
            ),
            (
                PlanningProjectionError::ZeroIncomePeriod {
                    source: BuildingId(6),
                },
                PlanningProjectionErrorTrace::ZeroIncomePeriod {
                    source: BuildingId(6),
                },
            ),
        ];
        for (error, expected) in projection_cases {
            assert_eq!(PlanningProjectionErrorTrace::from(error), expected);
            assert_eq!(
                AllocationCoordinatorFailureReasonTrace::from(&CoordinatorInputError::Projection(
                    error
                )),
                AllocationCoordinatorFailureReasonTrace::Projection { error: expected }
            );
        }
    }

    #[test]
    fn connected_binding_and_commit_errors_remain_distinguishable() {
        let binding_cases = [
            (
                ConnectedProducerBindingError::AlreadyBound,
                ConnectedProducerBindingErrorTrace::AlreadyBound,
            ),
            (
                ConnectedProducerBindingError::StaleActiveOperation,
                ConnectedProducerBindingErrorTrace::StaleActiveOperation,
            ),
            (
                ConnectedProducerBindingError::JobCount {
                    expected: usize::MAX,
                    actual: 3,
                },
                ConnectedProducerBindingErrorTrace::JobCount {
                    expected: u32::MAX,
                    actual: 3,
                },
            ),
            (
                ConnectedProducerBindingError::Owner { request_ordinal: 4 },
                ConnectedProducerBindingErrorTrace::Owner { request_ordinal: 4 },
            ),
            (
                ConnectedProducerBindingError::RequestOrdinal {
                    expected: 5,
                    actual: 6,
                },
                ConnectedProducerBindingErrorTrace::RequestOrdinal {
                    expected: 5,
                    actual: 6,
                },
            ),
            (
                ConnectedProducerBindingError::Kind { request_ordinal: 7 },
                ConnectedProducerBindingErrorTrace::Kind { request_ordinal: 7 },
            ),
            (
                ConnectedProducerBindingError::Producer { request_ordinal: 8 },
                ConnectedProducerBindingErrorTrace::Producer { request_ordinal: 8 },
            ),
            (
                ConnectedProducerBindingError::Timing { request_ordinal: 9 },
                ConnectedProducerBindingErrorTrace::Timing { request_ordinal: 9 },
            ),
            (
                ConnectedProducerBindingError::Funding {
                    request_ordinal: 10,
                },
                ConnectedProducerBindingErrorTrace::Funding {
                    request_ordinal: 10,
                },
            ),
        ];
        for (error, expected) in binding_cases {
            assert_eq!(ConnectedProducerBindingErrorTrace::from(error), expected);
            assert_eq!(
                AllocationCoordinatorFailureReasonTrace::from(error),
                AllocationCoordinatorFailureReasonTrace::ConnectedProducerBinding {
                    error: expected,
                }
            );
        }

        let commit_cases = [
            (
                ConnectedProposalCommitError::StalePlanner,
                ConnectedProposalCommitErrorTrace::StalePlanner,
            ),
            (
                ConnectedProposalCommitError::ExistingAssault,
                ConnectedProposalCommitErrorTrace::ExistingAssault,
            ),
            (
                ConnectedProposalCommitError::UnboundProducerSchedule,
                ConnectedProposalCommitErrorTrace::UnboundProducerSchedule,
            ),
        ];
        for (error, expected) in commit_cases {
            assert_eq!(ConnectedProposalCommitErrorTrace::from(error), expected);
            assert_eq!(
                AllocationCoordinatorFailureReasonTrace::from(error),
                AllocationCoordinatorFailureReasonTrace::ConnectedProposalCommit {
                    error: expected,
                }
            );
        }
    }

    #[test]
    fn allocation_trace_bounds_canonical_claims_and_obligations_explicitly() {
        let units = (0..ALLOCATION_TRACE_ENTRY_LIMIT + 5)
            .rev()
            .map(|id| UnitId(u32::try_from(id).unwrap()))
            .collect();
        let claims =
            ClaimBundle::new(0, Vec::new(), Vec::new(), units, Vec::new(), Vec::new()).unwrap();
        let claim_trace = AllocationClaimsTrace::from(&claims);

        assert_eq!(
            claim_trace.units.total,
            u32::try_from(ALLOCATION_TRACE_ENTRY_LIMIT + 5).unwrap()
        );
        assert_eq!(
            claim_trace.units.entries.len(),
            ALLOCATION_TRACE_ENTRY_LIMIT
        );
        assert_eq!(claim_trace.units.entries[0], UnitId(0));
        assert_eq!(
            claim_trace.units.entries.last(),
            Some(&UnitId(
                u32::try_from(ALLOCATION_TRACE_ENTRY_LIMIT - 1).unwrap()
            ))
        );
        assert_eq!(claim_trace.units.omitted, 5);

        let capital_assignments = (0..ALLOCATION_TRACE_ENTRY_LIMIT + 2)
            .map(|sequence| CapitalFundingAssignmentTrace {
                owner: ClaimOwnerTrace::Obligation {
                    class: ObligationClassTrace::PersistentPlan,
                    accepted_at: u64::try_from(sequence).unwrap(),
                    key: ObligationKeyTrace::OpeningCore {
                        sequence: u16::try_from(sequence).unwrap(),
                    },
                },
                through: 500,
                current_scrap: 1,
                forecast_scrap: 2,
            })
            .collect();
        let capital_assignments = BoundedTraceEntries::from_vec(capital_assignments);
        assert_eq!(
            capital_assignments.entries.len(),
            ALLOCATION_TRACE_ENTRY_LIMIT
        );
        assert_eq!(capital_assignments.omitted, 2);

        let obligations = (0..ALLOCATION_TRACE_ENTRY_LIMIT + 3)
            .rev()
            .map(|sequence| ImportedObligation {
                class: ObligationClass::Survival,
                accepted_at: 20,
                key: ObligationKey::OpeningCore {
                    sequence: u16::try_from(sequence).unwrap(),
                },
                claims: ClaimBundle::default(),
            })
            .collect::<Vec<_>>();
        let allocation = AllocationTrace::from_inputs::<(), ()>(&obligations, &[]);

        assert_eq!(
            allocation.obligations.entries.len(),
            ALLOCATION_TRACE_ENTRY_LIMIT
        );
        assert_eq!(allocation.obligations.omitted, 3);
        assert_eq!(
            allocation.obligations.entries[0].key,
            ObligationKeyTrace::OpeningCore { sequence: 0 }
        );
    }

    #[test]
    fn allocation_trace_records_dispositions_and_preserves_selected_lane_order() {
        let expansion =
            ProposalKey::FoundryExpansion(super::super::allocation::FoundryExpansionKey {
                anchor: TilePos::new(4, 5),
            });
        let offense_key = ConnectedOffenseKey {
            objective: BuildingId(90),
            anchor: TilePos::new(12, 9),
        };
        let offense = ProposalKey::ConnectedOffenseMinimum(offense_key);
        let case = ProposalCaseTrace {
            urgency: UrgencyTrace::Timely,
            confidence: ConfidenceTrace::Current,
            value: StrategicValueTrace::Material,
            time_to_impact: TimeToImpactTrace::Near,
            safety: ExecutionSafetyTrace::Managed,
        };
        let mut trace = AllocationTrace {
            proposals: BoundedTraceEntries::from_vec(vec![
                AllocationProposalTrace {
                    key: expansion.into(),
                    case,
                    personality_weight: None,
                    claims: AllocationClaimsTrace::default(),
                    disposition: ProposalDispositionTrace::NotEvaluated,
                },
                AllocationProposalTrace {
                    key: offense.into(),
                    case,
                    personality_weight: None,
                    claims: AllocationClaimsTrace::default(),
                    disposition: ProposalDispositionTrace::NotEvaluated,
                },
            ]),
            ..AllocationTrace::default()
        };
        trace.record_decisions(&[
            ProposalDecision {
                key: expansion,
                case: ProposalCase {
                    urgency: Urgency::Timely,
                    confidence: Confidence::Current,
                    value: StrategicValue::Material,
                    time_to_impact: TimeToImpact::Near,
                    safety: ExecutionSafety::Managed,
                },
                personality_weight: 141,
                disposition: ProposalDisposition::Accepted,
            },
            ProposalDecision {
                key: offense,
                case: ProposalCase {
                    urgency: Urgency::Timely,
                    confidence: Confidence::Current,
                    value: StrategicValue::Material,
                    time_to_impact: TimeToImpact::Near,
                    safety: ExecutionSafety::Managed,
                },
                personality_weight: 163,
                disposition: ProposalDisposition::Rejected(
                    ProposalRejection::ConflictsWithSelected {
                        selected: vec![expansion],
                        conflict: AllocationConflict::CurrentScrap {
                            requested: 210,
                            available: 180,
                        },
                    },
                ),
            },
        ]);
        let schedule = [
            ScheduledProducerJob {
                owner: ClaimOwner::Proposal(offense),
                producer: BuildingId(7),
                kind: UnitKind::Bombard,
                request_ordinal: 1,
                enqueued_at: 60,
                starts_at: 80,
                ready_at: 179,
                ready_before: 200,
                current_scrap: 0,
                forecast_scrap: 70,
            },
            ScheduledProducerJob {
                owner: ClaimOwner::Proposal(expansion),
                producer: BuildingId(3),
                kind: UnitKind::Sentinel,
                request_ordinal: 0,
                enqueued_at: 12,
                starts_at: 12,
                ready_at: 111,
                ready_before: 150,
                current_scrap: 50,
                forecast_scrap: 0,
            },
        ];
        trace.record_producer_schedule(&schedule);

        assert_eq!(trace.proposals.entries[0].personality_weight, Some(141));
        assert_eq!(
            trace.proposals.entries[0].disposition,
            ProposalDispositionTrace::Accepted
        );
        assert_eq!(trace.proposals.entries[1].personality_weight, Some(163));
        assert_eq!(
            trace.proposals.entries[1].disposition,
            ProposalDispositionTrace::ConflictsWithSelected {
                selected: BoundedTraceEntries::from_vec(vec![expansion.into()]),
                conflict: AllocationConflictTrace::CurrentScrap {
                    requested: 210,
                    available: 180,
                },
            }
        );
        assert_eq!(
            trace
                .producer_schedule
                .entries
                .iter()
                .map(|job| job.producer)
                .collect::<Vec<_>>(),
            [BuildingId(7), BuildingId(3)],
            "the trace must retain the allocator's exact schedule order"
        );
        assert_eq!(
            ProposalDispositionTrace::from(ProposalDisposition::Rejected(
                ProposalRejection::Outranked {
                    selected: vec![expansion],
                    basis: OutrankingBasis::Personality,
                },
            )),
            ProposalDispositionTrace::Outranked {
                selected: BoundedTraceEntries::from_vec(vec![expansion.into()]),
                basis: OutrankingBasisTrace::Personality,
            }
        );
    }

    #[test]
    fn connected_marginal_trace_distinguishes_atomic_acceptance_from_rejection() {
        let key = ConnectedOffenseKey {
            objective: BuildingId(20),
            anchor: TilePos::new(9, 8),
        };
        let claims = ClaimBundle::new(
            0,
            Vec::new(),
            Vec::new(),
            vec![UnitId(4)],
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let schedule = [ScheduledProducerJob {
            owner: ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(key)),
            producer: BuildingId(3),
            kind: UnitKind::Moth,
            request_ordinal: 2,
            enqueued_at: 100,
            starts_at: 120,
            ready_at: 319,
            ready_before: 350,
            current_scrap: 0,
            forecast_scrap: 90,
        }];
        let mut trace = AllocationTrace::default();

        trace.record_connected_marginal_accepted(key, &claims, &schedule);

        assert_eq!(
            trace.connected_marginal,
            Some(ConnectedMarginalTrace {
                key: key.into(),
                claims: (&claims).into(),
                disposition: ConnectedMarginalDispositionTrace::Accepted,
            })
        );
        assert_eq!(trace.producer_schedule.entries[0].kind, UnitKind::Moth);

        let conflict = AllocationConflict::ProducerSchedule {
            producers: vec![BuildingId(9), BuildingId(3)],
            owners: vec![
                ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(key)),
                ClaimOwner::Obligation {
                    class: ObligationClass::PersistentPlan,
                    accepted_at: 10,
                    key: ObligationKey::SavedFoundry {
                        anchor: TilePos::new(3, 4),
                    },
                },
            ],
        };
        trace.record_connected_marginal_rejected(key, &claims, &conflict);

        let Some(ConnectedMarginalTrace {
            disposition: ConnectedMarginalDispositionTrace::Rejected { conflict },
            ..
        }) = trace.connected_marginal
        else {
            panic!("the rejected marginal attempt remains explicit");
        };
        let AllocationConflictTrace::ProducerSchedule {
            producers, owners, ..
        } = conflict
        else {
            panic!("the exact producer conflict remains visible");
        };
        assert_eq!(producers.entries, [BuildingId(9), BuildingId(3)]);
        assert_eq!(owners.total, 2);
    }

    #[test]
    fn allocation_error_trace_retains_the_conflicting_obligation() {
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PaidWork,
            accepted_at: 31,
            key: ObligationKey::ObservedBuilderWork { builder: UnitId(8) },
        };
        let mut trace = AllocationTrace::default();

        trace.record_error(&AllocationError::ObligationConflict {
            obligation: owner,
            conflict: AllocationConflict::Actor {
                unit: UnitId(5),
                existing: owner,
            },
        });

        assert_eq!(
            trace.error,
            Some(AllocationErrorTrace::ObligationConflict {
                obligation: owner.into(),
                conflict: AllocationConflictTrace::Actor {
                    unit: UnitId(5),
                    existing: owner.into(),
                },
            })
        );
        assert_eq!(
            LegacyChannelTrace::from(LegacyChannel::StandingArmy),
            LegacyChannelTrace::StandingArmy
        );
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
        let expansion_key = ProposalKeyTrace::FoundryExpansion {
            anchor: TilePos::new(6, 5),
        };
        trace.allocation = AllocationTrace {
            obligations: BoundedTraceEntries::from_vec(vec![AllocationObligationTrace {
                class: ObligationClassTrace::Survival,
                accepted_at: 8,
                key: ObligationKeyTrace::OpeningCore { sequence: 1 },
                claims: AllocationClaimsTrace {
                    current_scrap: 50,
                    claimed_capital: 50,
                    builders: BoundedTraceEntries::from_vec(vec![UnitId(2)]),
                    ..AllocationClaimsTrace::default()
                },
            }]),
            proposals: BoundedTraceEntries::from_vec(vec![AllocationProposalTrace {
                key: expansion_key,
                case: ProposalCaseTrace {
                    urgency: UrgencyTrace::Timely,
                    confidence: ConfidenceTrace::Current,
                    value: StrategicValueTrace::Material,
                    time_to_impact: TimeToImpactTrace::Near,
                    safety: ExecutionSafetyTrace::Secure,
                },
                personality_weight: Some(147),
                claims: AllocationClaimsTrace {
                    current_scrap: 150,
                    producer_job_scrap_total: u128::from(UnitKind::Sentinel.stats().cost),
                    claimed_capital: 150 + u128::from(UnitKind::Sentinel.stats().cost),
                    sites: BoundedTraceEntries::from_vec(vec![SiteFootprintTrace {
                        anchor: TilePos::new(6, 5),
                        width: 3,
                        height: 3,
                    }]),
                    producer_jobs: BoundedTraceEntries::from_vec(vec![ProducerJobClaimTrace {
                        kind: UnitKind::Sentinel,
                        cost: UnitKind::Sentinel.stats().cost,
                        enqueue_not_before: 10,
                        ready_before: 120,
                        access: ProducerJobAccessTrace::Flexible {
                            eligible_producers: BoundedTraceEntries::from_vec(vec![BuildingId(3)]),
                        },
                    }]),
                    ..AllocationClaimsTrace::default()
                },
                disposition: ProposalDispositionTrace::Accepted,
            }]),
            producer_schedule: BoundedTraceEntries::from_vec(vec![ScheduledProducerJobTrace {
                owner: ClaimOwnerTrace::Proposal { key: expansion_key },
                producer: BuildingId(3),
                kind: UnitKind::Sentinel,
                request_ordinal: 0,
                enqueued_at: 10,
                starts_at: 10,
                ready_at: 109,
                ready_before: 120,
                current_scrap: 50,
                forecast_scrap: 0,
            }]),
            capital_assignments: BoundedTraceEntries::default(),
            error: None,
            coordinator_failure: None,
            connected_marginal: Some(ConnectedMarginalTrace {
                key: ProposalKeyTrace::ConnectedOffenseMinimum {
                    objective: BuildingId(9),
                    anchor: TilePos::new(12, 7),
                },
                claims: AllocationClaimsTrace {
                    current_scrap: 40,
                    claimed_capital: 40,
                    ..AllocationClaimsTrace::default()
                },
                disposition: ConnectedMarginalDispositionTrace::Rejected {
                    conflict: AllocationConflictTrace::CurrentScrap {
                        requested: 240,
                        available: 220,
                    },
                },
            }),
        };
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

        assert_eq!(trace.get("version"), Some(&Value::from(5)));

        assert_eq!(
            trace.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "budget",
                "allocation",
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

        let Some(Value::Object(allocation)) = trace.get("allocation") else {
            panic!("allocation trace serializes as an object");
        };
        assert_eq!(
            allocation
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "connected_marginal",
                "capital_assignments",
                "coordinator_failure",
                "error",
                "obligations",
                "producer_schedule",
                "proposals",
            ])
        );
        for field in [
            "obligations",
            "proposals",
            "producer_schedule",
            "capital_assignments",
        ] {
            let Some(Value::Object(entries)) = allocation.get(field) else {
                panic!("allocation {field} serializes as a bounded object");
            };
            assert_eq!(
                entries.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                BTreeSet::from(["entries", "omitted", "total"]),
                "allocation {field} changed bounded-entry schema"
            );
        }
        let Some(Value::Array(proposals)) = allocation
            .get("proposals")
            .and_then(|value| value.get("entries"))
        else {
            panic!("allocation proposals contain entries");
        };
        let Some(Value::Object(proposal)) = proposals.first() else {
            panic!("allocation proposal serializes as an object");
        };
        assert_eq!(
            proposal.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["case", "claims", "disposition", "key", "personality_weight",])
        );
        let Some(Value::Object(case)) = proposal.get("case") else {
            panic!("proposal case serializes as an object");
        };
        assert_eq!(
            case.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["confidence", "safety", "time_to_impact", "urgency", "value",])
        );
        let Some(Value::Object(claims)) = proposal.get("claims") else {
            panic!("proposal claims serialize as an object");
        };
        assert_eq!(
            claims.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "builders",
                "claimed_capital",
                "current_scrap",
                "deferrable_capital",
                "forecast_scrap",
                "forecast_scrap_total",
                "producer_jobs",
                "producer_job_scrap_total",
                "sites",
                "units",
            ])
        );
        let Some(Value::Object(producer_job)) = claims
            .get("producer_jobs")
            .and_then(|value| value.get("entries"))
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
        else {
            panic!("proposal production request serializes as an object");
        };
        assert_eq!(
            producer_job
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "access",
                "cost",
                "enqueue_not_before",
                "kind",
                "ready_before",
            ])
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
