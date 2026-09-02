//! Deterministic, player-facing bot match evaluation.
//!
//! This runner complements the frozen Overseer sweeps: it executes the bot
//! configuration serialized in an ordinary scenario, stops when the match is
//! decided, and emits one compact row suitable for JSONL comparison.

use anyhow::{Context, Result, bail, ensure};
use oxide_kit::GameReplay;
use oxide_sim::bot::{DecisionTrace, PublicMapBriefing, ResolvedProfile, SeatBot};
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use oxide_sim::{Event, Faction, GameResult, PlayerId, SIM_VERSION, Scenario};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_CANDIDATE_LEN: usize = 128;

/// Which half of a seat-paired evaluation produced a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationLeg {
    /// One ordinary evaluation with no profile exchange.
    Single,
    /// The first leg, before exchanging the two profiles.
    Forward,
    /// The second leg, after exchanging the two profiles.
    Swapped,
}

impl EvaluationLeg {
    /// Stable filename component for replay evidence.
    pub fn name(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Forward => "forward",
            Self::Swapped => "swapped",
        }
    }
}

/// Controller family used by one evaluation seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationControllerKind {
    /// No automated command source occupies this seat.
    None,
    /// The configurable opponent exposed to players.
    Scripted,
    /// The frozen pre-0.16 QA yardstick.
    Overseer,
}

/// Exact evaluation-only command source for one seat.
///
/// Overseer deliberately remains outside [`BotConfig`]: it is a frozen QA
/// baseline, not a controller ordinary matches may select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvaluationController {
    /// One exact player-facing configuration.
    Scripted {
        /// Difficulty, stance, and deterministic personality seed.
        config: BotConfig,
    },
    /// The frozen profile-free controller with one seat-independent policy
    /// identity.
    Overseer {
        /// Seed for the legacy army-size jitter, evaluated on its canonical
        /// stream regardless of which physical seat this leg drives.
        policy_seed: u64,
    },
}

impl EvaluationController {
    fn kind(self) -> EvaluationControllerKind {
        match self {
            Self::Scripted { .. } => EvaluationControllerKind::Scripted,
            Self::Overseer { .. } => EvaluationControllerKind::Overseer,
        }
    }

    fn config(self) -> Option<BotConfig> {
        match self {
            Self::Scripted { config } => Some(config),
            Self::Overseer { .. } => None,
        }
    }

    fn overseer_policy_seed(self) -> Option<u64> {
        match self {
            Self::Scripted { .. } => None,
            Self::Overseer { policy_seed } => Some(policy_seed),
        }
    }

    fn seat_bot(self, player: PlayerId, public_map: &Arc<PublicMapBriefing>) -> SeatBot {
        match self {
            Self::Scripted { config } => SeatBot::scripted(player, config, Arc::clone(public_map)),
            Self::Overseer { policy_seed } => {
                SeatBot::overseer_with_policy_seed(player, policy_seed)
            }
        }
    }
}

/// Map-end transform applied to a controlled evaluation cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationGeometry {
    /// Use the scenario exactly as authored.
    Authored,
    /// Rotate every spatial scenario field by 180 degrees.
    Rot180,
}

impl std::str::FromStr for EvaluationGeometry {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "authored" => Ok(Self::Authored),
            "rot180" => Ok(Self::Rot180),
            _ => Err(format!(
                "unknown evaluation geometry {value:?}; expected authored or rot180"
            )),
        }
    }
}

impl EvaluationGeometry {
    fn apply(self, scenario: &Scenario) -> Result<Scenario> {
        match self {
            Self::Authored => Ok(scenario.clone()),
            Self::Rot180 => crate::factorial::rotate_180(scenario),
        }
    }
}

/// Two-seat faction assignment for a controlled evaluation cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationFactionCell {
    /// Preserve the scenario's authored factions.
    Authored,
    /// Ferrous seat zero, Cupric seat one.
    Fc,
    /// Cupric seat zero, Ferrous seat one.
    Cf,
    /// Ferrous in both seats.
    Ff,
    /// Cupric in both seats.
    Cc,
}

impl std::str::FromStr for EvaluationFactionCell {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "authored" => Ok(Self::Authored),
            "fc" => Ok(Self::Fc),
            "cf" => Ok(Self::Cf),
            "ff" => Ok(Self::Ff),
            "cc" => Ok(Self::Cc),
            _ => Err(format!(
                "unknown evaluation faction cell {value:?}; expected authored, fc, cf, ff, or cc"
            )),
        }
    }
}

impl EvaluationFactionCell {
    fn apply(self, scenario: &mut Scenario) -> Result<()> {
        let factions = match self {
            Self::Authored => return Ok(()),
            Self::Fc => [Faction::Ferrous, Faction::Cupric],
            Self::Cf => [Faction::Cupric, Faction::Ferrous],
            Self::Ff => [Faction::Ferrous, Faction::Ferrous],
            Self::Cc => [Faction::Cupric, Faction::Cupric],
        };
        ensure!(
            scenario.players.len() == 2,
            "controlled faction cells require exactly two seats, got {}",
            scenario.players.len()
        );
        for (seat, faction) in factions.into_iter().enumerate() {
            scenario.retint_seat(seat, faction);
        }
        Ok(())
    }
}

/// One exact evaluation leg, including command sources that are intentionally
/// not serializable into an ordinary match setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationPlan {
    /// Seat-pair leg.
    pub leg: EvaluationLeg,
    /// Fully transformed scenario used to build the state and replay.
    pub scenario: Scenario,
    /// Exact evaluation-only controller per seat; `None` is an empty chair.
    pub controllers: Vec<Option<EvaluationController>>,
    /// Spatial transform applied to the source scenario.
    pub geometry: EvaluationGeometry,
    /// Faction assignment applied after the geometry transform.
    pub faction_cell: EvaluationFactionCell,
}

impl EvaluationPlan {
    /// Adapts an ordinary configured scenario to the evaluation runner.
    pub fn from_scenario(scenario: Scenario, leg: EvaluationLeg) -> Self {
        let controllers = scenario
            .players
            .iter()
            .map(|player| {
                (player.bot)
                    .then_some(player.bot_config)
                    .flatten()
                    .map(|config| EvaluationController::Scripted { config })
            })
            .collect();
        Self {
            leg,
            scenario,
            controllers,
            geometry: EvaluationGeometry::Authored,
            faction_cell: EvaluationFactionCell::Authored,
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.controllers.len() == self.scenario.players.len(),
            "evaluation plan has {} controllers for {} seats",
            self.controllers.len(),
            self.scenario.players.len()
        );
        Ok(())
    }

    fn seat_bots(&self) -> Result<Vec<SeatBot>> {
        let public_map = Arc::new(
            PublicMapBriefing::from_scenario(&self.scenario)
                .context("building evaluation public map briefing")?,
        );
        Ok(self
            .controllers
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(seat, controller)| {
                controller.map(|controller| controller.seat_bot(PlayerId(seat as u8), &public_map))
            })
            .collect())
    }
}

/// Why an evaluation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    /// The simulation declared a result.
    Decided,
    /// The configured tick ceiling was reached first.
    TickLimit,
    /// One unit stalled the same way often enough to prove a controller
    /// was re-issuing an impossible order; the leg stopped measuring.
    StallLoop,
}

/// Stalls of one reason on one unit that end a leg as a [`Termination::StallLoop`]
/// when no explicit limit is given. A blocked order that a controller
/// abandons stalls a handful of times; an order re-issued every think on a
/// severed map stalls hundreds of times and drowns every other metric.
pub const DEFAULT_STALL_LOOP_LIMIT: u64 = 200;

/// The order loop that ended a leg: one unit, one stall reason, `count`
/// occurrences by `tick`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StallLoop {
    /// Seat whose controller kept re-issuing the order.
    pub seat: u8,
    /// The unit that kept stalling.
    pub unit: u32,
    /// Stable wire name of the stall reason.
    pub reason: String,
    /// Stalls of that reason on that unit when the leg stopped.
    pub count: u64,
    /// Simulation tick at which the leg stopped.
    pub tick: u64,
}

/// One stall as counted into the evidence: the seat, unit, reason, and the
/// running total for that unit and reason.
struct StallSample {
    seat: u8,
    unit: u32,
    reason: String,
    count: u64,
}

/// The frozen Overseer has no severed-ground play: its legacy ferry and army
/// channels re-issue unreachable orders on every think, so on a map whose
/// seats share no ground route the yardstick measures a missing capability
/// rather than Prime. Such a cell is refused instead of recorded.
pub fn ensure_overseer_yardstick_ground(scenario: &Scenario) -> Result<()> {
    let audit = crate::audit::audit(scenario)
        .with_context(|| format!("auditing {} for the Overseer yardstick", scenario.name))?;
    if let Some(route) = audit
        .routes
        .iter()
        .find(|route| route.ground_steps.is_none())
    {
        bail!(
            "{}: seats {} and {} share no ground route, and the frozen Overseer has no severed-ground play; it is not a valid yardstick there, so compare player-facing profiles instead",
            scenario.name,
            route.seats.0,
            route.seats.1
        );
    }
    Ok(())
}

/// Controller choices for one evaluation cell.
///
/// The primary values apply to seat zero and, unless overridden, every other
/// seat. Opponent overrides and a shared personality seed are intentionally
/// two-seat features so a comparison never has an ambiguous "opponent".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileMatchup {
    /// Difficulty assigned to the primary seat.
    pub difficulty: BotDifficulty,
    /// Stance assigned to the primary seat.
    pub stance: BotStance,
    /// Optional difficulty assigned to seat one.
    pub opponent_difficulty: Option<BotDifficulty>,
    /// Optional stance assigned to seat one.
    pub opponent_stance: Option<BotStance>,
    /// Give both comparison seats the same personality seed.
    pub same_personality_seed: bool,
}

impl ProfileMatchup {
    /// A uniform matchup with distinct consecutive personality seeds.
    pub const fn uniform(difficulty: BotDifficulty, stance: BotStance) -> Self {
        Self {
            difficulty,
            stance,
            opponent_difficulty: None,
            opponent_stance: None,
            same_personality_seed: false,
        }
    }

    /// Returns the deterministic personality-seed base for `run`.
    ///
    /// Distinct-seat cells retain the original consecutive-seat arithmetic.
    /// Shared-seed comparisons consume one personality seed per run instead.
    pub fn personality_seed_base_for_run(
        self,
        initial_seed: u64,
        run: u64,
        seat_count: usize,
    ) -> Result<u64> {
        let stride = if self.same_personality_seed {
            1
        } else {
            u64::try_from(seat_count).expect("seat count fits u64")
        };
        let offset = run
            .checked_mul(stride)
            .context("personality seed range overflows u64")?;
        initial_seed
            .checked_add(offset)
            .context("personality seed range overflows u64")
    }

    fn requires_two_seats(self, paired: bool) -> bool {
        paired
            || self.opponent_difficulty.is_some()
            || self.opponent_stance.is_some()
            || self.same_personality_seed
    }

    fn config_for_seat(self, seat: usize, personality_seed: u64) -> BotConfig {
        let (difficulty, stance) = if seat == 1 {
            (
                self.opponent_difficulty.unwrap_or(self.difficulty),
                self.opponent_stance.unwrap_or(self.stance),
            )
        } else {
            (self.difficulty, self.stance)
        };
        BotConfig::scripted(difficulty, stance, personality_seed)
    }
}

/// Exact controller provenance for one seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeatConfiguration {
    /// Player seat.
    pub seat: u8,
    /// Command-source family used for this leg.
    pub controller: EvaluationControllerKind,
    /// Faction roster bound to the physical seat.
    pub faction: Faction,
    /// The configured built-in controller, or `None` for an empty chair.
    pub config: Option<BotConfig>,
    /// Frozen Overseer policy identity, absent for other command sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overseer_policy_seed: Option<u64>,
    /// Fully resolved hidden personality, included for exact comparison.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ResolvedProfile>,
}

/// Compact command failure evidence for one seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeatEvidence {
    /// Player seat.
    pub seat: u8,
    /// Commands emitted by this seat's controller.
    pub commands: u64,
    /// Commands rejected by the shared simulation rules.
    pub rejections: u64,
    /// Rejections grouped by their stable wire name.
    pub rejection_reasons: BTreeMap<String, u64>,
    /// Unit orders that could not complete.
    pub stalls: u64,
    /// Stalls grouped by their stable wire name.
    pub stall_reasons: BTreeMap<String, u64>,
    /// Per-unit stall reasons, so a large total can be traced to one stuck
    /// order instead of being mistaken for a controller-wide command storm.
    pub stall_units: BTreeMap<u32, BTreeMap<String, u64>>,
}

impl SeatEvidence {
    fn new(seat: usize) -> Self {
        Self {
            seat: seat as u8,
            commands: 0,
            rejections: 0,
            rejection_reasons: BTreeMap::new(),
            stalls: 0,
            stall_reasons: BTreeMap::new(),
            stall_units: BTreeMap::new(),
        }
    }

    fn observe(&mut self, event: &Event) -> Option<StallSample> {
        match event {
            Event::CommandRejected { reason, .. } => {
                self.rejections = self.rejections.saturating_add(1);
                increment(&mut self.rejection_reasons, wire_name(reason));
                None
            }
            Event::OrderStalled { unit, reason, .. } => {
                self.stalls = self.stalls.saturating_add(1);
                let reason = wire_name(reason);
                increment(&mut self.stall_reasons, reason.clone());
                let per_unit = self.stall_units.entry(unit.0).or_default();
                increment(per_unit, reason.clone());
                Some(StallSample {
                    seat: self.seat,
                    unit: unit.0,
                    count: per_unit.get(&reason).copied().unwrap_or(0),
                    reason,
                })
            }
            _ => None,
        }
    }
}

/// One self-contained JSONL evaluation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationRow {
    /// Simulation version that produced the record.
    pub sim_version: &'static str,
    /// User-supplied candidate or build identifier.
    pub candidate: String,
    /// Scenario display name.
    pub scenario: String,
    /// Stable digest of the complete configured scenario.
    pub scenario_fingerprint: String,
    /// Stable digest of the scenario, transforms, controller identities, and
    /// exact player-facing configuration for this leg.
    pub evaluation_fingerprint: String,
    /// Stable digest of only the transformed scenario and exact controllers.
    /// Axis labels and leg names are excluded so aliased matrix cells can be
    /// detected before execution.
    pub execution_fingerprint: String,
    /// Exact scenario seed.
    pub scenario_seed: u64,
    /// Requested simulation tick ceiling.
    pub tick_limit: u64,
    /// Stalls of one reason on one unit that end the leg early; `None` when
    /// the loop check was disabled.
    pub stall_loop_limit: Option<u64>,
    /// Seat-pair leg.
    pub leg: EvaluationLeg,
    /// Map-end transform applied to the source scenario.
    pub geometry: EvaluationGeometry,
    /// Faction assignment applied to the physical seats.
    pub faction_cell: EvaluationFactionCell,
    /// Exact player-facing controller configuration by seat.
    pub seats: Vec<SeatConfiguration>,
    /// Whether the match decided before the ceiling.
    pub termination: Termination,
    /// The order loop that stopped the leg, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall_loop: Option<StallLoop>,
    /// Final game result, absent at the tick ceiling.
    pub result: Option<GameResult>,
    /// Seats on the surviving team, empty for draws and undecided matches.
    pub winner_seats: Vec<u8>,
    /// Simulation ticks executed.
    pub duration_ticks: u64,
    /// Canonical final state hash.
    pub final_hash: String,
    /// Seed-independent digest of the exact tick-stamped command stream.
    pub command_hash: String,
    /// Per-seat command failure evidence.
    pub evidence: Vec<SeatEvidence>,
    /// Saved replay path, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<String>,
}

/// Schema version for [`EvaluationTraceRow`].
pub const EVALUATION_TRACE_ROW_VERSION: u32 = 1;

/// One opt-in decision trace joined to its exact evaluation leg.
///
/// These rows are diagnostic sidecar evidence. They are not replay input and
/// never become part of [`EvaluationRow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationTraceRow {
    /// Sidecar row schema version.
    pub version: u32,
    /// User-supplied candidate or build identifier.
    pub candidate: String,
    /// Stable digest of the exact evaluation plan.
    pub evaluation_fingerprint: String,
    /// Seat-pair leg.
    pub leg: EvaluationLeg,
    /// Player seat whose fog-honest decision produced the trace.
    pub seat: u8,
    /// Simulation tick observed by the controller.
    pub tick: u64,
    /// Player-facing decision diagnostic.
    pub trace: DecisionTrace,
}

type EvaluationTraceSink<'a> = &'a mut dyn FnMut(&EvaluationTraceRow) -> Result<()>;

/// Runs one configured scenario until its result or `tick_limit`.
///
/// A replay is always assembled in memory so command counts and optional
/// evidence use the exact recorded input stream. It is written only when
/// `replay_path` is supplied.
pub fn evaluate(
    scenario: &Scenario,
    tick_limit: u64,
    leg: EvaluationLeg,
    candidate: &str,
    replay_path: Option<&Path>,
) -> Result<EvaluationRow> {
    let (mut row, replay) = evaluate_artifact(scenario, tick_limit, leg, candidate)?;
    if let Some(path) = replay_path {
        let mut batch = EvidenceBatch::default();
        batch.stage_replay(&replay, path)?;
        batch.publish()?;
        row.replay = Some(path.display().to_string());
    }
    Ok(row)
}

/// Runs one configured scenario and returns its row plus unpublished replay.
///
/// Callers evaluating a batch can stage every returned replay and publish only
/// after every scenario has succeeded.
pub fn evaluate_artifact(
    scenario: &Scenario,
    tick_limit: u64,
    leg: EvaluationLeg,
    candidate: &str,
) -> Result<(EvaluationRow, GameReplay)> {
    let plan = EvaluationPlan::from_scenario(scenario.clone(), leg);
    evaluate_plan_artifact(&plan, tick_limit, candidate)
}

/// Runs one exact evaluation plan and returns its row plus unpublished replay.
///
/// Unlike [`evaluate_artifact`], this entry point can seat the frozen Overseer
/// without pretending it is an ordinary player-facing [`BotConfig`].
pub fn evaluate_plan_artifact(
    plan: &EvaluationPlan,
    tick_limit: u64,
    candidate: &str,
) -> Result<(EvaluationRow, GameReplay)> {
    evaluate_plan_artifact_with(plan, tick_limit, Some(DEFAULT_STALL_LOOP_LIMIT), candidate)
}

/// [`evaluate_plan_artifact`] with an explicit stall-loop limit; `None`
/// disables the early stop so a leg always runs to its result or ceiling.
pub fn evaluate_plan_artifact_with(
    plan: &EvaluationPlan,
    tick_limit: u64,
    stall_loop_limit: Option<u64>,
    candidate: &str,
) -> Result<(EvaluationRow, GameReplay)> {
    let (row, replay, _) =
        evaluate_plan_artifact_impl(plan, tick_limit, stall_loop_limit, candidate, None)?;
    Ok((row, replay))
}

/// Runs one exact evaluation plan and also captures player-facing decision
/// diagnostics produced during that run.
///
/// The returned replay and compact evaluation row are identical to those from
/// [`evaluate_plan_artifact_with`]. The frozen Overseer produces no trace rows.
pub fn evaluate_plan_artifact_traced_with<F>(
    plan: &EvaluationPlan,
    tick_limit: u64,
    stall_loop_limit: Option<u64>,
    candidate: &str,
    mut on_trace: F,
) -> Result<(EvaluationRow, GameReplay, u64)>
where
    F: FnMut(&EvaluationTraceRow) -> Result<()>,
{
    evaluate_plan_artifact_impl(
        plan,
        tick_limit,
        stall_loop_limit,
        candidate,
        Some(&mut on_trace),
    )
}

fn evaluate_plan_artifact_impl(
    plan: &EvaluationPlan,
    tick_limit: u64,
    stall_loop_limit: Option<u64>,
    candidate: &str,
    mut trace_sink: Option<EvaluationTraceSink<'_>>,
) -> Result<(EvaluationRow, GameReplay, u64)> {
    ensure!(tick_limit > 0, "bot evaluation tick limit must be positive");
    ensure!(
        stall_loop_limit != Some(0),
        "bot evaluation stall-loop limit must be positive or disabled"
    );
    validate_candidate(candidate)?;
    plan.validate()?;
    let scenario = &plan.scenario;
    let scenario_fingerprint = scenario_fingerprint(scenario)?;
    let evaluation_fingerprint = evaluation_fingerprint(plan)?;
    let execution_fingerprint = execution_fingerprint(plan)?;

    let mut state = scenario
        .build()
        .context("building bot evaluation scenario")?;
    let mut bots = plan.seat_bots()?;
    let mut replay = GameReplay::new(SIM_VERSION, scenario.clone());
    replay.meta.kind = Some("bot-eval".into());
    let controllers = serde_json::to_string(&plan.controllers)
        .context("serializing replay controller provenance")?;
    replay.meta.description = Some(format!(
        "bot-eval candidate={candidate}; scenario={scenario_fingerprint}; evaluation={evaluation_fingerprint}; execution={execution_fingerprint}; tick_limit={tick_limit}; leg={}; geometry={:?}; faction_cell={:?}; controllers={controllers}",
        plan.leg.name(),
        plan.geometry,
        plan.faction_cell,
    ));
    let mut evidence: Vec<SeatEvidence> =
        (0..scenario.players.len()).map(SeatEvidence::new).collect();
    let mut trace_count = 0_u64;

    let mut stall_loop = None;
    'run: while state.current_tick() < tick_limit && state.result().is_none() {
        let report = if let Some(on_trace) = trace_sink.as_mut() {
            let traced = oxide_kit::runner::step_traced(&mut state, &mut bots, Some(&mut replay));
            for trace in traced.traces {
                let row = EvaluationTraceRow {
                    version: EVALUATION_TRACE_ROW_VERSION,
                    candidate: candidate.to_string(),
                    evaluation_fingerprint: evaluation_fingerprint.clone(),
                    leg: plan.leg,
                    seat: trace.player.0,
                    tick: trace.tick,
                    trace,
                };
                on_trace(&row)?;
                trace_count = trace_count.saturating_add(1);
            }
            traced.report
        } else {
            oxide_kit::runner::step(&mut state, &mut bots, Some(&mut replay))
        };
        for event in &report.events {
            if let Some(sample) = record_evidence_event(&mut evidence, event)
                && stall_loop_limit.is_some_and(|limit| sample.count >= limit)
            {
                stall_loop = Some(StallLoop {
                    seat: sample.seat,
                    unit: sample.unit,
                    reason: sample.reason,
                    count: sample.count,
                    tick: state.current_tick(),
                });
                break 'run;
            }
        }
    }

    replay.meta.ticks = Some(state.current_tick());
    for command in &replay.commands {
        if let Some(row) = evidence.get_mut(usize::from(command.command.player.0)) {
            row.commands = row.commands.saturating_add(1);
        }
    }
    let command_hash = command_hash(&replay)?;

    let row = EvaluationRow {
        sim_version: SIM_VERSION,
        candidate: candidate.to_string(),
        scenario: scenario.name.clone(),
        scenario_fingerprint,
        evaluation_fingerprint,
        execution_fingerprint,
        scenario_seed: scenario.seed,
        tick_limit,
        stall_loop_limit,
        leg: plan.leg,
        geometry: plan.geometry,
        faction_cell: plan.faction_cell,
        seats: scenario
            .players
            .iter()
            .enumerate()
            .map(|(seat, player)| SeatConfiguration {
                seat: seat as u8,
                controller: plan.controllers[seat]
                    .map(EvaluationController::kind)
                    .unwrap_or(EvaluationControllerKind::None),
                faction: player.faction,
                config: plan.controllers[seat].and_then(EvaluationController::config),
                overseer_policy_seed: plan.controllers[seat]
                    .and_then(EvaluationController::overseer_policy_seed),
                profile: plan.controllers[seat]
                    .and_then(EvaluationController::config)
                    .map(BotConfig::resolve_profile),
            })
            .collect(),
        termination: if stall_loop.is_some() {
            Termination::StallLoop
        } else if state.result().is_some() {
            Termination::Decided
        } else {
            Termination::TickLimit
        },
        stall_loop,
        result: state.result(),
        winner_seats: state.winners().into_iter().map(|seat| seat.0).collect(),
        duration_ticks: state.current_tick(),
        final_hash: oxide_protocol::hash_hex(state.hash()),
        command_hash,
        evidence,
        replay: None,
    };
    Ok((row, replay, trace_count))
}

fn record_evidence_event(evidence: &mut [SeatEvidence], event: &Event) -> Option<StallSample> {
    let player = match event {
        Event::CommandRejected { player, .. } | Event::OrderStalled { player, .. } => Some(*player),
        _ => None,
    };
    let PlayerId(seat) = player?;
    evidence.get_mut(usize::from(seat))?.observe(event)
}

/// Builds the single or paired all-bot legs for one exact seed cell.
///
/// A paired cell is defined only for two-player scenarios. The second leg
/// exchanges the two complete controller configurations while preserving
/// map geometry, factions, teams, starting rosters, and simulation seed.
pub fn configured_legs(
    source: &Scenario,
    scenario_seed: u64,
    difficulty: BotDifficulty,
    stance: BotStance,
    personality_seed_base: u64,
    paired: bool,
) -> Result<Vec<(EvaluationLeg, Scenario)>> {
    configured_matchup_legs(
        source,
        scenario_seed,
        ProfileMatchup::uniform(difficulty, stance),
        personality_seed_base,
        paired,
    )
}

/// Builds evaluation legs with optional seat-one controller overrides.
///
/// When `paired` is true, the return leg exchanges the complete serialized
/// controller configs. This moves difficulty, stance, personality seed, and
/// therefore the resolved profile together while all non-controller scenario
/// state remains fixed.
pub fn configured_matchup_legs(
    source: &Scenario,
    scenario_seed: u64,
    matchup: ProfileMatchup,
    personality_seed_base: u64,
    paired: bool,
) -> Result<Vec<(EvaluationLeg, Scenario)>> {
    if matchup.requires_two_seats(paired) {
        ensure!(
            source.players.len() == 2,
            "paired, opponent-specific, and shared-personality bot evaluations require exactly two seats, got {}",
            source.players.len()
        );
    }

    let mut forward = source.clone();
    forward.seed = scenario_seed;
    for (seat, player) in forward.players.iter_mut().enumerate() {
        let offset = u64::try_from(seat).expect("seat count fits u64");
        let personality_seed = if matchup.same_personality_seed {
            personality_seed_base
        } else {
            personality_seed_base
                .checked_add(offset)
                .context("personality seed range overflows u64")?
        };
        player.bot = true;
        player.bot_config = Some(matchup.config_for_seat(seat, personality_seed));
    }

    if !paired {
        return Ok(vec![(EvaluationLeg::Single, forward)]);
    }

    let mut swapped = forward.clone();
    let first = swapped.players[0].bot_config;
    swapped.players[0].bot_config = swapped.players[1].bot_config;
    swapped.players[1].bot_config = first;
    Ok(vec![
        (EvaluationLeg::Forward, forward),
        (EvaluationLeg::Swapped, swapped),
    ])
}

/// Builds a controlled player-facing-bot versus Overseer cell.
///
/// The transformed scenario is identical in both paired legs. Only the two
/// evaluation-only command sources exchange seats. A full crossed matrix is
/// still required to separate controller performance from faction and map-end
/// effects.
pub fn configured_overseer_plans(
    source: &Scenario,
    scenario_seed: u64,
    config: BotConfig,
    overseer_policy_seed: u64,
    paired: bool,
    faction_cell: EvaluationFactionCell,
    geometry: EvaluationGeometry,
) -> Result<Vec<EvaluationPlan>> {
    ensure!(
        source.players.len() == 2,
        "scripted-versus-Overseer evaluations require exactly two seats, got {}",
        source.players.len()
    );
    ensure!(
        source.players[0].team.is_none() || source.players[0].team != source.players[1].team,
        "scripted-versus-Overseer evaluations require two opposing teams"
    );

    let mut scenario = geometry.apply(source)?;
    scenario.seed = scenario_seed;
    faction_cell.apply(&mut scenario)?;
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }

    let scripted = EvaluationController::Scripted { config };
    let overseer = EvaluationController::Overseer {
        policy_seed: overseer_policy_seed,
    };
    let forward = EvaluationPlan {
        leg: if paired {
            EvaluationLeg::Forward
        } else {
            EvaluationLeg::Single
        },
        scenario: scenario.clone(),
        controllers: vec![Some(scripted), Some(overseer)],
        geometry,
        faction_cell,
    };
    if !paired {
        return Ok(vec![forward]);
    }
    let swapped = EvaluationPlan {
        leg: EvaluationLeg::Swapped,
        scenario,
        controllers: vec![Some(overseer), Some(scripted)],
        geometry,
        faction_cell,
    };
    Ok(vec![forward, swapped])
}

/// Atomically writes compact evaluation records as one JSON object per line.
pub fn write_jsonl(rows: &[EvaluationRow], path: &Path) -> Result<()> {
    write_serialized_jsonl(rows, path, "bot evaluation JSONL")
}

fn write_serialized_jsonl<T: Serialize>(rows: &[T], path: &Path, label: &str) -> Result<()> {
    chassis::fsx::write_atomic(path, |writer| -> Result<()> {
        for row in rows {
            serde_json::to_writer(&mut *writer, row)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    })
    .with_context(|| format!("writing {label} to {}", path.display()))
}

/// A bounded, unpublished decision-trace JSONL stream.
///
/// Call [`finish`](Self::finish) before publishing its [`EvidenceBatch`]. A
/// dropped or failed stream remains private and the batch removes it.
pub struct EvaluationTraceWriter {
    writer: Option<BufWriter<std::fs::File>>,
    staged: PathBuf,
    destination: PathBuf,
    ready: Arc<AtomicBool>,
    rows: u64,
}

impl EvaluationTraceWriter {
    /// Appends one deterministic trace row to the private staging file.
    pub fn write_row(&mut self, row: &EvaluationTraceRow) -> Result<()> {
        let writer = self
            .writer
            .as_mut()
            .expect("a live trace writer retains its staging file");
        serde_json::to_writer(&mut *writer, row).with_context(|| {
            format!(
                "serializing bot decision trace for {}",
                self.destination.display()
            )
        })?;
        writer.write_all(b"\n").with_context(|| {
            format!(
                "writing bot decision trace for {}",
                self.destination.display()
            )
        })?;
        self.rows = self.rows.saturating_add(1);
        Ok(())
    }

    /// Flushes and syncs the private trace file, returning its row count.
    pub fn finish(mut self) -> Result<u64> {
        let mut writer = self
            .writer
            .take()
            .expect("a live trace writer retains its staging file");
        writer.flush().with_context(|| {
            format!(
                "flushing bot decision trace for {}",
                self.destination.display()
            )
        })?;
        writer.get_ref().sync_all().with_context(|| {
            format!(
                "syncing bot decision trace for {}",
                self.destination.display()
            )
        })?;
        drop(writer);
        self.ready.store(true, Ordering::Release);
        Ok(self.rows)
    }
}

impl Drop for EvaluationTraceWriter {
    fn drop(&mut self) {
        if !self.ready.load(Ordering::Acquire) {
            drop(self.writer.take());
            std::fs::remove_file(&self.staged).ok();
        }
    }
}

/// Unpublished evidence files for one evaluation invocation.
///
/// Each payload is first written to a unique sibling path. [`publish`](Self::publish)
/// then creates every destination without replacing an existing file. If any
/// publication returns an error, files linked earlier in that attempt are
/// rolled back while the batch unwinds. Abrupt process termination is outside
/// that contract: a filesystem cannot atomically reveal an arbitrary set of
/// final paths, so a killed process can leave staging files or a partial final
/// set for manual inspection.
#[derive(Debug, Default)]
pub struct EvidenceBatch {
    staged: Vec<(PathBuf, PathBuf)>,
    trace_ready: Vec<Arc<AtomicBool>>,
    published: Vec<PathBuf>,
    committed: bool,
}

impl EvidenceBatch {
    /// Stages a replay next to its eventual destination without publishing it.
    pub fn stage_replay(&mut self, replay: &GameReplay, destination: &Path) -> Result<()> {
        let staged = self.reserve_stage(destination)?;
        replay.save(&staged).with_context(|| {
            format!(
                "staging bot evaluation replay for {}",
                destination.display()
            )
        })
    }

    /// Stages the JSONL index next to its eventual destination.
    pub fn stage_jsonl(&mut self, rows: &[EvaluationRow], destination: &Path) -> Result<()> {
        let staged = self.reserve_stage(destination)?;
        write_jsonl(rows, &staged)
            .with_context(|| format!("staging evidence index for {}", destination.display()))
    }

    /// Opens a bounded decision-trace JSONL stream at a private sibling path.
    pub fn stage_trace_jsonl(&mut self, destination: &Path) -> Result<EvaluationTraceWriter> {
        let staged = self.reserve_stage(destination)?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&staged)
            .with_context(|| {
                format!(
                    "opening bot decision trace staging file for {}",
                    destination.display()
                )
            })?;
        let ready = Arc::new(AtomicBool::new(false));
        self.trace_ready.push(Arc::clone(&ready));
        Ok(EvaluationTraceWriter {
            writer: Some(BufWriter::new(file)),
            staged,
            destination: destination.to_path_buf(),
            ready,
            rows: 0,
        })
    }

    /// Publishes every staged file as a create-only hard link.
    ///
    /// Because each staging path is a sibling of its destination, the link
    /// stays on one filesystem and cannot overwrite an earlier evidence file.
    /// Errors returned from this call roll back links already created by this
    /// batch; abrupt process termination can interrupt that rollback.
    pub fn publish(mut self) -> Result<()> {
        ensure!(
            self.trace_ready
                .iter()
                .all(|ready| ready.load(Ordering::Acquire)),
            "bot evaluation decision traces must be finished before publication"
        );
        let mut destinations: Vec<&Path> = self
            .staged
            .iter()
            .map(|(_, destination)| destination.as_path())
            .collect();
        destinations.sort_unstable();
        ensure!(
            destinations.windows(2).all(|pair| pair[0] != pair[1]),
            "bot evaluation evidence destinations must be unique"
        );

        for index in 0..self.staged.len() {
            let (staged, destination) = &self.staged[index];
            std::fs::hard_link(staged, destination).with_context(|| {
                format!(
                    "publishing bot evaluation evidence to {} without overwriting",
                    destination.display()
                )
            })?;
            self.published.push(destination.clone());
        }
        sync_parents(self.staged.iter().map(|(_, destination)| destination))?;
        for (staged, _) in &self.staged {
            std::fs::remove_file(staged).with_context(|| {
                format!("removing bot evaluation staging file {}", staged.display())
            })?;
        }
        sync_parents(self.staged.iter().map(|(_, destination)| destination))?;
        self.committed = true;
        Ok(())
    }

    fn reserve_stage(&mut self, destination: &Path) -> Result<PathBuf> {
        ensure!(
            !self
                .staged
                .iter()
                .any(|(_, existing)| existing == destination),
            "duplicate bot evaluation evidence destination {}",
            destination.display()
        );
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "creating bot evaluation evidence directory {}",
                parent.display()
            )
        })?;

        static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("evidence");
        let staged = loop {
            let nonce = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".{name}.bot-eval-stage.{}.{nonce}",
                std::process::id()
            ));
            match std::fs::File::create_new(&candidate) {
                Ok(_) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("reserving staging file {}", candidate.display())
                    });
                }
            }
        };
        self.staged
            .push((staged.clone(), destination.to_path_buf()));
        Ok(staged)
    }
}

impl Drop for EvidenceBatch {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.published {
                std::fs::remove_file(path).ok();
            }
        }
        for (staged, _) in &self.staged {
            std::fs::remove_file(staged).ok();
        }
    }
}

/// Refuses duplicate or already-present evidence destinations before any
/// match is evaluated. Publication repeats the create-only check atomically.
pub fn preflight_destinations(paths: &[PathBuf]) -> Result<()> {
    let mut ordered: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    ordered.sort_unstable();
    ensure!(
        ordered.windows(2).all(|pair| pair[0] != pair[1]),
        "bot evaluation evidence destinations must be unique"
    );
    for path in ordered {
        match std::fs::symlink_metadata(path) {
            Ok(_) => anyhow::bail!(
                "bot evaluation evidence already exists at {}; refusing to overwrite it",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("checking bot evaluation destination {}", path.display())
                });
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parents<'a>(destinations: impl Iterator<Item = &'a PathBuf>) -> Result<()> {
    let mut parents: Vec<&Path> = destinations
        .map(|path| {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
        })
        .collect();
    parents.sort_unstable();
    parents.dedup();
    for parent in parents {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("syncing evidence directory {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parents<'a>(_destinations: impl Iterator<Item = &'a PathBuf>) -> Result<()> {
    Ok(())
}

/// Stable digest of every serialized scenario field used to build a match.
pub fn scenario_fingerprint(scenario: &Scenario) -> Result<String> {
    let configured = serde_json::to_vec(scenario).context("serializing scenario provenance")?;
    Ok(format!(
        "fnv1a64:{:016x}",
        chassis::hash::fnv1a(&configured)
    ))
}

/// Stable digest of the transformed matchup, including evaluation-only
/// command sources that are intentionally absent from an ordinary scenario.
pub fn evaluation_fingerprint(plan: &EvaluationPlan) -> Result<String> {
    plan.validate()?;
    let configured = serde_json::to_vec(plan).context("serializing evaluation provenance")?;
    Ok(format!(
        "fnv1a64:{:016x}",
        chassis::hash::fnv1a(&configured)
    ))
}

/// Stable digest of the exact world and controller identities that execute.
///
/// Unlike [`evaluation_fingerprint`], this deliberately excludes matrix axis
/// labels and the leg name. Two nominal cells with this same digest would run
/// the same match and must not be counted as independent evidence.
pub fn execution_fingerprint(plan: &EvaluationPlan) -> Result<String> {
    let configured = execution_identity_bytes(plan)?;
    Ok(format!(
        "fnv1a64:{:016x}",
        chassis::hash::fnv1a(&configured)
    ))
}

/// Rejects nominal matrix cells that resolve to the same executable matchup.
pub fn ensure_unique_execution_plans<'a>(
    plans: impl IntoIterator<Item = &'a EvaluationPlan>,
) -> Result<()> {
    let mut seen = BTreeMap::new();
    for plan in plans {
        let identity = execution_identity_bytes(plan)?;
        let label = format!(
            "scenario {:?} seed {} {:?}/{:?}/{}",
            plan.scenario.name,
            plan.scenario.seed,
            plan.geometry,
            plan.faction_cell,
            plan.leg.name()
        );
        if let Some(first) = seen.insert(identity, label.clone()) {
            anyhow::bail!(
                "bot evaluation matrix contains duplicate executable cells: {first} and {label}"
            );
        }
    }
    Ok(())
}

fn execution_identity_bytes(plan: &EvaluationPlan) -> Result<Vec<u8>> {
    plan.validate()?;
    serde_json::to_vec(&(&plan.scenario, &plan.controllers))
        .context("serializing executable evaluation identity")
}

/// Stable digest of the exact tick-stamped commands in one recorded match.
///
/// The replay setup and simulation seed are excluded so independent seed cells
/// that happened to produce the same behavior remain recognizable.
pub fn command_hash(replay: &GameReplay) -> Result<String> {
    let commands =
        serde_json::to_vec(&replay.commands).context("serializing evaluation command stream")?;
    Ok(format!("fnv1a64:{:016x}", chassis::hash::fnv1a(&commands)))
}

fn validate_candidate(candidate: &str) -> Result<()> {
    ensure!(
        !candidate.is_empty(),
        "bot evaluation candidate must not be empty"
    );
    ensure!(
        candidate.len() <= MAX_CANDIDATE_LEN,
        "bot evaluation candidate must be at most {MAX_CANDIDATE_LEN} bytes"
    );
    ensure!(
        candidate.trim() == candidate && !candidate.chars().any(char::is_control),
        "bot evaluation candidate must not contain surrounding whitespace or control characters"
    );
    Ok(())
}

/// Builds a replay filename that cannot alias a different configured matchup.
///
/// The digest covers the candidate, simulation version, tick ceiling, and
/// complete configured scenario, including every seat's difficulty, stance,
/// and personality seed. Repeating the exact cell intentionally chooses the
/// same path so create-only publication refuses to erase its earlier evidence.
pub fn replay_filename(
    scenario_index: usize,
    run: u64,
    scenario_seed: u64,
    tick_limit: u64,
    leg: EvaluationLeg,
    candidate: &str,
    scenario: &Scenario,
) -> Result<String> {
    let plan = EvaluationPlan::from_scenario(scenario.clone(), leg);
    evaluation_replay_filename(
        scenario_index,
        run,
        scenario_seed,
        tick_limit,
        candidate,
        &plan,
    )
}

/// Builds a replay filename for one exact evaluation plan.
pub fn evaluation_replay_filename(
    scenario_index: usize,
    run: u64,
    scenario_seed: u64,
    tick_limit: u64,
    candidate: &str,
    plan: &EvaluationPlan,
) -> Result<String> {
    validate_candidate(candidate)?;
    plan.validate()?;
    ensure!(
        scenario_seed == plan.scenario.seed,
        "replay filename seed {scenario_seed} does not match evaluation scenario seed {}",
        plan.scenario.seed
    );
    let configured = serde_json::to_vec(&(candidate, SIM_VERSION, tick_limit, plan))
        .context("serializing replay filename input")?;
    let digest = chassis::hash::fnv1a(&configured);
    Ok(format!(
        "{scenario_index:03}-{run:03}-s{scenario_seed}-{}-c{digest:016x}.json",
        plan.leg.name()
    ))
}

fn wire_name(value: &impl Serialize) -> String {
    match serde_json::to_value(value).expect("simulation enums serialize") {
        serde_json::Value::String(name) => name,
        value => value.to_string(),
    }
}

fn increment(counts: &mut BTreeMap<String, u64>, key: String) {
    *counts.entry(key).or_default() += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::command::{Command, PlayerCommand, RejectReason};
    use oxide_sim::scenario::{PlayerSpec, UnitSpec};
    use oxide_sim::{Faction, StallReason, UnitKind};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_REPLAY_ID: AtomicU64 = AtomicU64::new(0);
    const TEST_OVERSEER_POLICY_SEED: u64 = 73;

    fn firing_squad() -> Scenario {
        let ground = ".".repeat(16);
        let mut anchored: Vec<char> = ground.chars().collect();
        anchored[1] = '1';
        anchored[11] = '2';
        let mut units = Vec::new();
        for x in [8, 9] {
            for y in [1, 2, 3, 4] {
                units.push(UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x,
                    y,
                });
            }
        }
        Scenario {
            name: "firing-squad".into(),
            seed: 7,
            map: vec![
                ground.clone(),
                ground.clone(),
                anchored.into_iter().collect(),
                ground.clone(),
                ground.clone(),
                ground,
            ],
            players: vec![
                PlayerSpec {
                    name: "attacker".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 100,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "victim".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 100,
                    bot: false,
                    bot_config: None,
                },
            ],
            units,
            buildings: Vec::new(),
            meta: None,
        }
    }

    fn prime_config() -> BotConfig {
        BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 8_100)
    }

    fn overseer_controller() -> EvaluationController {
        EvaluationController::Overseer {
            policy_seed: TEST_OVERSEER_POLICY_SEED,
        }
    }

    fn one_evaluation_trace() -> EvaluationTraceRow {
        let plan = configured_overseer_plans(
            &Scenario::skirmish(),
            73,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Authored,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);
        let mut trace = None;
        evaluate_plan_artifact_traced_with(&plan, 1, None, "candidate-a", |row| {
            assert!(trace.replace(row.clone()).is_none());
            Ok(())
        })
        .unwrap();
        trace.expect("Prime emits one trace at tick zero")
    }

    #[test]
    fn overseer_pair_swaps_only_controllers_on_one_transformed_scenario() {
        let source = Scenario::skirmish();
        let plans = configured_overseer_plans(
            &source,
            91,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            true,
            EvaluationFactionCell::Cf,
            EvaluationGeometry::Rot180,
        )
        .unwrap();

        assert_eq!(plans.len(), 2);
        let (forward, swapped) = (&plans[0], &plans[1]);
        assert_eq!(forward.leg, EvaluationLeg::Forward);
        assert_eq!(swapped.leg, EvaluationLeg::Swapped);
        assert_eq!(forward.geometry, EvaluationGeometry::Rot180);
        assert_eq!(swapped.geometry, EvaluationGeometry::Rot180);
        assert_eq!(forward.faction_cell, EvaluationFactionCell::Cf);
        assert_eq!(swapped.faction_cell, EvaluationFactionCell::Cf);
        assert_eq!(forward.scenario, swapped.scenario);
        assert_eq!(forward.scenario.seed, 91);
        assert_eq!(
            forward
                .scenario
                .players
                .iter()
                .map(|player| player.faction)
                .collect::<Vec<_>>(),
            [Faction::Cupric, Faction::Ferrous]
        );
        assert!(
            forward
                .scenario
                .players
                .iter()
                .all(|player| !player.bot && player.bot_config.is_none())
        );
        assert_eq!(
            forward.controllers,
            [
                Some(EvaluationController::Scripted {
                    config: prime_config()
                }),
                Some(overseer_controller()),
            ]
        );
        assert_eq!(
            swapped.controllers,
            [
                Some(overseer_controller()),
                Some(EvaluationController::Scripted {
                    config: prime_config()
                }),
            ]
        );
        assert_eq!(source, Scenario::skirmish(), "planning mutated the source");
    }

    #[test]
    fn controlled_faction_cells_retint_players_and_starting_rosters() {
        let source = Scenario::skirmish();
        let fc = configured_overseer_plans(
            &source,
            source.seed,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Fc,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);
        let cf = configured_overseer_plans(
            &source,
            source.seed,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Cf,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);

        assert_eq!(
            fc.scenario
                .players
                .iter()
                .map(|player| player.faction)
                .collect::<Vec<_>>(),
            [Faction::Ferrous, Faction::Cupric]
        );
        assert_eq!(fc.scenario.units, source.units);
        assert_eq!(
            cf.scenario
                .players
                .iter()
                .map(|player| player.faction)
                .collect::<Vec<_>>(),
            [Faction::Cupric, Faction::Ferrous]
        );
        for (actual, authored) in cf.scenario.units.iter().zip(&source.units) {
            let faction = cf.scenario.players[usize::from(actual.player)].faction;
            assert_eq!(actual.player, authored.player);
            assert_eq!((actual.x, actual.y), (authored.x, authored.y));
            assert_eq!(actual.kind, authored.kind.role().unit_for(faction));
        }
    }

    #[test]
    fn controlled_geometry_records_and_applies_the_exact_half_turn() {
        let source = Scenario::skirmish();
        let authored = configured_overseer_plans(
            &source,
            31,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Cf,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);
        let rotated = configured_overseer_plans(
            &source,
            31,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Cf,
            EvaluationGeometry::Rot180,
        )
        .unwrap()
        .remove(0);

        assert_eq!(authored.geometry, EvaluationGeometry::Authored);
        assert_eq!(rotated.geometry, EvaluationGeometry::Rot180);
        assert_ne!(authored.scenario.units, rotated.scenario.units);
        assert_eq!(
            crate::factorial::rotate_180(&rotated.scenario).unwrap(),
            authored.scenario,
            "the recorded rot180 cell must be the exact involutive transform"
        );
    }

    #[test]
    fn overseer_evaluation_records_exact_controller_and_roster_provenance() {
        let plans = configured_overseer_plans(
            &Scenario::skirmish(),
            73,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            true,
            EvaluationFactionCell::Cf,
            EvaluationGeometry::Authored,
        )
        .unwrap();

        for plan in plans {
            let (row, replay) = evaluate_plan_artifact(&plan, 1, "candidate-a").unwrap();
            let expected = match plan.leg {
                EvaluationLeg::Forward => [
                    (
                        EvaluationControllerKind::Scripted,
                        Faction::Cupric,
                        Some(prime_config()),
                    ),
                    (EvaluationControllerKind::Overseer, Faction::Ferrous, None),
                ],
                EvaluationLeg::Swapped => [
                    (EvaluationControllerKind::Overseer, Faction::Cupric, None),
                    (
                        EvaluationControllerKind::Scripted,
                        Faction::Ferrous,
                        Some(prime_config()),
                    ),
                ],
                EvaluationLeg::Single => panic!("paired plans cannot contain a single leg"),
            };

            assert_eq!(row.leg, plan.leg);
            assert_eq!(row.geometry, EvaluationGeometry::Authored);
            assert_eq!(row.faction_cell, EvaluationFactionCell::Cf);
            assert_eq!(
                row.scenario_fingerprint,
                scenario_fingerprint(&plan.scenario).unwrap()
            );
            assert_eq!(
                row.evaluation_fingerprint,
                evaluation_fingerprint(&plan).unwrap()
            );
            assert_eq!(replay.setup, plan.scenario);
            for (seat_index, (seat, (controller, faction, config))) in
                row.seats.iter().zip(expected).enumerate()
            {
                assert_eq!(seat.seat, seat_index as u8);
                assert_eq!(seat.controller, controller);
                assert_eq!(seat.faction, faction);
                assert_eq!(seat.config, config);
                assert_eq!(
                    seat.overseer_policy_seed,
                    (controller == EvaluationControllerKind::Overseer)
                        .then_some(TEST_OVERSEER_POLICY_SEED)
                );
                assert_eq!(seat.profile, config.map(BotConfig::resolve_profile));
            }
            assert!(
                row.evidence.iter().all(|seat| seat.commands > 0),
                "both explicit evaluation controllers must actually run"
            );
            let description = replay.meta.description.as_deref().unwrap();
            assert!(description.contains(&format!("evaluation={}", row.evaluation_fingerprint)));
            assert!(description.contains("\"kind\":\"scripted\""));
            assert!(description.contains(&format!(
                "\"kind\":\"overseer\",\"policy_seed\":{TEST_OVERSEER_POLICY_SEED}"
            )));
        }
    }

    #[test]
    fn traced_evaluation_is_deterministic_and_does_not_change_authoritative_evidence() {
        let plan = configured_overseer_plans(
            &Scenario::skirmish(),
            73,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Cf,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);

        let (ordinary_row, ordinary_replay) =
            evaluate_plan_artifact_with(&plan, 25, None, "candidate-a").unwrap();
        let mut traces = Vec::new();
        let (traced_row, traced_replay, trace_count) =
            evaluate_plan_artifact_traced_with(&plan, 25, None, "candidate-a", |row| {
                traces.push(row.clone());
                Ok(())
            })
            .unwrap();
        let mut repeated_traces = Vec::new();
        let (_, _, repeated_count) =
            evaluate_plan_artifact_traced_with(&plan, 25, None, "candidate-a", |row| {
                repeated_traces.push(row.clone());
                Ok(())
            })
            .unwrap();

        assert_eq!(traced_row, ordinary_row);
        assert_eq!(traced_row.final_hash, ordinary_row.final_hash);
        assert_eq!(traced_row.command_hash, ordinary_row.command_hash);
        assert_eq!(
            serde_json::to_vec(&traced_replay).unwrap(),
            serde_json::to_vec(&ordinary_replay).unwrap(),
            "trace capture must not enter replay metadata or commands"
        );
        assert!(!traces.is_empty(), "Prime should produce a decision trace");
        assert_eq!(trace_count, traces.len() as u64);
        assert_eq!(repeated_count, repeated_traces.len() as u64);
        assert_eq!(traces, repeated_traces);
        assert_eq!(
            serde_json::to_vec(&traces).unwrap(),
            serde_json::to_vec(&repeated_traces).unwrap()
        );
        for row in &traces {
            assert_eq!(row.version, EVALUATION_TRACE_ROW_VERSION);
            assert_eq!(row.candidate, "candidate-a");
            assert_eq!(
                row.evaluation_fingerprint,
                traced_row.evaluation_fingerprint
            );
            assert_eq!(row.leg, traced_row.leg);
            assert_eq!(row.seat, 0, "the frozen Overseer must not emit traces");
            assert_eq!(row.seat, row.trace.player.0);
            assert_eq!(row.tick, row.trace.tick);
            assert!(row.tick < traced_row.duration_ticks);
        }
    }

    #[test]
    fn nominal_axis_aliases_share_one_execution_identity_and_are_refused() {
        let source = Scenario::skirmish();
        let authored = configured_overseer_plans(
            &source,
            source.seed,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Authored,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);
        let explicit = configured_overseer_plans(
            &source,
            source.seed,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Fc,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);

        assert_eq!(authored.scenario, explicit.scenario);
        assert_eq!(authored.controllers, explicit.controllers);
        assert_ne!(
            evaluation_fingerprint(&authored).unwrap(),
            evaluation_fingerprint(&explicit).unwrap(),
            "the nominal provenance labels remain distinguishable"
        );
        assert_eq!(
            execution_fingerprint(&authored).unwrap(),
            execution_fingerprint(&explicit).unwrap(),
            "execution identity must ignore the aliasing faction label"
        );
        let error = ensure_unique_execution_plans([&authored, &explicit]).unwrap_err();
        assert!(
            error.to_string().contains("duplicate executable cells"),
            "alias refusal should identify the evidence problem: {error:#}"
        );
    }

    #[test]
    fn command_hash_ignores_setup_seed_but_covers_ticks_and_commands() {
        let mut first = GameReplay::new(SIM_VERSION, Scenario::skirmish());
        let mut second_setup = Scenario::skirmish();
        second_setup.seed = second_setup.seed.wrapping_add(1);
        let mut second = GameReplay::new(SIM_VERSION, second_setup);
        let stop = PlayerCommand {
            player: PlayerId(0),
            command: Command::Stop { units: Vec::new() },
        };
        first.record(11, stop.clone());
        second.record(11, stop.clone());

        assert_eq!(
            command_hash(&first).unwrap(),
            command_hash(&second).unwrap()
        );
        second.record(12, stop);
        assert_ne!(
            command_hash(&first).unwrap(),
            command_hash(&second).unwrap()
        );
    }

    #[test]
    fn controller_swaps_have_one_scenario_but_distinct_evidence_identities() {
        let plans = configured_overseer_plans(
            &Scenario::skirmish(),
            73,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            true,
            EvaluationFactionCell::Fc,
            EvaluationGeometry::Rot180,
        )
        .unwrap();
        let (forward, swapped) = (&plans[0], &plans[1]);

        assert_eq!(forward.scenario, swapped.scenario);
        assert_eq!(
            scenario_fingerprint(&forward.scenario).unwrap(),
            scenario_fingerprint(&swapped.scenario).unwrap()
        );
        assert_ne!(
            evaluation_fingerprint(forward).unwrap(),
            evaluation_fingerprint(swapped).unwrap()
        );
        let forward_name =
            evaluation_replay_filename(0, 0, 73, 60_000, "candidate-a", forward).unwrap();
        let swapped_name =
            evaluation_replay_filename(0, 0, 73, 60_000, "candidate-a", swapped).unwrap();
        assert_ne!(forward_name, swapped_name);
        assert!(forward_name.contains("-forward-"));
        assert!(swapped_name.contains("-swapped-"));
    }

    #[test]
    fn evaluation_plan_refuses_missing_or_excess_controller_slots() {
        let scenario = firing_squad();
        for controllers in [
            vec![Some(overseer_controller())],
            vec![
                Some(overseer_controller()),
                Some(overseer_controller()),
                Some(overseer_controller()),
            ],
        ] {
            let plan = EvaluationPlan {
                leg: EvaluationLeg::Single,
                scenario: scenario.clone(),
                controllers,
                geometry: EvaluationGeometry::Authored,
                faction_cell: EvaluationFactionCell::Authored,
            };

            let fingerprint_error = evaluation_fingerprint(&plan).unwrap_err();
            assert!(
                fingerprint_error
                    .to_string()
                    .contains("controllers for 2 seats"),
                "invalid provenance should fail clearly: {fingerprint_error:#}"
            );
            let evaluation_error = evaluate_plan_artifact(&plan, 1, "candidate-a").unwrap_err();
            assert!(
                evaluation_error
                    .to_string()
                    .contains("controllers for 2 seats"),
                "invalid execution should fail before building: {evaluation_error:#}"
            );
        }
    }

    #[test]
    fn overseer_plans_refuse_non_duel_scenarios_before_transforming_them() {
        let mut too_few = firing_squad();
        too_few.players.pop();
        let mut too_many = firing_squad();
        too_many.players.push(too_many.players[0].clone());

        for scenario in [&too_few, &too_many] {
            let error = configured_overseer_plans(
                scenario,
                1,
                prime_config(),
                TEST_OVERSEER_POLICY_SEED,
                true,
                EvaluationFactionCell::Fc,
                EvaluationGeometry::Authored,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("exactly two seats"),
                "non-duel refusal should name the controller shape: {error:#}"
            );
        }
    }

    #[test]
    fn overseer_plans_refuse_two_seats_on_the_same_team() {
        let mut allied = firing_squad();
        allied.players[0].team = Some(4);
        allied.players[1].team = Some(4);

        let error = configured_overseer_plans(
            &allied,
            1,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            true,
            EvaluationFactionCell::Fc,
            EvaluationGeometry::Authored,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("two opposing teams"),
            "allied-seat refusal should explain the competitive invariant: {error:#}"
        );
    }

    #[test]
    fn replay_filename_refuses_a_seed_outside_its_plan() {
        let plan = configured_overseer_plans(
            &Scenario::skirmish(),
            73,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Fc,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);

        let error = evaluation_replay_filename(0, 0, 74, 60_000, "candidate-a", &plan).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match evaluation scenario seed 73"),
            "filename provenance mismatch should fail clearly: {error:#}"
        );
    }

    #[test]
    fn evaluation_stops_on_the_decision_instead_of_padding_frozen_ticks() {
        let row = evaluate(
            &firing_squad(),
            3_000,
            EvaluationLeg::Single,
            "test-build",
            None,
        )
        .unwrap();
        assert_eq!(row.termination, Termination::Decided);
        assert_eq!(row.result, Some(GameResult::Victory { team: 0 }));
        assert_eq!(row.winner_seats, [0]);
        assert!(row.duration_ticks < 3_000, "decision was not an early stop");
        assert_eq!(
            row.evidence.iter().map(|seat| seat.commands).sum::<u64>(),
            0
        );
    }

    #[test]
    fn identical_evaluations_produce_identical_rows() {
        let scenario = firing_squad();
        let a = evaluate(&scenario, 3_000, EvaluationLeg::Single, "test-build", None).unwrap();
        let b = evaluate(&scenario, 3_000, EvaluationLeg::Single, "test-build", None).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.candidate, "test-build");
        assert_eq!(a.tick_limit, 3_000);
        assert_eq!(
            a.scenario_fingerprint,
            scenario_fingerprint(&scenario).unwrap()
        );
    }

    #[test]
    fn saved_replay_preserves_the_exact_compared_configs() {
        let matchup = ProfileMatchup {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Aggressive,
            opponent_difficulty: Some(BotDifficulty::Veteran),
            opponent_stance: Some(BotStance::Turtle),
            same_personality_seed: true,
        };
        let scenario = configured_matchup_legs(&firing_squad(), 91, matchup, 400, false)
            .unwrap()
            .remove(0)
            .1;
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let replay_path = std::env::temp_dir().join(format!(
            "oxide-bot-eval-{}-{replay_id}.json",
            std::process::id()
        ));

        let row = evaluate(
            &scenario,
            1,
            EvaluationLeg::Single,
            "candidate-a",
            Some(&replay_path),
        )
        .unwrap();
        let replay = oxide_kit::load_replay(&replay_path).unwrap();
        std::fs::remove_file(&replay_path).unwrap();

        assert_eq!(row.replay.as_deref(), replay_path.to_str());
        assert_eq!(replay.setup.seed, 91);
        assert_eq!(replay.setup.players, scenario.players);
        let description = replay.meta.description.as_deref().unwrap();
        assert!(description.contains("candidate=candidate-a"));
        assert!(description.contains(&format!("scenario={}", row.scenario_fingerprint)));
        assert!(description.contains("tick_limit=1"));
    }

    #[test]
    fn replay_filenames_distinguish_matchups_with_the_same_numeric_seed_cell() {
        let source = firing_squad();
        let prime_scrapheap = configured_matchup_legs(
            &source,
            13,
            ProfileMatchup {
                difficulty: BotDifficulty::Prime,
                stance: BotStance::Balanced,
                opponent_difficulty: Some(BotDifficulty::Scrapheap),
                opponent_stance: None,
                same_personality_seed: true,
            },
            40,
            true,
        )
        .unwrap();
        let veteran_standard = configured_matchup_legs(
            &source,
            13,
            ProfileMatchup {
                difficulty: BotDifficulty::Veteran,
                stance: BotStance::Balanced,
                opponent_difficulty: Some(BotDifficulty::Standard),
                opponent_stance: None,
                same_personality_seed: true,
            },
            40,
            true,
        )
        .unwrap();

        for leg in 0..2 {
            let (first_leg, first) = &prime_scrapheap[leg];
            let (second_leg, second) = &veteran_standard[leg];
            let first_name =
                replay_filename(0, 0, 13, 48_000, *first_leg, "candidate-a", first).unwrap();
            let second_name =
                replay_filename(0, 0, 13, 48_000, *second_leg, "candidate-a", second).unwrap();
            assert_ne!(first_name, second_name);
        }
    }

    #[test]
    fn replay_filenames_distinguish_extended_tick_limits() {
        let scenario = configured_legs(
            &firing_squad(),
            13,
            BotDifficulty::Prime,
            BotStance::Balanced,
            40,
            false,
        )
        .unwrap()
        .remove(0)
        .1;
        let short =
            replay_filename(0, 0, 13, 1, EvaluationLeg::Single, "candidate-a", &scenario).unwrap();
        let extended =
            replay_filename(0, 0, 13, 2, EvaluationLeg::Single, "candidate-a", &scenario).unwrap();
        assert_ne!(short, extended);
    }

    #[test]
    fn replay_filenames_distinguish_candidate_builds() {
        let scenario = firing_squad();
        let first = replay_filename(
            0,
            0,
            scenario.seed,
            1,
            EvaluationLeg::Single,
            "candidate-a",
            &scenario,
        )
        .unwrap();
        let second = replay_filename(
            0,
            0,
            scenario.seed,
            1,
            EvaluationLeg::Single,
            "candidate-b",
            &scenario,
        )
        .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn replay_evidence_never_overwrites_an_existing_file() {
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let replay_path = std::env::temp_dir().join(format!(
            "oxide-bot-eval-existing-{}-{replay_id}.json",
            std::process::id()
        ));
        std::fs::write(&replay_path, b"earlier evidence").unwrap();

        let error = evaluate(
            &firing_squad(),
            1,
            EvaluationLeg::Single,
            "candidate-a",
            Some(&replay_path),
        )
        .unwrap_err();
        assert!(error.to_string().contains("without overwriting"));
        assert_eq!(std::fs::read(&replay_path).unwrap(), b"earlier evidence");
        std::fs::remove_file(replay_path).unwrap();
    }

    #[test]
    fn a_publication_collision_rolls_back_the_whole_staged_set() {
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "oxide-bot-eval-batch-{}-{replay_id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        let (_, replay) =
            evaluate_artifact(&firing_squad(), 1, EvaluationLeg::Single, "candidate-a").unwrap();
        let mut batch = EvidenceBatch::default();
        batch.stage_replay(&replay, &first).unwrap();
        batch.stage_replay(&replay, &second).unwrap();
        std::fs::write(&second, b"racing evidence").unwrap();

        let error = batch.publish().unwrap_err();
        assert!(error.to_string().contains("without overwriting"));
        assert!(!first.exists(), "the earlier publication was rolled back");
        assert_eq!(std::fs::read(&second).unwrap(), b"racing evidence");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_trace_publication_collision_rolls_back_the_compact_index() {
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "oxide-bot-trace-batch-{}-{replay_id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let index_path = dir.join("rows.jsonl");
        let trace_path = dir.join("trace.jsonl");
        let plan = configured_overseer_plans(
            &Scenario::skirmish(),
            73,
            prime_config(),
            TEST_OVERSEER_POLICY_SEED,
            false,
            EvaluationFactionCell::Authored,
            EvaluationGeometry::Authored,
        )
        .unwrap()
        .remove(0);
        let mut traces = Vec::new();
        let (row, _, trace_count) =
            evaluate_plan_artifact_traced_with(&plan, 1, None, "candidate-a", |row| {
                traces.push(row.clone());
                Ok(())
            })
            .unwrap();
        assert!(!traces.is_empty());
        assert_eq!(trace_count, traces.len() as u64);

        let mut batch = EvidenceBatch::default();
        batch
            .stage_jsonl(std::slice::from_ref(&row), &index_path)
            .unwrap();
        let mut writer = batch.stage_trace_jsonl(&trace_path).unwrap();
        for trace in &traces {
            writer.write_row(trace).unwrap();
        }
        assert_eq!(writer.finish().unwrap(), traces.len() as u64);
        std::fs::write(&trace_path, b"racing trace evidence").unwrap();

        let error = batch.publish().unwrap_err();
        assert!(error.to_string().contains("without overwriting"));
        assert!(!index_path.exists(), "the earlier index was rolled back");
        assert_eq!(
            std::fs::read(&trace_path).unwrap(),
            b"racing trace evidence"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_unfinished_trace_stream_cannot_be_published() {
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "oxide-bot-trace-unfinished-{}-{replay_id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let trace_path = dir.join("trace.jsonl");
        let mut batch = EvidenceBatch::default();
        let mut writer = batch.stage_trace_jsonl(&trace_path).unwrap();
        writer.write_row(&one_evaluation_trace()).unwrap();

        let error = batch.publish().unwrap_err();
        assert!(error.to_string().contains("must be finished"));
        assert!(!trace_path.exists(), "unfinished output was not published");
        drop(writer);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "closing the unfinished writer removes its private staging file"
        );
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn an_unfinished_trace_writer_cleans_up_after_its_batch_is_dropped() {
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "oxide-bot-trace-drop-order-{}-{replay_id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let trace_path = dir.join("trace.jsonl");
        let writer = {
            let mut batch = EvidenceBatch::default();
            let writer = batch.stage_trace_jsonl(&trace_path).unwrap();
            drop(batch);
            writer
        };

        drop(writer);
        assert!(!trace_path.exists(), "a dropped stream is never published");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "the writer removes a staging file that outlives its batch"
        );
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn duplicate_evidence_destinations_are_refused_without_leaving_staging_files() {
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "oxide-bot-eval-duplicate-{}-{replay_id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let destination = dir.join("evidence.json");
        let row = evaluate(
            &firing_squad(),
            1,
            EvaluationLeg::Single,
            "candidate-a",
            None,
        )
        .unwrap();

        let preflight_error =
            preflight_destinations(&[destination.clone(), destination.clone()]).unwrap_err();
        assert!(preflight_error.to_string().contains("must be unique"));

        let mut batch = EvidenceBatch::default();
        batch
            .stage_jsonl(std::slice::from_ref(&row), &destination)
            .unwrap();
        let staging_error = batch
            .stage_jsonl(std::slice::from_ref(&row), &destination)
            .unwrap_err();
        assert!(staging_error.to_string().contains("duplicate"));
        drop(batch);

        assert!(!destination.exists(), "a staged payload is not published");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "dropping the refused batch removes its private staging file"
        );
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn an_invalid_evidence_parent_fails_cleanly_without_replacing_the_blocker() {
        let replay_id = NEXT_REPLAY_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "oxide-bot-eval-parent-{}-{replay_id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("not-a-directory");
        std::fs::write(&blocker, b"keep me").unwrap();
        let destination = blocker.join("evidence.jsonl");
        let row = evaluate(
            &firing_squad(),
            1,
            EvaluationLeg::Single,
            "candidate-a",
            None,
        )
        .unwrap();

        let mut batch = EvidenceBatch::default();
        let error = batch
            .stage_jsonl(std::slice::from_ref(&row), &destination)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("creating bot evaluation evidence directory"),
            "bad output parents should retain actionable context: {error:#}"
        );
        assert_eq!(std::fs::read(&blocker).unwrap(), b"keep me");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn evidence_uses_stable_wire_names_and_is_attributed_to_the_emitting_seat() {
        let mut evidence = vec![SeatEvidence::new(0), SeatEvidence::new(1)];
        record_evidence_event(
            &mut evidence,
            &Event::CommandRejected {
                player: PlayerId(1),
                reason: RejectReason::BadSite,
            },
        );
        record_evidence_event(
            &mut evidence,
            &Event::OrderStalled {
                unit: oxide_sim::UnitId(4),
                player: PlayerId(0),
                pos: chassis::fx::Vec2Fx::ZERO,
                reason: StallReason::NoRoute,
            },
        );
        record_evidence_event(
            &mut evidence,
            &Event::CommandRejected {
                player: PlayerId(9),
                reason: RejectReason::BadSite,
            },
        );
        record_evidence_event(
            &mut evidence,
            &Event::GameOver {
                result: GameResult::Draw,
            },
        );

        assert_eq!(evidence[0].rejections, 0);
        assert_eq!(evidence[0].stalls, 1);
        assert_eq!(evidence[0].stall_reasons["no_route"], 1);
        assert_eq!(evidence[0].stall_units[&4]["no_route"], 1);
        assert_eq!(evidence[1].rejections, 1);
        assert_eq!(evidence[1].stalls, 0);
        assert_eq!(evidence[1].rejection_reasons["bad_site"], 1);
    }

    #[test]
    fn candidate_provenance_refuses_empty_padded_control_and_overlong_values() {
        let overlong = "x".repeat(MAX_CANDIDATE_LEN + 1);
        let cases = [
            ("", "must not be empty"),
            (" padded", "surrounding whitespace"),
            ("padded ", "surrounding whitespace"),
            ("line\nbreak", "control characters"),
            (overlong.as_str(), "at most"),
        ];

        for (candidate, expected) in cases {
            let error = evaluate_artifact(&firing_squad(), 1, EvaluationLeg::Single, candidate)
                .unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "{candidate:?} produced an unclear refusal: {error:#}"
            );
        }
    }

    #[test]
    fn paired_legs_exchange_profiles_without_changing_the_match_seed() {
        let legs = configured_legs(
            &firing_squad(),
            91,
            BotDifficulty::Prime,
            BotStance::Aggressive,
            400,
            true,
        )
        .unwrap();
        assert_eq!(legs.len(), 2);
        let (forward_leg, forward) = &legs[0];
        let (swapped_leg, swapped) = &legs[1];
        assert_eq!(*forward_leg, EvaluationLeg::Forward);
        assert_eq!(*swapped_leg, EvaluationLeg::Swapped);
        assert_eq!((forward.seed, swapped.seed), (91, 91));
        assert_eq!(forward.players[0].bot_config.unwrap().personality_seed, 400);
        assert_eq!(forward.players[1].bot_config.unwrap().personality_seed, 401);
        assert_eq!(swapped.players[0].bot_config, forward.players[1].bot_config);
        assert_eq!(swapped.players[1].bot_config, forward.players[0].bot_config);
        assert!(
            forward
                .players
                .iter()
                .chain(&swapped.players)
                .all(|player| player.bot)
        );
    }

    #[test]
    fn cross_difficulty_legs_share_identity_and_swap_complete_configs() {
        let matchup = ProfileMatchup {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            opponent_difficulty: Some(BotDifficulty::Scrapheap),
            opponent_stance: None,
            same_personality_seed: true,
        };
        let source = firing_squad();
        let legs = configured_matchup_legs(&source, 91, matchup, 400, true).unwrap();
        let forward = &legs[0].1;
        let swapped = &legs[1].1;
        let prime = forward.players[0].bot_config.unwrap();
        let scrapheap = forward.players[1].bot_config.unwrap();

        assert_eq!(prime.difficulty, BotDifficulty::Prime);
        assert_eq!(scrapheap.difficulty, BotDifficulty::Scrapheap);
        assert_eq!(
            (prime.stance, scrapheap.stance),
            (BotStance::Balanced, BotStance::Balanced)
        );
        assert_eq!(
            (prime.personality_seed, scrapheap.personality_seed),
            (400, 400)
        );
        assert_eq!(
            prime.resolve_profile().traits,
            scrapheap.resolve_profile().traits
        );
        assert_eq!(swapped.players[0].bot_config, Some(scrapheap));
        assert_eq!(swapped.players[1].bot_config, Some(prime));

        for seat in 0..source.players.len() {
            assert_eq!(forward.players[seat].name, source.players[seat].name);
            assert_eq!(forward.players[seat].faction, source.players[seat].faction);
            assert_eq!(forward.players[seat].team, source.players[seat].team);
            assert_eq!(forward.players[seat].scrap, source.players[seat].scrap);
            assert_eq!(swapped.players[seat].name, source.players[seat].name);
            assert_eq!(swapped.players[seat].faction, source.players[seat].faction);
            assert_eq!(swapped.players[seat].team, source.players[seat].team);
            assert_eq!(swapped.players[seat].scrap, source.players[seat].scrap);
        }
        assert_eq!(forward.map, source.map);
        assert_eq!(swapped.map, source.map);
        assert_eq!(forward.units, source.units);
        assert_eq!(swapped.units, source.units);
    }

    #[test]
    fn personality_seed_runs_are_dense_and_overflow_checked() {
        let distinct = ProfileMatchup::uniform(BotDifficulty::Standard, BotStance::Balanced);
        assert_eq!(
            distinct.personality_seed_base_for_run(100, 0, 2).unwrap(),
            100
        );
        assert_eq!(
            distinct.personality_seed_base_for_run(100, 1, 2).unwrap(),
            102
        );
        assert_eq!(
            distinct.personality_seed_base_for_run(100, 2, 2).unwrap(),
            104
        );

        let shared = ProfileMatchup {
            same_personality_seed: true,
            ..distinct
        };
        assert_eq!(
            shared.personality_seed_base_for_run(100, 0, 2).unwrap(),
            100
        );
        assert_eq!(
            shared.personality_seed_base_for_run(100, 1, 2).unwrap(),
            101
        );
        assert_eq!(
            shared.personality_seed_base_for_run(100, 2, 2).unwrap(),
            102
        );
        assert!(
            distinct
                .personality_seed_base_for_run(u64::MAX, 1, 2)
                .unwrap_err()
                .to_string()
                .contains("overflows")
        );
        assert!(
            shared
                .personality_seed_base_for_run(u64::MAX, 1, 2)
                .unwrap_err()
                .to_string()
                .contains("overflows")
        );
    }

    #[test]
    fn comparison_only_options_refuse_ambiguous_multiseat_scenarios() {
        let mut scenario = firing_squad();
        scenario.players.push(scenario.players[0].clone());
        let uniform = ProfileMatchup::uniform(BotDifficulty::Standard, BotStance::Balanced);
        let cases = [
            (
                "opponent difficulty",
                ProfileMatchup {
                    opponent_difficulty: Some(BotDifficulty::Prime),
                    ..uniform
                },
                false,
            ),
            (
                "opponent stance",
                ProfileMatchup {
                    opponent_stance: Some(BotStance::Aggressive),
                    ..uniform
                },
                false,
            ),
            (
                "shared personality",
                ProfileMatchup {
                    same_personality_seed: true,
                    ..uniform
                },
                false,
            ),
            ("paired", uniform, true),
        ];
        for (label, matchup, paired) in cases {
            let error = configured_matchup_legs(&scenario, 1, matchup, 2, paired).unwrap_err();
            assert!(
                error.to_string().contains("exactly two seats"),
                "{label} produced an unclear error: {error:#}"
            );
        }
    }

    #[test]
    fn paired_legs_refuse_a_shape_that_cannot_be_exchanged() {
        let mut scenario = firing_squad();
        scenario.players.push(scenario.players[0].clone());
        let error = configured_legs(
            &scenario,
            1,
            BotDifficulty::Standard,
            BotStance::Balanced,
            2,
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("exactly two seats"));
    }

    #[test]
    fn a_unit_stalling_the_same_way_repeatedly_is_a_counted_loop() {
        let mut evidence = vec![SeatEvidence::new(0), SeatEvidence::new(1)];
        let stall = Event::OrderStalled {
            unit: oxide_sim::UnitId(4),
            player: PlayerId(1),
            pos: chassis::fx::Vec2Fx::new(
                chassis::fx::Fx::from_num(3),
                chassis::fx::Fx::from_num(3),
            ),
            reason: StallReason::NoRoute,
        };
        let mut last = None;
        for _ in 0..DEFAULT_STALL_LOOP_LIMIT {
            last = record_evidence_event(&mut evidence, &stall);
        }
        let sample = last.expect("a stall is a counted sample");
        assert_eq!(
            (sample.seat, sample.unit, sample.reason.as_str()),
            (1, 4, "no_route")
        );
        assert_eq!(sample.count, DEFAULT_STALL_LOOP_LIMIT);
        assert_eq!(evidence[1].stalls, DEFAULT_STALL_LOOP_LIMIT);
        let other = Event::OrderStalled {
            unit: oxide_sim::UnitId(9),
            player: PlayerId(1),
            pos: chassis::fx::Vec2Fx::new(
                chassis::fx::Fx::from_num(3),
                chassis::fx::Fx::from_num(3),
            ),
            reason: StallReason::NoRoute,
        };
        let fresh = record_evidence_event(&mut evidence, &other).expect("counted");
        assert_eq!(fresh.count, 1, "the loop count is per unit, not per seat");
        assert!(
            record_evidence_event(
                &mut evidence,
                &Event::CommandRejected {
                    player: PlayerId(0),
                    reason: oxide_sim::command::RejectReason::NoValidUnits,
                }
            )
            .is_none(),
            "a rejection is not a stall sample"
        );
        assert_eq!(
            serde_json::to_value(Termination::StallLoop).unwrap(),
            serde_json::json!("stall_loop")
        );
    }

    #[test]
    fn a_stall_loop_limit_of_zero_is_refused_and_none_disables_the_stop() {
        let plan = EvaluationPlan::from_scenario(firing_squad(), EvaluationLeg::Single);
        let error = evaluate_plan_artifact_with(&plan, 100, Some(0), "test-build").unwrap_err();
        assert!(error.to_string().contains("stall-loop limit"), "{error:#}");
        let (row, _) = evaluate_plan_artifact_with(&plan, 100, None, "test-build").unwrap();
        assert_eq!(row.stall_loop_limit, None);
        assert_eq!(row.stall_loop, None);
        let (row, _) = evaluate_plan_artifact(&plan, 100, "test-build").unwrap();
        assert_eq!(row.stall_loop_limit, Some(DEFAULT_STALL_LOOP_LIMIT));
        assert!(
            !serde_json::to_string(&row)
                .unwrap()
                .contains("\"stall_loop\":"),
            "an absent loop stays off the wire"
        );
    }

    #[test]
    fn severed_ground_maps_are_refused_for_the_overseer_yardstick() {
        assert!(ensure_overseer_yardstick_ground(&Scenario::skirmish()).is_ok());
        let severance = Scenario::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/severance.json"
        ))
        .expect("the shipped Severance map loads");
        let error = ensure_overseer_yardstick_ground(&severance).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("share no ground route"), "{text}");
        assert!(text.contains("frozen Overseer"), "{text}");
    }
}
