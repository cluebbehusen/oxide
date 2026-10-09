//! Deterministic, player-facing bot match evaluation.
//!
//! Runs the bot configuration serialized in an ordinary scenario, stops when
//! the match is decided, and emits one compact row for JSONL comparison.

use anyhow::{Context, Result, ensure};
use oxide_kit::GameReplay;
use oxide_kit::controller::{OpponentMap, SeatController, SeatTrace};
use oxide_opponent::ResolvedProfile;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use oxide_sim::{Event, Faction, GameResult, PlayerId, SIM_VERSION, Scenario};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

mod batch;
mod calibration;
mod failures;
mod income;
mod reactivity;
pub use batch::{EvaluationBatchOptions, EvaluationBatchResult, evaluate_batch};
pub use calibration::{AttackBucket, AttackCalibration};
pub use failures::{
    DELIVERY_TICKS, Deliveries, ENGAGE_MARGIN, EXEMPT_STALL_REASON, FAILURE_WINDOW_TICKS,
    FailureIncident, FailureTally, HOME_REACH, IDLE_ARMY_FLOOR, IDLE_TICKS, MAX_FAILURE_EXAMPLES,
    ProducerIdle, REPEATED_ORDER_STALLS, REST_DRIFT, SeatFailures,
};
pub use income::{
    HARVESTERS_PER_NODE, INCOME_CHECKPOINTS, INCOME_WINDOW_TICKS, IncomeSample, NODES_PER_FOUNDRY,
    saturation_per_minute,
};
pub use reactivity::{
    ANTI_AIR_TICKS, ARTILLERY_TICKS, CLEAR_TILES, DAMAGED, DEFENSE_TICKS, EVACUATE_TICKS,
    HOME_TILES, PRESS_TILES, RELIEF_TICKS, REPAIR_TICKS, RESTORE_TICKS, RUN_TILES, Reactions,
    SCOUT_TICKS, STALE_TICKS, SeatReactivity,
};

const MAX_CANDIDATE_LEN: usize = 128;

/// Ticks between failure-detector and passive-income checks.
pub const QA_CHECK_PERIOD: u64 = 12;

/// Which half of a seat-paired evaluation produced a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Exact evaluation-only command source for one seat: one configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EvaluationController {
    config: BotConfig,
}

impl EvaluationController {
    /// The evaluation source for one configuration.
    pub fn configured(config: BotConfig) -> Self {
        Self { config }
    }

    pub(crate) fn config(self) -> BotConfig {
        self.config
    }

    fn profile(self) -> ResolvedProfile {
        ResolvedProfile::resolve(self.config)
    }

    fn seat_controller(
        self,
        player: PlayerId,
        opponent_map: &OpponentMap<'_>,
    ) -> Result<SeatController> {
        SeatController::configured(player, self.config(), opponent_map)
            .context("building an evaluation seat's map model")
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
            Self::Rot180 => crate::rotation::rotate_180(scenario),
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
                    .map(EvaluationController::configured)
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

    fn seat_controllers(&self) -> Result<Vec<SeatController>> {
        let opponent_map = OpponentMap::new(&self.scenario);
        self.controllers
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(seat, controller)| {
                controller.map(|controller| {
                    controller.seat_controller(PlayerId::from_index(seat), &opponent_map)
                })
            })
            .collect()
    }
}

/// Why an evaluation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    /// The simulation declared a result.
    Decided,
    /// The configured tick ceiling was reached first.
    TickLimit,
    /// One unit stalled the same way often enough to show a controller
    /// re-issuing an impossible order; the leg stopped early.
    StallLoop,
}

/// Stalls of one reason on one unit that end a leg as a [`Termination::StallLoop`]
/// when no explicit limit is given. A blocked order that a controller
/// abandons stalls a handful of times; an order re-issued every decision on a
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

/// Profile choices for one evaluation cell.
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
    /// Distinct-seat cells advance by the seat count per run so every seat
    /// gets a consecutive seed; shared-seed comparisons advance by one.
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
        BotConfig::new(difficulty, stance, personality_seed)
    }
}

/// Exact controller provenance for one seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeatConfiguration {
    /// Player seat.
    pub seat: u8,
    /// Faction roster bound to the physical seat.
    pub faction: Faction,
    /// Team the seat plays for; seats sharing one are allies.
    pub team: u8,
    /// The seat's bot configuration, or `None` for an empty chair.
    pub config: Option<BotConfig>,
    /// Fully resolved hidden personality, included for exact comparison.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ResolvedProfile>,
}

/// Compact command failure and QA evidence for one seat.
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
    /// Tick the seat resigned or lost its last Foundry; absent while it stands.
    pub eliminated_at: Option<u64>,
    /// Consequential failures found by omniscient detectors.
    pub failures: SeatFailures,
    /// Diagnostic: producers that sat idle while the bank could pay for them.
    pub idle_producers: Vec<ProducerIdle>,
    /// Diagnostic: what became of armed ground units trained on severed
    /// ground; absent for seats without a controller.
    pub deliveries: Option<Deliveries>,
    /// Actual income against a saturation estimate at each checkpoint reached.
    pub income: Vec<IncomeSample>,
    /// Situations the seat met and how it answered them; absent for seats
    /// without a controller.
    pub reactivity: Option<SeatReactivity>,
    /// What the seat's units and buildings did, valued in scrap; absent for
    /// seats without a controller.
    pub ledger: Option<crate::ledger::SeatLedger>,
    /// What `oxide-opponent` believed when it launched each attack, against
    /// how the attack went; absent for other seats.
    pub attacks: Option<AttackCalibration>,
}

impl SeatEvidence {
    fn new(seat: usize) -> Self {
        Self {
            seat: u8::try_from(seat).expect("seat indices fit in u8"),
            commands: 0,
            rejections: 0,
            rejection_reasons: BTreeMap::new(),
            stalls: 0,
            stall_reasons: BTreeMap::new(),
            stall_units: BTreeMap::new(),
            eliminated_at: None,
            failures: SeatFailures::default(),
            idle_producers: Vec::new(),
            deliveries: None,
            income: Vec::new(),
            reactivity: None,
            ledger: None,
            attacks: None,
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
    /// The driver build that produced the record.
    pub build: oxide_kit::recovery::BuildIdentity,
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
    pub trace: SeatTrace,
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
/// Controller identities come from the evaluation plan and are recorded in
/// replay provenance independently of the physical scenario configuration.
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
/// [`evaluate_plan_artifact_with`]. Every controlled seat emits decision traces.
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
    let mut bots = plan.seat_controllers()?;
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
    let watched: Vec<bool> = plan.controllers.iter().map(Option::is_some).collect();
    let mut failures = failures::FailureDetectors::new(watched.iter().copied());
    let mut reactions = reactivity::ReactivityDetectors::new(watched.iter().copied());
    let mut income = income::IncomeTracker::new(&state, watched.clone());
    let mut ledger = crate::ledger::ImpactLedger::new(&state);
    let mut attacks = calibration::AttackFollower::new(watched.clone());

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
        let tick = state.current_tick();
        for bot in &bots {
            attacks.open(bot.player().0, bot.opponent().launches(), &ledger);
        }
        ledger.observe(&state, &report);
        failures.observe_events(&state, &report.events, tick);
        reactions.observe_events(&state, &report.events, tick);
        for event in &report.events {
            let Some(sample) = record_evidence_event(&mut evidence, event) else {
                continue;
            };
            failures.record_stall(sample.seat, sample.unit, &sample.reason, tick);
            if stall_loop_limit.is_some_and(|limit| sample.count >= limit) {
                stall_loop = Some(StallLoop {
                    seat: sample.seat,
                    unit: sample.unit,
                    reason: sample.reason,
                    count: sample.count,
                    tick,
                });
                break 'run;
            }
        }
        income.observe(&state, &report.events, QA_CHECK_PERIOD);
        if tick.is_multiple_of(QA_CHECK_PERIOD) {
            let mut protected = vec![0_u32; scenario.players.len()];
            for bot in &bots {
                protected[usize::from(bot.player().0)] = bot.protected_scrap();
                if state.accepts_commands(bot.player()) {
                    let missions = bot.opponent().missions();
                    failures.check_missions(bot.player().0, tick, &missions);
                    reactions.check_missions(bot.player().0, tick, &missions);
                    attacks.follow(bot.player().0, tick, &missions, &ledger);
                }
            }
            failures.check(&state, tick, &protected);
            reactions.check(&state, tick);
        }
    }
    let calibrations = attacks.finish(&ledger);
    let ledgers = ledger.finish(&state);
    for (seat, (((((evidence, report), income), reactivity), seat_ledger), calibration)) in evidence
        .iter_mut()
        .zip(failures.finish())
        .zip(income.finish())
        .zip(reactions.finish())
        .zip(ledgers)
        .zip(calibrations)
        .enumerate()
    {
        evidence.ledger = watched[seat].then_some(seat_ledger);
        evidence.attacks = calibration;
        evidence.eliminated_at = state.players()[seat].eliminated_at;
        evidence.failures = report.failures;
        evidence.idle_producers = report.idle_producers;
        evidence.deliveries = report.deliveries;
        evidence.income = income;
        evidence.reactivity = reactivity;
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
        build: crate::build_identity(),
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
                seat: u8::try_from(seat).expect("seat indices fit in u8"),
                faction: player.faction,
                team: state.players()[seat].team,
                config: plan.controllers[seat].map(EvaluationController::config),
                profile: plan.controllers[seat].map(EvaluationController::profile),
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

/// Builds a faction and geometry cell's legs from complete controller profiles.
/// Paired legs exchange the profiles while preserving the physical map and rosters.
pub fn configured_matchup_plans(
    source: &Scenario,
    scenario_seed: u64,
    matchup: ProfileMatchup,
    personality_seed_base: u64,
    paired: bool,
    faction_cell: EvaluationFactionCell,
    geometry: EvaluationGeometry,
) -> Result<Vec<EvaluationPlan>> {
    ensure!(
        source.players.len() == 2,
        "controlled evaluation axes require exactly two seats, got {}",
        source.players.len()
    );
    ensure!(
        source.players[0].team.is_none() || source.players[0].team != source.players[1].team,
        "controlled evaluation axes require two opposing teams"
    );
    let mut scenario = geometry.apply(source)?;
    faction_cell.apply(&mut scenario)?;
    configured_matchup_legs(
        &scenario,
        scenario_seed,
        matchup,
        personality_seed_base,
        paired,
    )?
    .into_iter()
    .map(|(leg, scenario)| {
        let mut plan = EvaluationPlan::from_scenario(scenario, leg);
        for player in &mut plan.scenario.players {
            player.bot = false;
            player.bot_config = None;
        }
        plan.geometry = geometry;
        plan.faction_cell = faction_cell;
        Ok(plan)
    })
    .collect()
}

/// Atomically writes compact evaluation records as one JSON object per line.
pub fn write_jsonl(rows: &[EvaluationRow], path: &Path) -> Result<()> {
    write_serialized_jsonl(rows, path, "bot evaluation JSONL")
}

pub(crate) fn write_serialized_jsonl<T: Serialize>(
    rows: &[T],
    path: &Path,
    label: &str,
) -> Result<()> {
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

    /// Stages a JSONL index next to its eventual destination.
    pub fn stage_jsonl<T: Serialize>(&mut self, rows: &[T], destination: &Path) -> Result<()> {
        let staged = self.reserve_stage(destination)?;
        write_serialized_jsonl(rows, &staged, "evaluation JSONL")
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
        static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
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
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
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
mod tests;
