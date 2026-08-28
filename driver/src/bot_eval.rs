//! Deterministic, player-facing bot match evaluation.
//!
//! This runner complements the frozen Overseer sweeps: it executes the bot
//! configuration serialized in an ordinary scenario, stops when the match is
//! decided, and emits one compact row suitable for JSONL comparison.

use anyhow::{Context, Result, ensure};
use oxide_kit::GameReplay;
use oxide_sim::bot::ResolvedProfile;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use oxide_sim::{Event, GameResult, PlayerId, SIM_VERSION, Scenario};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Why an evaluation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    /// The simulation declared a result.
    Decided,
    /// The configured tick ceiling was reached first.
    TickLimit,
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
    /// The configured built-in controller, or `None` for an empty chair.
    pub config: Option<BotConfig>,
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

    fn observe(&mut self, event: &Event) {
        match event {
            Event::CommandRejected { reason, .. } => {
                self.rejections = self.rejections.saturating_add(1);
                increment(&mut self.rejection_reasons, wire_name(reason));
            }
            Event::OrderStalled { unit, reason, .. } => {
                self.stalls = self.stalls.saturating_add(1);
                let reason = wire_name(reason);
                increment(&mut self.stall_reasons, reason.clone());
                increment(self.stall_units.entry(unit.0).or_default(), reason);
            }
            _ => {}
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
    /// Exact scenario seed.
    pub scenario_seed: u64,
    /// Requested simulation tick ceiling.
    pub tick_limit: u64,
    /// Seat-pair leg.
    pub leg: EvaluationLeg,
    /// Exact player-facing controller configuration by seat.
    pub seats: Vec<SeatConfiguration>,
    /// Whether the match decided before the ceiling.
    pub termination: Termination,
    /// Final game result, absent at the tick ceiling.
    pub result: Option<GameResult>,
    /// Seats on the surviving team, empty for draws and undecided matches.
    pub winner_seats: Vec<u8>,
    /// Simulation ticks executed.
    pub duration_ticks: u64,
    /// Canonical final state hash.
    pub final_hash: String,
    /// Per-seat command failure evidence.
    pub evidence: Vec<SeatEvidence>,
    /// Saved replay path, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<String>,
}

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
    ensure!(tick_limit > 0, "bot evaluation tick limit must be positive");
    validate_candidate(candidate)?;
    let scenario_fingerprint = scenario_fingerprint(scenario)?;

    let mut state = scenario
        .build()
        .context("building bot evaluation scenario")?;
    let mut bots = oxide_sim::bot::seat_bots(scenario);
    let mut replay = GameReplay::new(SIM_VERSION, scenario.clone());
    replay.meta.kind = Some("bot-eval".into());
    replay.meta.description = Some(format!(
        "bot-eval candidate={candidate}; scenario={scenario_fingerprint}; tick_limit={tick_limit}; leg={}",
        leg.name()
    ));
    let mut evidence: Vec<SeatEvidence> =
        (0..scenario.players.len()).map(SeatEvidence::new).collect();

    while state.current_tick() < tick_limit && state.result().is_none() {
        let report = oxide_kit::runner::step(&mut state, &mut bots, Some(&mut replay));
        for event in &report.events {
            record_evidence_event(&mut evidence, event);
        }
    }

    replay.meta.ticks = Some(state.current_tick());
    for command in &replay.commands {
        if let Some(row) = evidence.get_mut(usize::from(command.command.player.0)) {
            row.commands = row.commands.saturating_add(1);
        }
    }

    let row = EvaluationRow {
        sim_version: SIM_VERSION,
        candidate: candidate.to_string(),
        scenario: scenario.name.clone(),
        scenario_fingerprint,
        scenario_seed: scenario.seed,
        tick_limit,
        leg,
        seats: scenario
            .players
            .iter()
            .enumerate()
            .map(|(seat, player)| SeatConfiguration {
                seat: seat as u8,
                config: player.bot_config,
                profile: player.bot_config.map(BotConfig::resolve_profile),
            })
            .collect(),
        termination: if state.result().is_some() {
            Termination::Decided
        } else {
            Termination::TickLimit
        },
        result: state.result(),
        winner_seats: state.winners().into_iter().map(|seat| seat.0).collect(),
        duration_ticks: state.current_tick(),
        final_hash: oxide_protocol::hash_hex(state.hash()),
        evidence,
        replay: None,
    };
    Ok((row, replay))
}

fn record_evidence_event(evidence: &mut [SeatEvidence], event: &Event) {
    let player = match event {
        Event::CommandRejected { player, .. } | Event::OrderStalled { player, .. } => Some(*player),
        _ => None,
    };
    if let Some(PlayerId(seat)) = player
        && let Some(row) = evidence.get_mut(usize::from(seat))
    {
        row.observe(event);
    }
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

/// Atomically writes compact evaluation records as one JSON object per line.
pub fn write_jsonl(rows: &[EvaluationRow], path: &Path) -> Result<()> {
    chassis::fsx::write_atomic(path, |writer| -> Result<()> {
        for row in rows {
            serde_json::to_writer(&mut *writer, row)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    })
    .with_context(|| format!("writing bot evaluation JSONL to {}", path.display()))
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

    /// Publishes every staged file as a create-only hard link.
    ///
    /// Because each staging path is a sibling of its destination, the link
    /// stays on one filesystem and cannot overwrite an earlier evidence file.
    /// Errors returned from this call roll back links already created by this
    /// batch; abrupt process termination can interrupt that rollback.
    pub fn publish(mut self) -> Result<()> {
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
    validate_candidate(candidate)?;
    let configured = serde_json::to_vec(&(candidate, SIM_VERSION, tick_limit, scenario))
        .context("serializing replay filename input")?;
    let digest = chassis::hash::fnv1a(&configured);
    Ok(format!(
        "{scenario_index:03}-{run:03}-s{scenario_seed}-{}-c{digest:016x}.json",
        leg.name()
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
    use oxide_sim::command::RejectReason;
    use oxide_sim::scenario::{PlayerSpec, UnitSpec};
    use oxide_sim::{Faction, StallReason, UnitKind};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_REPLAY_ID: AtomicU64 = AtomicU64::new(0);

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
}
