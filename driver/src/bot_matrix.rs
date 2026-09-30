//! Evaluation matrices of `oxide-opponent` against the frozen `oxide-bot`
//! reference on duel, team and free-for-all maps.
//!
//! A manifest names maps with a family plus difficulties, stances, runs, seed
//! bases and a tick limit. Every map, difficulty, stance and run becomes one
//! cell. Each compared pairing of a cell is two legs, the second with every
//! seat's controller exchanged, and every seat shares one personality seed:
//!
//! - a duel is a head-to-head pair, the new bot in seat zero and then in seat
//!   one;
//! - a team map has a head-to-head pair, one team of each bot and then the
//!   sides swapped, and a mixed pair, the bots alternating along each team's
//!   front so that neighbouring enemies run different bots;
//! - a free-for-all is a mixed pair, the bots on alternating seats.
//!
//! Every seat is controlled, including a map's authored human chair. Each cell
//! has one baseline leg with `oxide-bot` in every seat; exchanging identical
//! seats would repeat that match exactly. Baseline rows are cached under the
//! reference digest of the `bot/`, `sim/` and `chassis/` sources, the `kit`
//! code that hosts `oxide-bot`, and `Cargo.lock`, so a change that leaves those
//! untouched reuses them.

mod report;
pub use report::{
    ControllerTally, GroupReport, IncomeMedian, LegTally, MatrixReport, NewShare, PairTally,
    PairingReport, Placement, Provenance, ScoredRow, build_report, load_rows,
};

use crate::bot_eval::{
    DEFAULT_STALL_LOOP_LIMIT, EvaluationBatchOptions, EvaluationController, EvaluationFactionCell,
    EvaluationGeometry, EvaluationLeg, EvaluationPlan, EvidenceBatch, REFERENCE_DIGEST,
    ensure_unique_execution_plans, evaluate_batch, execution_fingerprint, preflight_destinations,
};
use anyhow::{Context, Result, bail, ensure};
use oxide_sim::scenario::{BotConfig, BotController, BotDifficulty, BotStance};
use oxide_sim::{SIM_VERSION, Scenario};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Bump when the evaluation loop changes what a row records, so cached
/// baseline rows are recomputed rather than mixed with new ones.
pub const BASELINE_CACHE_VERSION: u32 = 1;

/// File a matrix run publishes in its output directory.
pub const ROWS_FILE: &str = "rows.jsonl";

/// Map shape, reported separately because it decides which capabilities a
/// match needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapFamily {
    /// Open ground between the bases.
    Open,
    /// Ground routes narrowed by terrain.
    Constrained,
    /// No ground route between the bases.
    Severed,
}

impl MapFamily {
    /// Stable lowercase name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Constrained => "constrained",
            Self::Severed => "severed",
        }
    }
}

/// How a map divides its seats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Two seats on opposing teams.
    Duel,
    /// Two teams of equal size.
    Teams,
    /// More than two seats, each its own team.
    FreeForAll,
}

impl MatchMode {
    /// Stable lowercase name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Duel => "duel",
            Self::Teams => "teams",
            Self::FreeForAll => "free-for-all",
        }
    }

    /// Classifies seats by their teams; any other division is refused.
    pub fn of(teams: &[u8]) -> Result<Self> {
        let mut sizes: BTreeMap<u8, usize> = BTreeMap::new();
        for &team in teams {
            *sizes.entry(team).or_default() += 1;
        }
        let equal = sizes
            .values()
            .all(|&size| Some(&size) == sizes.values().next());
        Ok(match (teams.len(), sizes.len()) {
            (2, 2) => Self::Duel,
            (seats, count) if seats > 2 && count == seats => Self::FreeForAll,
            (_, 2) if equal => Self::Teams,
            _ => bail!(
                "seats on teams {teams:?} are neither a duel, two equal teams nor a free-for-all"
            ),
        })
    }
}

/// Which comparison a matrix leg belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pairing {
    /// `oxide-opponent` on one side against `oxide-bot` on the other, scored
    /// by which side wins.
    HeadToHead,
    /// Both bots on every side, scored by which bot's seats outlast the
    /// other's.
    Mixed,
    /// `oxide-bot` against itself.
    Baseline,
}

/// A matrix definition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixManifest {
    /// Name recorded in every row.
    pub name: String,
    /// Maximum ticks per leg.
    pub tick_limit: u64,
    /// Seed cells per map, difficulty and stance.
    pub runs: u64,
    /// Simulation seed of run zero; each run adds one.
    pub scenario_seed_base: u64,
    /// Personality seed of run zero, shared by every seat; each run adds one.
    pub personality_seed_base: u64,
    /// Difficulties, applied to every seat.
    pub difficulties: Vec<BotDifficulty>,
    /// Stances, applied to every seat.
    pub stances: Vec<BotStance>,
    /// Duel, team and free-for-all maps.
    pub maps: Vec<ManifestMap>,
}

/// One manifest map.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMap {
    /// Scenario path relative to the manifest, or `skirmish` for the built-in
    /// map.
    pub path: String,
    /// Map shape.
    pub family: MapFamily,
}

impl ManifestMap {
    /// Row label: the scenario file stem.
    pub fn key(&self) -> String {
        Path::new(&self.path)
            .file_stem()
            .map_or_else(|| self.path.clone(), |stem| stem.to_string_lossy().into())
    }
}

impl MatrixManifest {
    /// Reads and validates a manifest.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading matrix manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing matrix manifest {}", path.display()))?;
        manifest
            .validate()
            .with_context(|| format!("validating matrix manifest {}", path.display()))?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.is_empty() && !self.name.contains(char::is_whitespace),
            "manifest name must be a non-empty word"
        );
        ensure!(self.tick_limit > 0, "tick limit must be positive");
        ensure!(self.runs > 0, "runs must be positive");
        for base in [self.scenario_seed_base, self.personality_seed_base] {
            ensure!(
                base.checked_add(self.runs - 1).is_some(),
                "seed range overflows u64"
            );
        }
        ensure!(!self.difficulties.is_empty(), "difficulties are empty");
        ensure!(!self.stances.is_empty(), "stances are empty");
        ensure!(!self.maps.is_empty(), "maps are empty");
        ensure!(distinct(&self.difficulties), "difficulties repeat a value");
        ensure!(distinct(&self.stances), "stances repeat a value");
        let keys: Vec<String> = self.maps.iter().map(ManifestMap::key).collect();
        ensure!(distinct(&keys), "maps repeat a scenario name");
        Ok(())
    }

    /// Loads every map, resolving relative paths against `base`, and checks
    /// that each builds.
    pub fn scenarios(&self, base: &Path) -> Result<Vec<Scenario>> {
        self.maps
            .iter()
            .map(|map| {
                let scenario = if map.path == "skirmish" {
                    Scenario::skirmish()
                } else {
                    let path = base.join(&map.path);
                    Scenario::load(&path)
                        .with_context(|| format!("loading scenario {}", path.display()))?
                };
                scenario
                    .build()
                    .with_context(|| format!("building scenario {}", map.path))?;
                Ok(scenario)
            })
            .collect()
    }
}

fn distinct<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[..index].contains(value))
}

/// Where a row sits in its matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixLabel {
    /// Manifest name.
    pub manifest: String,
    /// Scenario file stem.
    pub map: String,
    /// Map shape.
    pub family: MapFamily,
    /// Difficulty of every seat.
    pub difficulty: BotDifficulty,
    /// Stance of every seat.
    pub stance: BotStance,
    /// Seed cell.
    pub run: u64,
    /// Head-to-head, mixed or baseline.
    pub pairing: Pairing,
}

/// One leg to evaluate.
#[derive(Debug, Clone)]
pub struct MatrixLeg {
    /// Matrix position.
    pub label: MatrixLabel,
    /// Exact evaluation plan.
    pub plan: EvaluationPlan,
}

/// Expands a manifest into legs: for each map, difficulty, stance and run,
/// each compared pairing's forward and swapped legs followed by the baseline
/// leg.
pub fn expand(manifest: &MatrixManifest, scenarios: &[Scenario]) -> Result<Vec<MatrixLeg>> {
    ensure!(
        scenarios.len() == manifest.maps.len(),
        "{} scenarios for {} manifest maps",
        scenarios.len(),
        manifest.maps.len()
    );
    let mut legs = Vec::new();
    for (map, source) in manifest.maps.iter().zip(scenarios) {
        let layouts = layouts(source).with_context(|| format!("planning {}", map.path))?;
        for &difficulty in &manifest.difficulties {
            for &stance in &manifest.stances {
                for run in 0..manifest.runs {
                    let label = |pairing| MatrixLabel {
                        manifest: manifest.name.clone(),
                        map: map.key(),
                        family: map.family,
                        difficulty,
                        stance,
                        run,
                        pairing,
                    };
                    let seat = |opponent: bool| BotConfig {
                        controller: if opponent {
                            BotController::Opponent
                        } else {
                            BotController::Scripted
                        },
                        difficulty,
                        stance,
                        personality_seed: manifest.personality_seed_base + run,
                    };
                    let seed = manifest.scenario_seed_base + run;
                    for (pairing, forward) in &layouts {
                        for (leg, swapped) in [
                            (EvaluationLeg::Forward, false),
                            (EvaluationLeg::Swapped, true),
                        ] {
                            let seats = forward.iter().map(|&opponent| seat(opponent != swapped));
                            legs.push(MatrixLeg {
                                label: label(*pairing),
                                plan: seated_plan(source, seed, leg, seats),
                            });
                        }
                    }
                    let seats = source.players.iter().map(|_| seat(false));
                    legs.push(MatrixLeg {
                        label: label(Pairing::Baseline),
                        plan: seated_plan(source, seed, EvaluationLeg::Single, seats),
                    });
                }
            }
        }
    }
    ensure_unique_execution_plans(legs.iter().map(|leg| &leg.plan))?;
    Ok(legs)
}

/// A map's teams by seat.
fn seat_teams(source: &Scenario) -> Result<Vec<u8>> {
    Ok(source
        .build()?
        .players()
        .iter()
        .map(|player| player.team)
        .collect())
}

/// The compared pairings of a map, each with the seats that run
/// `oxide-opponent` in its forward leg.
fn layouts(source: &Scenario) -> Result<Vec<(Pairing, Vec<bool>)>> {
    let teams = seat_teams(source)?;
    let first = teams[0];
    Ok(match MatchMode::of(&teams)? {
        MatchMode::Duel => vec![(Pairing::HeadToHead, vec![true, false])],
        MatchMode::Teams => vec![
            (
                Pairing::HeadToHead,
                teams.iter().map(|&team| team == first).collect(),
            ),
            (
                Pairing::Mixed,
                front_places(source, &teams)?
                    .into_iter()
                    .zip(&teams)
                    .map(|(place, &team)| (place + usize::from(team != first)).is_multiple_of(2))
                    .collect(),
            ),
        ],
        MatchMode::FreeForAll => vec![(
            Pairing::Mixed,
            (0..teams.len())
                .map(|seat| seat.is_multiple_of(2))
                .collect(),
        )],
    })
}

/// Each seat's place within its team along the front: its Foundry's position
/// across the line between the two teams' Foundries, ties by seat.
fn front_places(source: &Scenario, teams: &[u8]) -> Result<Vec<usize>> {
    let (_, anchors) = source.parse_map_and_anchors()?;
    let mut foundries = vec![None; teams.len()];
    for (player, anchor) in anchors {
        let foundry = &mut foundries[usize::from(player.0)];
        ensure!(foundry.is_none(), "seat {} has two Foundries", player.0);
        *foundry = Some((i64::from(anchor.x), i64::from(anchor.y)));
    }
    let foundries: Vec<(i64, i64)> = foundries
        .into_iter()
        .collect::<Option<_>>()
        .context("a seat has no Foundry")?;
    let (mut dx, mut dy) = (0, 0);
    for (&(x, y), &team) in foundries.iter().zip(teams) {
        let sign = if team == teams[0] { -1 } else { 1 };
        dx += sign * x;
        dy += sign * y;
    }
    let mut order: Vec<usize> = (0..teams.len()).collect();
    order.sort_by_key(|&seat| {
        let (x, y) = foundries[seat];
        (teams[seat], y * dx - x * dy, seat)
    });
    let mut places = vec![0; teams.len()];
    for (index, &seat) in order.iter().enumerate() {
        let team_start = order
            .iter()
            .position(|&other| teams[other] == teams[seat])
            .expect("the seat itself is in the order");
        places[seat] = index - team_start;
    }
    Ok(places)
}

/// A plan with every seat controlled, the authored human chair included.
fn seated_plan(
    source: &Scenario,
    scenario_seed: u64,
    leg: EvaluationLeg,
    seats: impl Iterator<Item = BotConfig>,
) -> EvaluationPlan {
    let mut scenario = source.clone();
    scenario.seed = scenario_seed;
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }
    EvaluationPlan {
        leg,
        scenario,
        controllers: seats
            .map(|config| Some(EvaluationController::configured(config)))
            .collect(),
        geometry: EvaluationGeometry::Authored,
        faction_cell: EvaluationFactionCell::Authored,
    }
}

/// Baseline rows on disk, keyed by the reference digest and each leg's exact
/// execution. Entries from another digest, simulation version, tick limit or
/// stall-loop limit are never read.
pub struct BaselineCache {
    directory: PathBuf,
}

impl BaselineCache {
    /// A cache under `root`, in a directory for this build's reference digest.
    pub fn new(root: &Path) -> Self {
        let digest = REFERENCE_DIGEST
            .rsplit(':')
            .next()
            .unwrap_or(REFERENCE_DIGEST);
        Self {
            directory: root.join(digest),
        }
    }

    fn entry(&self, plan: &EvaluationPlan, ticks: u64, stall: Option<u64>) -> Result<Entry> {
        let execution = execution_fingerprint(plan)?;
        let key = serde_json::to_vec(&(
            BASELINE_CACHE_VERSION,
            SIM_VERSION,
            REFERENCE_DIGEST,
            &execution,
            ticks,
            stall,
        ))?;
        Ok(Entry {
            path: self
                .directory
                .join(format!("{:016x}.json", chassis::hash::fnv1a(&key))),
            execution,
        })
    }

    /// The cached row for this leg, if one was stored under the same identity.
    pub fn load(
        &self,
        plan: &EvaluationPlan,
        ticks: u64,
        stall: Option<u64>,
    ) -> Result<Option<serde_json::Value>> {
        let entry = self.entry(plan, ticks, stall)?;
        let bytes = match std::fs::read(&entry.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading cached row {}", entry.path.display()));
            }
        };
        let Ok(row) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Ok(None);
        };
        let matches = row["execution_fingerprint"] == entry.execution.as_str()
            && row["tick_limit"] == ticks
            && row["stall_loop_limit"] == serde_json::to_value(stall)?
            && row["sim_version"] == SIM_VERSION
            && row["reference_digest"] == REFERENCE_DIGEST;
        Ok(matches.then_some(row))
    }

    /// Stores a freshly evaluated row for this leg.
    pub fn store(
        &self,
        plan: &EvaluationPlan,
        ticks: u64,
        stall: Option<u64>,
        row: &serde_json::Value,
    ) -> Result<()> {
        let entry = self.entry(plan, ticks, stall)?;
        std::fs::create_dir_all(&self.directory)
            .with_context(|| format!("creating baseline cache {}", self.directory.display()))?;
        chassis::fsx::write_atomic(&entry.path, |writer| -> Result<()> {
            serde_json::to_writer(writer, row)?;
            Ok(())
        })
        .with_context(|| format!("writing cached row {}", entry.path.display()))
    }
}

struct Entry {
    path: PathBuf,
    execution: String,
}

/// The default baseline cache, shared by every checkout of this user:
/// `oxide/bot-matrix-baseline` under `$XDG_CACHE_HOME`, else `$HOME/.cache`,
/// else `%LOCALAPPDATA%`.
pub fn default_baseline_cache() -> Option<PathBuf> {
    cache_root(
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("LOCALAPPDATA"),
    )
}

fn cache_root(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    local: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let set = |value: Option<std::ffi::OsString>| value.filter(|value| !value.is_empty());
    set(xdg)
        .map(PathBuf::from)
        .or_else(|| set(home).map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| set(local).map(PathBuf::from))
        .map(|root| root.join("oxide").join("bot-matrix-baseline"))
}

/// Execution settings for [`run_matrix`].
pub struct MatrixOptions<'a> {
    /// Candidate recorded in freshly evaluated rows.
    pub candidate: &'a str,
    /// Upper bound on concurrent matches.
    pub jobs: NonZeroUsize,
    /// Baseline cache root.
    pub baseline_cache: &'a Path,
}

/// One published row with its matrix position.
#[derive(Debug, Clone, Serialize)]
pub struct LabelledRow {
    /// Matrix position.
    pub matrix: MatrixLabel,
    /// The evaluation row.
    #[serde(flatten)]
    pub row: serde_json::Value,
}

/// Rows of one matrix run, in leg order.
pub struct MatrixRun {
    /// Labelled rows.
    pub rows: Vec<LabelledRow>,
    /// Legs evaluated by this run.
    pub evaluated: usize,
    /// Baseline legs reused from the cache.
    pub reused: usize,
}

/// Evaluates every leg not already cached and returns all rows in leg order.
pub fn run_matrix(
    legs: &[MatrixLeg],
    ticks: u64,
    options: &MatrixOptions<'_>,
) -> Result<MatrixRun> {
    let cache = BaselineCache::new(options.baseline_cache);
    let stall = Some(DEFAULT_STALL_LOOP_LIMIT);
    let mut rows: Vec<Option<serde_json::Value>> = vec![None; legs.len()];
    let mut pending = Vec::new();
    for (index, leg) in legs.iter().enumerate() {
        let cached = if leg.label.pairing == Pairing::Baseline {
            cache.load(&leg.plan, ticks, stall)?
        } else {
            None
        };
        match cached {
            Some(row) => rows[index] = Some(row),
            None => pending.push(index),
        }
    }
    let plans: Vec<(EvaluationPlan, Option<PathBuf>)> = pending
        .iter()
        .map(|&index| (legs[index].plan.clone(), None))
        .collect();
    let evaluated = evaluate_batch(
        &plans,
        &EvaluationBatchOptions {
            ticks,
            stall_loop_limit: stall,
            candidate: options.candidate,
            jobs: options.jobs,
            output: None,
            trace_output: None,
        },
    )?
    .rows;
    for (&index, row) in pending.iter().zip(&evaluated) {
        let value = serde_json::to_value(row)?;
        if legs[index].label.pairing == Pairing::Baseline {
            cache.store(&legs[index].plan, ticks, stall, &value)?;
        }
        rows[index] = Some(value);
    }
    Ok(MatrixRun {
        evaluated: pending.len(),
        reused: legs.len() - pending.len(),
        rows: legs
            .iter()
            .zip(rows)
            .map(|(leg, row)| LabelledRow {
                matrix: leg.label.clone(),
                row: row.expect("every leg was evaluated or reused"),
            })
            .collect(),
    })
}

/// Refuses an output directory that already holds matrix rows.
pub fn preflight_output(directory: &Path) -> Result<PathBuf> {
    let path = directory.join(ROWS_FILE);
    preflight_destinations(std::slice::from_ref(&path))?;
    Ok(path)
}

/// Publishes rows to `path` without replacing an existing file.
pub fn publish_rows(rows: &[LabelledRow], path: &Path) -> Result<()> {
    let mut batch = EvidenceBatch::default();
    batch.stage_jsonl(rows, path)?;
    batch.publish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_eval::{ProfileMatchup, configured_matchup_plans};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "oxide-bot-matrix-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn manifest() -> MatrixManifest {
        serde_json::from_value(serde_json::json!({
            "name": "unit",
            "tick_limit": 30,
            "runs": 2,
            "scenario_seed_base": 70,
            "personality_seed_base": 900,
            "difficulties": ["standard", "prime"],
            "stances": ["aggressive"],
            "maps": [{"path": "skirmish", "family": "open"}],
        }))
        .unwrap()
    }

    #[test]
    fn manifests_expand_into_seat_swapped_pairs_and_one_baseline_leg() {
        let manifest = manifest();
        manifest.validate().unwrap();
        let legs = expand(&manifest, &manifest.scenarios(Path::new(".")).unwrap()).unwrap();
        assert_eq!(legs.len(), 2 * 2 * 3);
        for (cell, chunk) in legs.chunks(3).enumerate() {
            let difficulty = [BotDifficulty::Standard, BotDifficulty::Prime][cell / 2];
            let run = (cell % 2) as u64;
            let config = |controller| BotConfig {
                controller,
                difficulty,
                stance: BotStance::Aggressive,
                personality_seed: 900 + run,
            };
            let opponent = Some(EvaluationController::configured(config(
                BotController::Opponent,
            )));
            let scripted = Some(EvaluationController::configured(config(
                BotController::Scripted,
            )));
            let [forward, swapped, baseline] = chunk else {
                unreachable!()
            };
            assert_eq!(forward.plan.controllers, [opponent, scripted]);
            assert_eq!(swapped.plan.controllers, [scripted, opponent]);
            assert_eq!(baseline.plan.controllers, [scripted, scripted]);
            assert_eq!(
                (forward.plan.leg, swapped.plan.leg, baseline.plan.leg),
                (
                    EvaluationLeg::Forward,
                    EvaluationLeg::Swapped,
                    EvaluationLeg::Single
                )
            );
            for leg in chunk {
                assert_eq!(leg.plan.scenario.seed, 70 + run);
                assert_eq!(leg.label.map, "skirmish");
                assert_eq!(leg.label.family, MapFamily::Open);
                assert_eq!(leg.label.difficulty, difficulty);
                assert_eq!(leg.label.run, run);
            }
            assert_eq!(forward.label.pairing, Pairing::HeadToHead);
            assert_eq!(swapped.label.pairing, Pairing::HeadToHead);
            assert_eq!(baseline.label.pairing, Pairing::Baseline);
        }
    }

    /// A one-cell manifest over shipped scenarios.
    fn cell(maps: &[&str]) -> (MatrixManifest, Vec<Scenario>) {
        let manifest: MatrixManifest = serde_json::from_value(serde_json::json!({
            "name": "unit",
            "tick_limit": 30,
            "runs": 1,
            "scenario_seed_base": 70,
            "personality_seed_base": 900,
            "difficulties": ["standard"],
            "stances": ["aggressive"],
            "maps": maps
                .iter()
                .map(|map| serde_json::json!({
                    "path": format!("../scenarios/{map}.json"),
                    "family": "open",
                }))
                .collect::<Vec<_>>(),
        }))
        .unwrap();
        let scenarios = manifest
            .scenarios(Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap();
        (manifest, scenarios)
    }

    /// The cell's seats, `true` running `oxide-opponent`.
    fn seats(opponent: &[bool]) -> Vec<Option<EvaluationController>> {
        opponent
            .iter()
            .map(|&opponent| {
                Some(EvaluationController::configured(BotConfig {
                    controller: if opponent {
                        BotController::Opponent
                    } else {
                        BotController::Scripted
                    },
                    difficulty: BotDifficulty::Standard,
                    stance: BotStance::Aggressive,
                    personality_seed: 900,
                }))
            })
            .collect()
    }

    fn flipped(layout: &[bool]) -> Vec<bool> {
        layout.iter().map(|opponent| !opponent).collect()
    }

    fn mixed_layout(source: &Scenario) -> Vec<bool> {
        layouts(source)
            .unwrap()
            .into_iter()
            .find_map(|(pairing, layout)| (pairing == Pairing::Mixed).then_some(layout))
            .unwrap()
    }

    #[test]
    fn team_maps_expand_into_pure_and_mixed_pairs_and_one_baseline_leg() {
        let (manifest, scenarios) = cell(&["broad-front"]);
        let teams = seat_teams(&scenarios[0]).unwrap();
        assert_eq!(teams, [0, 1, 0, 1]);
        let legs = expand(&manifest, &scenarios).unwrap();
        let [pure, pure_back, mixed, mixed_back, baseline] = legs.as_slice() else {
            panic!("five legs: {}", legs.len());
        };
        let pure_layout = [true, false, true, false];
        assert_eq!(pure.plan.controllers, seats(&pure_layout));
        assert_eq!(pure_back.plan.controllers, seats(&flipped(&pure_layout)));
        let layout = mixed_layout(&scenarios[0]);
        assert_eq!(mixed.plan.controllers, seats(&layout));
        assert_eq!(mixed_back.plan.controllers, seats(&flipped(&layout)));
        for team in [0, 1] {
            let bots: Vec<bool> = (0..4)
                .filter(|&seat| teams[seat] == team)
                .map(|seat| layout[seat])
                .collect();
            assert!(bots.contains(&true) && bots.contains(&false), "{bots:?}");
        }
        assert_eq!(baseline.plan.controllers, seats(&[false; 4]));
        assert_eq!(
            legs.iter()
                .map(|leg| (leg.label.pairing, leg.plan.leg))
                .collect::<Vec<_>>(),
            [
                (Pairing::HeadToHead, EvaluationLeg::Forward),
                (Pairing::HeadToHead, EvaluationLeg::Swapped),
                (Pairing::Mixed, EvaluationLeg::Forward),
                (Pairing::Mixed, EvaluationLeg::Swapped),
                (Pairing::Baseline, EvaluationLeg::Single),
            ]
        );
        for leg in &legs {
            assert_eq!(leg.plan.scenario.seed, 70);
            assert!(leg.plan.scenario.players.iter().all(|player| !player.bot));
        }
    }

    #[test]
    fn free_for_alls_seat_the_human_chair_and_alternate_the_bots() {
        let (manifest, scenarios) = cell(&["skyhook-anchorage"]);
        assert!(!scenarios[0].players[0].bot, "seat zero is authored human");
        let legs = expand(&manifest, &scenarios).unwrap();
        let [forward, swapped, baseline] = legs.as_slice() else {
            panic!("three legs: {}", legs.len());
        };
        let layout: Vec<bool> = (0..8).map(|seat| seat % 2 == 0).collect();
        assert_eq!(forward.plan.controllers, seats(&layout));
        assert_eq!(swapped.plan.controllers, seats(&flipped(&layout)));
        assert_eq!(baseline.plan.controllers, seats(&[false; 8]));
        assert_eq!(
            [forward, swapped, baseline].map(|leg| leg.label.pairing),
            [Pairing::Mixed, Pairing::Mixed, Pairing::Baseline]
        );
    }

    #[test]
    fn duel_plans_are_the_configured_matchup_plans() {
        let manifest = manifest();
        let legs = expand(&manifest, &manifest.scenarios(Path::new(".")).unwrap()).unwrap();
        let source = Scenario::skirmish();
        for (pairing, controller, opponent, paired) in [
            (
                Pairing::HeadToHead,
                BotController::Opponent,
                Some(BotController::Scripted),
                true,
            ),
            (Pairing::Baseline, BotController::Scripted, None, false),
        ] {
            let expected = configured_matchup_plans(
                &source,
                70,
                ProfileMatchup {
                    controller,
                    opponent_controller: opponent,
                    difficulty: BotDifficulty::Standard,
                    stance: BotStance::Aggressive,
                    opponent_difficulty: None,
                    opponent_stance: None,
                    same_personality_seed: true,
                },
                900,
                paired,
                EvaluationFactionCell::Authored,
                EvaluationGeometry::Authored,
            )
            .unwrap();
            let actual: Vec<&EvaluationPlan> = legs
                .iter()
                .filter(|leg| leg.label.pairing == pairing && leg.label.run == 0)
                .filter(|leg| leg.label.difficulty == BotDifficulty::Standard)
                .map(|leg| &leg.plan)
                .collect();
            assert_eq!(actual, expected.iter().collect::<Vec<_>>());
        }
    }

    #[test]
    fn mixed_teams_face_the_other_bot_across_each_lane() {
        let (_, scenarios) = cell(&[
            "open-quarry",
            "broad-front",
            "causeway-verdict",
            "compass-grand",
        ]);
        for source in &scenarios {
            let teams = seat_teams(source).unwrap();
            let layout = mixed_layout(source);
            let (_, anchors) = source.parse_map_and_anchors().unwrap();
            let foundry = |seat: usize| {
                anchors
                    .iter()
                    .find(|(player, _)| usize::from(player.0) == seat)
                    .map(|(_, anchor)| (i64::from(anchor.x), i64::from(anchor.y)))
                    .unwrap()
            };
            for seat in 0..teams.len() {
                let (x, y) = foundry(seat);
                let nearest = (0..teams.len())
                    .filter(|&enemy| teams[enemy] != teams[seat])
                    .min_by_key(|&enemy| {
                        let (ex, ey) = foundry(enemy);
                        ((ex - x).pow(2) + (ey - y).pow(2), enemy)
                    })
                    .unwrap();
                assert_ne!(
                    layout[seat], layout[nearest],
                    "{}: seat {seat} and its nearest enemy {nearest} run one bot",
                    source.name
                );
            }
            let count = |opponent| layout.iter().filter(|&&seat| seat == opponent).count();
            assert_eq!(count(true), count(false), "{}", source.name);
        }
    }

    #[test]
    fn match_modes_follow_teams() {
        let cases: [(&[u8], Option<MatchMode>); 8] = [
            (&[0, 1], Some(MatchMode::Duel)),
            (&[0, 1, 2], Some(MatchMode::FreeForAll)),
            (&[0, 0, 1, 1], Some(MatchMode::Teams)),
            (&[0, 1, 0, 1], Some(MatchMode::Teams)),
            (&[0], None),
            (&[0, 0], None),
            (&[0, 0, 0, 1], None),
            (&[0, 0, 1, 2], None),
        ];
        for (teams, mode) in cases {
            assert_eq!(MatchMode::of(teams).ok(), mode, "{teams:?}");
        }
    }

    #[test]
    fn every_shipped_scenario_expands_by_its_mode() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
        let mut names: Vec<String> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let (manifest, scenarios) = cell(&[name.as_str()]);
            let players = &scenarios[0].players;
            let (mode, legs) = if players.len() == 2 {
                (MatchMode::Duel, 3)
            } else if players.iter().all(|player| player.team.is_none()) {
                (MatchMode::FreeForAll, 3)
            } else {
                (MatchMode::Teams, 5)
            };
            let teams = seat_teams(&scenarios[0]).unwrap();
            assert_eq!(MatchMode::of(&teams).unwrap(), mode, "{name}");
            assert_eq!(expand(&manifest, &scenarios).unwrap().len(), legs, "{name}");
        }
    }

    type Edit = fn(&mut MatrixManifest);

    #[test]
    fn invalid_manifests_are_refused() {
        let cases: [(&str, Edit); 7] = [
            ("tick limit", |manifest| manifest.tick_limit = 0),
            ("runs", |manifest| manifest.runs = 0),
            ("overflows", |manifest| {
                manifest.scenario_seed_base = u64::MAX
            }),
            ("difficulties are empty", |manifest| {
                manifest.difficulties.clear()
            }),
            ("stances repeat", |manifest| {
                manifest.stances.push(BotStance::Aggressive)
            }),
            ("maps repeat", |manifest| {
                manifest.maps.push(ManifestMap {
                    path: "../scenarios/skirmish.json".into(),
                    family: MapFamily::Open,
                })
            }),
            ("name", |manifest| manifest.name = "two words".into()),
        ];
        for (expected, edit) in cases {
            let mut manifest = manifest();
            edit(&mut manifest);
            let error = manifest.validate().unwrap_err().to_string();
            assert!(error.contains(expected), "{expected}: {error}");
        }
        let unknown = serde_json::from_value::<MatrixManifest>(serde_json::json!({
            "name": "unit", "tick_limit": 1, "runs": 1, "scenario_seed_base": 0,
            "personality_seed_base": 0, "difficulties": ["prime"], "stances": ["balanced"],
            "maps": [{"path": "skirmish", "family": "open"}], "jobs": 3,
        }));
        assert!(unknown.is_err(), "unknown manifest fields are refused");
    }

    #[test]
    fn shipped_manifests_expand_their_maps_by_mode() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation");
        for (name, mode, legs_per_cell) in [
            ("smoke.json", MatchMode::Duel, 3),
            ("duels.json", MatchMode::Duel, 3),
            ("teams.json", MatchMode::Teams, 5),
            ("free-for-all.json", MatchMode::FreeForAll, 3),
        ] {
            let manifest = MatrixManifest::load(&directory.join(name)).unwrap();
            let scenarios = manifest.scenarios(&directory).unwrap();
            for scenario in &scenarios {
                let teams = seat_teams(scenario).unwrap();
                assert_eq!(MatchMode::of(&teams).unwrap(), mode, "{name}");
            }
            let legs = expand(&manifest, &scenarios).unwrap();
            assert_eq!(
                legs.len(),
                manifest.maps.len()
                    * manifest.difficulties.len()
                    * manifest.stances.len()
                    * manifest.runs as usize
                    * legs_per_cell,
                "{name}"
            );
        }
    }

    #[test]
    fn baseline_rows_are_reused_only_under_the_same_identity() {
        let root = scratch();
        let mut manifest = manifest();
        manifest.runs = 1;
        manifest.difficulties = vec![BotDifficulty::Standard];
        let legs = expand(&manifest, &manifest.scenarios(Path::new(".")).unwrap()).unwrap();
        let options = MatrixOptions {
            candidate: "unit",
            jobs: NonZeroUsize::new(2).unwrap(),
            baseline_cache: &root,
        };
        let first = run_matrix(&legs, 30, &options).unwrap();
        assert_eq!((first.evaluated, first.reused), (3, 0));
        let second = run_matrix(&legs, 30, &options).unwrap();
        assert_eq!((second.evaluated, second.reused), (2, 1));
        assert_eq!(first.rows[2].row, second.rows[2].row);
        let longer = run_matrix(&legs, 31, &options).unwrap();
        assert_eq!((longer.evaluated, longer.reused), (3, 0));

        let cache = BaselineCache::new(&root);
        let baseline = &legs[2].plan;
        let entry = cache
            .entry(baseline, 30, Some(DEFAULT_STALL_LOOP_LIMIT))
            .unwrap();
        let mut forged = first.rows[2].row.clone();
        forged["reference_digest"] = "fnv1a64:0000000000000000".into();
        std::fs::write(&entry.path, serde_json::to_vec(&forged).unwrap()).unwrap();
        assert!(
            cache
                .load(baseline, 30, Some(DEFAULT_STALL_LOOP_LIMIT))
                .unwrap()
                .is_none()
        );
        std::fs::write(&entry.path, b"not json").unwrap();
        assert!(
            cache
                .load(baseline, 30, Some(DEFAULT_STALL_LOOP_LIMIT))
                .unwrap()
                .is_none()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn published_rows_carry_labels_and_never_replace_earlier_rows() {
        let root = scratch();
        let mut manifest = manifest();
        manifest.runs = 1;
        manifest.difficulties = vec![BotDifficulty::Standard];
        let legs = expand(&manifest, &manifest.scenarios(Path::new(".")).unwrap()).unwrap();
        let run = run_matrix(
            &legs,
            12,
            &MatrixOptions {
                candidate: "unit",
                jobs: NonZeroUsize::new(1).unwrap(),
                baseline_cache: &root.join("cache"),
            },
        )
        .unwrap();
        let path = preflight_output(&root.join("out")).unwrap();
        publish_rows(&run.rows, &path).unwrap();
        let rows = load_rows(std::slice::from_ref(&path)).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].matrix, legs[0].label);
        assert_eq!(rows[2].matrix.pairing, Pairing::Baseline);
        assert!(preflight_output(&root.join("out")).is_err());
        assert!(publish_rows(&run.rows, &path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_default_cache_is_per_user() {
        let some = |value: &str| Some(std::ffi::OsString::from(value));
        let leaf = Path::new("oxide").join("bot-matrix-baseline");
        assert_eq!(
            cache_root(some("/xdg"), some("/home"), some("/local")),
            Some(Path::new("/xdg").join(&leaf))
        );
        assert_eq!(
            cache_root(some(""), some("/home"), None),
            Some(Path::new("/home").join(".cache").join(&leaf))
        );
        assert_eq!(
            cache_root(None, None, some("/local")),
            Some(Path::new("/local").join(&leaf))
        );
        assert_eq!(cache_root(None, None, None), None);
    }
}
