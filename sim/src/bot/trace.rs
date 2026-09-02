//! Opt-in, fog-honest diagnostics for one player-facing bot decision.
//!
//! Traces are observational output. They are not controller memory, simulation
//! state, events, or replay input, and nothing in the policy may read them back.

use super::observation::Observation;
use crate::PlayerCommand;
use crate::ids::{PlayerId, UnitId};
use chassis::Tick;
use serde::Serialize;

/// Schema version for serialized decision traces.
pub const DECISION_TRACE_VERSION: u32 = 1;

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
    /// Coordinator-owned gates. `None` means the full strategic path did not
    /// reach that gate.
    pub gates: GateTrace,
    /// Existing coordinator scrap holds. Absent on an early recovery return.
    pub budget: Option<ScrapBudgetTrace>,
    /// Fixed current planner channels, in schema rather than insertion order.
    pub channels: ChannelTraces,
    /// Input and output size of the utility-policy pass.
    pub utility: UtilityTrace,
    /// Final intent-to-command lowering summary.
    pub lowering: LoweringTrace,
}

impl DecisionTrace {
    fn from_observation(observation: &Observation) -> Self {
        Self {
            version: DECISION_TRACE_VERSION,
            tick: observation.tick,
            player: observation.me,
            control_flow: DecisionControlFlow::Policy,
            evidence: EvidenceTrace {
                scrap: observation.scrap,
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
            gates: GateTrace::default(),
            budget: None,
            channels: ChannelTraces::default(),
            utility: UtilityTrace::default(),
            lowering: LoweringTrace::default(),
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
    /// Own current bank.
    pub scrap: u32,
    /// Currently visible hostile units.
    pub current_enemy_units: u32,
    /// Currently visible hostile buildings.
    pub current_enemy_buildings: u32,
    /// Hostile building ghosts currently present in the observation.
    pub remembered_enemy_buildings: u32,
    /// Unidentified hostile radar contacts.
    pub radar_blips: u32,
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
    /// Scrap exposed to strategic planners after those holds.
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
    /// Connected operation committed its bomber wing.
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
            deferred_construction: 2,
            airworks_capacity: 3,
            shallow_sentinel: 4,
            opening_bootstrap: 5,
            frozen: false,
            strategic_spendable: 6,
            strategic_committed: 7,
            prospective_carrier: 8,
            utility_spendable: 9,
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
        let Value::Object(trace) = serde_json::to_value(trace).expect("the trace serializes")
        else {
            panic!("a decision trace serializes as an object");
        };

        assert_eq!(
            trace.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "budget",
                "channels",
                "control_flow",
                "evidence",
                "gates",
                "lowering",
                "player",
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
                "evidence",
                BTreeSet::from([
                    "current_enemy_buildings",
                    "current_enemy_units",
                    "radar_blips",
                    "remembered_enemy_buildings",
                    "scrap",
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
                    "frozen",
                    "opening_bootstrap",
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
}
