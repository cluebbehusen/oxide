#![doc = include_str!("../README.md")]

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use oxide_driver::client::Client;
use oxide_driver::runner;
use oxide_driver::{render, smoke};
use oxide_protocol::{Reply, StateFilter, hash_hex};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "oxide-driver",
    version,
    about = "Headless harness and live-game client for Oxide"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a scenario headless at full speed.
    Run {
        /// Scenario path, or "skirmish" for the built-in map.
        scenario: String,
        /// Ticks to simulate.
        #[arg(long, default_value_t = 2000)]
        ticks: u64,
        /// Let scenario-configured bots play.
        #[arg(long)]
        bots: bool,
        /// Hand every seat to the scripted Balanced AI. This is the
        /// complete-match evaluation path for shipped scenarios whose
        /// first chair is normally human.
        #[arg(long, conflicts_with = "bots")]
        all_bots: bool,
        /// Record and save a replay here.
        #[arg(long)]
        save_replay: Option<PathBuf>,
        /// Also print the ASCII map at the end.
        #[arg(long)]
        map: bool,
    },
    /// Evaluate the player-facing rules bot and emit one compact JSONL row
    /// per exact seed/profile leg. Unlike the frozen Overseer sweeps, this
    /// follows the controller that ships to players and stops at the result.
    BotEval {
        /// Scenario paths, or "skirmish" for the built-in map.
        #[arg(required = true)]
        scenarios: Vec<String>,
        /// Maximum ticks per match.
        #[arg(long, default_value_t = 60_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// Stalls of one reason on one unit that stop a leg as a `stall_loop`
        /// anomaly instead of running it to the ceiling; 0 disables the stop.
        #[arg(long, default_value_t = oxide_driver::bot_eval::DEFAULT_STALL_LOOP_LIMIT)]
        stall_loop_limit: u64,
        /// Consecutive seed cells per scenario for player-facing profile
        /// comparisons.
        #[arg(
            long,
            default_value_t = 1,
            value_parser = clap::value_parser!(u64).range(1..),
            conflicts_with = "against_overseer"
        )]
        runs: u64,
        /// First simulation seed. Defaults to each scenario's authored seed;
        /// subsequent runs increment it.
        #[arg(long, conflicts_with = "scenario_seeds")]
        scenario_seed_base: Option<u64>,
        /// Exact simulation seeds to cross with every personality seed when
        /// comparing against Overseer.
        #[arg(
            long,
            value_delimiter = ',',
            requires = "against_overseer",
            conflicts_with = "scenario_seed_base"
        )]
        scenario_seeds: Vec<u64>,
        /// First personality seed. Seats and subsequent runs receive
        /// deterministic seeds; use `--same-personality-seed` to consume one
        /// shared seed per two-seat run.
        #[arg(long, conflicts_with = "personality_seeds")]
        personality_seed_base: Option<u64>,
        /// Exact player-facing personality seeds to cross with every
        /// simulation seed when comparing against Overseer.
        #[arg(
            long,
            value_delimiter = ',',
            requires = "against_overseer",
            conflicts_with = "personality_seed_base"
        )]
        personality_seeds: Vec<u64>,
        /// Player-facing skill rung.
        #[arg(long, default_value_t = oxide_sim::scenario::BotDifficulty::Standard)]
        difficulty: oxide_sim::scenario::BotDifficulty,
        /// Player-facing strategic posture.
        #[arg(long, default_value_t = oxide_sim::scenario::BotStance::Balanced)]
        stance: oxide_sim::scenario::BotStance,
        /// Difficulty for seat one. Supplying this requires a two-seat
        /// scenario; omit it to use `--difficulty` for both seats.
        #[arg(long)]
        opponent_difficulty: Option<oxide_sim::scenario::BotDifficulty>,
        /// Strategic posture for seat one. Supplying this requires a two-seat
        /// scenario; omit it to use `--stance` for both seats.
        #[arg(long)]
        opponent_stance: Option<oxide_sim::scenario::BotStance>,
        /// Give both seats the same personality seed. This isolates difficulty
        /// when both stances match and requires a two-seat scenario.
        #[arg(long)]
        same_personality_seed: bool,
        /// Compare the player-facing controller with the frozen pre-0.16
        /// Overseer yardstick instead of another player-facing profile.
        #[arg(
            long,
            conflicts_with_all = [
                "opponent_difficulty",
                "opponent_stance",
                "same_personality_seed"
            ]
        )]
        against_overseer: bool,
        /// Seat-independent identity for Overseer's frozen army-size jitter.
        /// Defaults to zero and stays fixed across the full matrix.
        #[arg(long, requires = "against_overseer")]
        overseer_policy_seed: Option<u64>,
        /// Physical-seat faction cells for an Overseer comparison.
        #[arg(long, value_delimiter = ',', requires = "against_overseer")]
        faction_cells: Vec<oxide_driver::bot_eval::EvaluationFactionCell>,
        /// Map-end geometry cells for an Overseer comparison.
        #[arg(long, value_delimiter = ',', requires = "against_overseer")]
        geometries: Vec<oxide_driver::bot_eval::EvaluationGeometry>,
        /// On a two-seat scenario, run a second leg with the two complete
        /// controller configurations exchanged between seats.
        #[arg(long)]
        paired: bool,
        /// Stable candidate or build identifier. Required when evidence is
        /// persisted with `--out` or `--replay-dir`.
        #[arg(long)]
        candidate: Option<String>,
        /// Atomically publish JSONL here instead of standard output. An
        /// existing file is never replaced.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Preserve one replay per leg in this directory without replacing
        /// any existing evidence.
        #[arg(long, requires = "out")]
        replay_dir: Option<PathBuf>,
    },
    /// Re-execute a replay and report (or check) the final hash.
    Replay {
        /// Replay JSON path.
        path: PathBuf,
        /// Override the tick count from the replay metadata.
        #[arg(long)]
        ticks: Option<u64>,
        /// Fail unless the final hash equals this (0x-prefixed hex).
        #[arg(long)]
        expect_hash: Option<String>,
        /// Play a replay recorded on a different sim version anyway
        /// (reproduction not guaranteed — archaeology only).
        #[arg(long)]
        allow_version_mismatch: bool,
        /// Run past the built-in length bound (marathon reproductions).
        #[arg(long)]
        allow_long: bool,
    },
    /// Inspect a replay as stable JSON: metadata, command activity, outcome,
    /// and exact snapshots at selected simulation ticks.
    ReplayInspect {
        /// Replay or save JSON path.
        path: PathBuf,
        /// State ticks to capture, repeatable or comma-separated. Tick N is
        /// before commands stamped N execute. Defaults to the final tick.
        #[arg(long = "tick", value_delimiter = ',')]
        ticks: Vec<u64>,
        /// Also capture the fog-honest view belonging to this seat.
        #[arg(long)]
        fog_seat: Option<u8>,
        /// Include the omniscient ASCII map in every state snapshot.
        #[arg(long)]
        map: bool,
    },
    /// Digest a replay for review without screenshots: an event timeline
    /// (battles, expansions, tech firsts, eliminations, lulls), per-seat
    /// digests at intervals, and a coarse ASCII minimap. Large games read
    /// tighter with `--every 10000` or `--minimaps none`.
    ReplaySummary {
        /// Replay or save JSON path.
        path: PathBuf,
        /// Stop after this state tick (tick N is before commands stamped N
        /// execute); clamped to the replay's recorded duration.
        #[arg(long)]
        until: Option<u64>,
        /// Digest cadence in ticks. Defaults to duration/16, clamped to
        /// [2000, 10000].
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        every: Option<u64>,
        /// Emit JSON instead of the text digest.
        #[arg(long)]
        json: bool,
        /// Minimaps on digests: every digest, sparse (every fourth and the
        /// final), or none.
        #[arg(long, default_value = "sparse", value_parser = ["all", "sparse", "none"])]
        minimaps: String,
    },
    /// Render a scenario state to a PNG (software rasterizer, no window).
    Render {
        /// Scenario path, or "skirmish".
        scenario: String,
        /// Ticks to simulate first.
        #[arg(long, default_value_t = 0)]
        ticks: u64,
        /// Let scenario-configured bots play during those ticks.
        #[arg(long)]
        bots: bool,
        /// Hand every seat to the scripted Balanced AI before rendering.
        #[arg(long, conflicts_with = "bots")]
        all_bots: bool,
        /// Output PNG path.
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Decisiveness seed sweep: N seeds of Overseer-vs-Overseer on one
    /// 1v1 map. Measures endings and seat lean.
    Sweep {
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Seeds (one match per seed).
        #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick cap per match.
        #[arg(long, default_value_t = 40_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// First scenario seed; offsets count up from here.
        #[arg(long, default_value_t = 7_000)]
        seed_base: u64,
        /// Raw JSON output path.
        #[arg(long)]
        out: Option<String>,
    },
    /// Empirical pace measurement: the decisiveness sweep run over every
    /// 1v1 map in a directory, tabling measured decision-tick quartiles
    /// (ticks and clock) beside the geometric `pace` label and the
    /// audited ground route. Measurement only — nothing gates on it.
    PaceSweep {
        /// Scenario directory to sweep (other formats are skipped).
        #[arg(long, default_value = "scenarios")]
        dir: String,
        /// Seeds per map (one Overseer-vs-Overseer match per seed).
        #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick cap per match; every map's slowest tail must fit under
        /// it or its quantiles read censored.
        #[arg(long, default_value_t = 40_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// First scenario seed; offsets count up from here.
        #[arg(long, default_value_t = 7_000)]
        seed_base: u64,
        /// Raw JSON output path.
        #[arg(long)]
        out: Option<String>,
    },
    /// Factorial fairness probe: every advantage the game binds to the
    /// seat index — roster, geometry, id range, command order —
    /// permuted as a full cross product on one seed set with the
    /// Overseer in both chairs. Reports per-factor marginals with
    /// Wilson intervals and the whole cell table, because the
    /// interactions are the finding.
    SweepFactorial {
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Factors in the design, comma-separated (default: all of
        /// faction, spawn, command, geometry).
        #[arg(long)]
        factors: Option<String>,
        /// Seeds per cell.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick cap per match.
        #[arg(long, default_value_t = 40_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// First scenario seed; offsets count up from here.
        #[arg(long, default_value_t = 7_000)]
        seed_base: u64,
        /// Raw JSON output path.
        #[arg(long)]
        out: Option<String>,
    },
    /// Timed mass-battle bench: ticks/second at scale, plus a hash
    /// self-check. Wall-clock stays local; CI asserts only correctness.
    Bench {
        /// Units per side.
        #[arg(long, default_value_t = 250)]
        units: u32,
        /// Ticks to run.
        #[arg(long, default_value_t = 2_000)]
        ticks: u32,
        /// Bench a shipped scenario with the Overseer thinking in every
        /// chair instead of the synthetic mass battle (e.g.
        /// "scenarios/compass-grand.json" — eight scripted minds).
        #[arg(long)]
        scenario: Option<String>,
    },
    /// Paired, seat-neutral arena duel between two hand-picked armies
    /// (no economy): the balance review's controlled experiment.
    Matchup {
        /// Side A, as "kind:count,kind:count".
        #[arg(long)]
        a: String,
        /// Side B, same shape.
        #[arg(long)]
        b: String,
        /// Pre-built structures for side B, as "kind:count" (defense
        /// mode: the swarm-vs-fortification experiment).
        #[arg(long)]
        b_structures: Option<String>,
        /// Seat rosters, west then east: ff, cc, fc or cf. Both seats
        /// wear one roster by default, so a leg swap exchanges seat,
        /// geometry and ID range and nothing else.
        #[arg(long, default_value = "ff", value_parser = oxide_kit::matchup::parse_factions)]
        factions: oxide_kit::matchup::SeatFactions,
        /// Tile spacing of the garrison grid (must clear the widest
        /// structure standing in it).
        #[arg(long, default_value_t = 3)]
        garrison_pitch: i32,
    },
    /// Measure a map: room per seat, route lengths by domain, resources,
    /// artillery pressure, spawn spacing.
    MapAudit {
        /// Scenario path, or "skirmish".
        scenario: String,
        /// Emit JSON instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Recompute match statistics from a replay (scrap and army-value
    /// series, losses) — the record is the match.
    ReplayStats {
        /// Replay JSON path.
        path: PathBuf,
        /// Sample stride in ticks.
        #[arg(long, default_value_t = 200)]
        every: u64,
    },
    /// Talk to a running shell (`oxide-shell --debug-server`).
    Live {
        /// Shell debug-server address.
        #[arg(long, default_value = "127.0.0.1:4123")]
        addr: String,
        #[command(subcommand)]
        cmd: LiveCmd,
    },
    /// Resume a record prefix and profile a live Playing interval in a
    /// temporary real GPU-backed shell.
    ProfileShell {
        /// Replay or save JSON whose prefix becomes the live match.
        replay: PathBuf,
        /// First tick included in the measured window.
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// Tick at which the harness pauses and reports.
        #[arg(long)]
        to: u64,
        /// Live wall-clock speed multiplier.
        #[arg(long, default_value_t = 8.0)]
        speed: f64,
        /// Debug-server port for the temporary shell.
        #[arg(long, default_value_t = 4198)]
        port: u16,
        /// Profile an unoptimized development shell instead of release.
        #[arg(long)]
        dev: bool,
    },
    /// Serve the debug protocol windowless: a persistent headless match
    /// (no GPU, no wall clock — always driven mode) that every
    /// `driver live` verb can drive. Screenshots are CPU schematic
    /// renders of the whole map.
    Session {
        /// TCP port to listen on (the shell's default, so `driver live`
        /// needs no --addr when only one server runs).
        #[arg(long, default_value_t = oxide_protocol::DEFAULT_PORT)]
        port: u16,
        /// Starting scenario path, or "skirmish" for the built-in map.
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Seconds a silent connection may sit before it is closed.
        #[arg(long, default_value_t = 30 * 60, value_parser = clap::value_parser!(u64).range(1..))]
        idle_timeout: u64,
    },
    /// Automated end-to-end check against a live shell.
    Smoke {
        /// Shell debug-server address.
        #[arg(long, default_value = "127.0.0.1:4123")]
        addr: String,
        /// Build and spawn an isolated shell first, then kill it after.
        #[arg(long)]
        spawn: bool,
    },
    /// Perceptual-diff screenshot suite: twelve canonical screens from a
    /// spawned automation shell, compared against per-machine
    /// references (gitignored — a local gate, never CI).
    Shots {
        /// Debug-server port for the spawned shell.
        #[arg(long, default_value_t = 4151)]
        port: u16,
        /// Adopt the current captures as the new references.
        #[arg(long)]
        bless: bool,
        /// Reference/run directory.
        #[arg(long, default_value = "shots")]
        dir: PathBuf,
        /// Mean per-channel difference tolerated, in percent. The default
        /// is calibrated: font AA jitter measures <= 0.003% run to run,
        /// while a small UI element appearing or vanishing measures
        /// ~0.02% — the gate must sit between them.
        #[arg(long, default_value_t = 0.01)]
        threshold: f64,
    },
}

mod live_cli;
mod parse;

use live_cli::{LiveCmd, capture_sequence, live_requests};

/// Per-tick latency digest for bench output: mean, median, tail, and the
/// worst tick with its index — the spike a throughput mean cannot see.
fn latency_summary(samples_ns: &[u64]) -> String {
    if samples_ns.is_empty() {
        return "no samples".to_string();
    }
    let mut sorted = samples_ns.to_vec();
    sorted.sort_unstable();
    let ms = |ns: u64| ns as f64 / 1_000_000.0;
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    let mean = samples_ns.iter().sum::<u64>() / samples_ns.len() as u64;
    let (worst_tick, worst) = samples_ns
        .iter()
        .enumerate()
        .max_by_key(|(index, ns)| (**ns, std::cmp::Reverse(*index)))
        .expect("nonempty samples");
    format!(
        "mean {:.3}ms p50 {:.3} p99 {:.3} max {:.2} @tick {}",
        ms(mean),
        ms(at(0.5)),
        ms(at(0.99)),
        ms(*worst),
        worst_tick,
    )
}

fn ensure_distinct<T: PartialEq>(values: &[T], label: &str) -> Result<()> {
    for (index, value) in values.iter().enumerate() {
        if values[..index].contains(value) {
            bail!("{label} contains a duplicate value");
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Run {
            scenario,
            ticks,
            bots,
            all_bots,
            save_replay,
            map,
        } => {
            let mut scenario = runner::load_scenario(&scenario)?;
            if all_bots {
                oxide_kit::bench::all_bots(&mut scenario);
            }
            let outcome =
                runner::run_scenario(&scenario, ticks, bots || all_bots, save_replay.is_some())?;
            if let (Some(path), Some(replay)) = (&save_replay, &outcome.replay) {
                replay.save(path)?;
                eprintln!(
                    "replay: {} ({} commands)",
                    path.display(),
                    replay.commands.len()
                );
            }
            let filter = StateFilter {
                map,
                ..StateFilter::default()
            };
            let view = oxide_protocol::StateView::capture(&outcome.state, filter);
            println!("{}", serde_json::to_string_pretty(&view)?);
        }
        Cmd::BotEval {
            scenarios,
            ticks,
            stall_loop_limit,
            runs,
            scenario_seed_base,
            scenario_seeds,
            personality_seed_base,
            personality_seeds,
            difficulty,
            stance,
            opponent_difficulty,
            opponent_stance,
            same_personality_seed,
            against_overseer,
            overseer_policy_seed,
            faction_cells,
            geometries,
            paired,
            candidate,
            out,
            replay_dir,
        } => {
            let stall_loop_limit = (stall_loop_limit > 0).then_some(stall_loop_limit);
            let candidate = match candidate {
                Some(candidate) => candidate,
                None if out.is_some() || replay_dir.is_some() => {
                    bail!("--candidate is required with --out or --replay-dir")
                }
                None => "ad-hoc".to_string(),
            };
            let matchup = oxide_driver::bot_eval::ProfileMatchup {
                difficulty,
                stance,
                opponent_difficulty,
                opponent_stance,
                same_personality_seed,
            };
            ensure_distinct(&scenario_seeds, "--scenario-seeds")?;
            ensure_distinct(&personality_seeds, "--personality-seeds")?;
            ensure_distinct(&faction_cells, "--faction-cells")?;
            ensure_distinct(&geometries, "--geometries")?;
            let mut plans = Vec::new();
            for (scenario_index, scenario_name) in scenarios.iter().enumerate() {
                let source = runner::load_scenario(scenario_name)?;
                if against_overseer {
                    oxide_driver::bot_eval::ensure_overseer_yardstick_ground(&source)?;
                    let overseer_policy_seed = overseer_policy_seed.unwrap_or(0);
                    let scenario_seed_values = if scenario_seeds.is_empty() {
                        vec![scenario_seed_base.unwrap_or(source.seed)]
                    } else {
                        scenario_seeds.clone()
                    };
                    let personality_seed_values = if personality_seeds.is_empty() {
                        vec![personality_seed_base.unwrap_or(0)]
                    } else {
                        personality_seeds.clone()
                    };
                    let faction_cells = if faction_cells.is_empty() {
                        vec![oxide_driver::bot_eval::EvaluationFactionCell::Authored]
                    } else {
                        faction_cells.clone()
                    };
                    let geometries = if geometries.is_empty() {
                        vec![oxide_driver::bot_eval::EvaluationGeometry::Authored]
                    } else {
                        geometries.clone()
                    };

                    let mut seed_cell = 0_u64;
                    for &scenario_seed in &scenario_seed_values {
                        for &personality_seed in &personality_seed_values {
                            let config = oxide_sim::scenario::BotConfig::scripted(
                                difficulty,
                                stance,
                                personality_seed,
                            );
                            for &faction_cell in &faction_cells {
                                for &geometry in &geometries {
                                    for plan in oxide_driver::bot_eval::configured_overseer_plans(
                                        &source,
                                        scenario_seed,
                                        config,
                                        overseer_policy_seed,
                                        paired,
                                        faction_cell,
                                        geometry,
                                    )? {
                                        let replay_path = replay_dir
                                            .as_ref()
                                            .map(|dir| {
                                                oxide_driver::bot_eval::evaluation_replay_filename(
                                                    scenario_index,
                                                    seed_cell,
                                                    scenario_seed,
                                                    ticks,
                                                    &candidate,
                                                    &plan,
                                                )
                                                .map(|filename| dir.join(filename))
                                            })
                                            .transpose()?;
                                        plan.scenario.build().with_context(|| {
                                            format!(
                                                "prevalidating bot evaluation scenario {scenario_name} seed cell {seed_cell} {}",
                                                plan.leg.name()
                                            )
                                        })?;
                                        plans.push((plan, replay_path));
                                    }
                                }
                            }
                            seed_cell = seed_cell
                                .checked_add(1)
                                .context("evaluation seed-cell index overflows u64")?;
                        }
                    }
                } else {
                    let seed_base = scenario_seed_base.unwrap_or(source.seed);
                    let personality_seed_base = personality_seed_base.unwrap_or(0);
                    for run in 0..runs {
                        let scenario_seed = seed_base
                            .checked_add(run)
                            .context("scenario seed range overflows u64")?;
                        let profile_base = matchup.personality_seed_base_for_run(
                            personality_seed_base,
                            run,
                            source.players.len(),
                        )?;
                        for (leg, scenario) in oxide_driver::bot_eval::configured_matchup_legs(
                            &source,
                            scenario_seed,
                            matchup,
                            profile_base,
                            paired,
                        )? {
                            let plan = oxide_driver::bot_eval::EvaluationPlan::from_scenario(
                                scenario, leg,
                            );
                            let replay_path = replay_dir
                                .as_ref()
                                .map(|dir| {
                                    oxide_driver::bot_eval::evaluation_replay_filename(
                                        scenario_index,
                                        run,
                                        scenario_seed,
                                        ticks,
                                        &candidate,
                                        &plan,
                                    )
                                    .map(|filename| dir.join(filename))
                                })
                                .transpose()?;
                            plan.scenario.build().with_context(|| {
                                format!(
                                    "prevalidating bot evaluation scenario {scenario_name} run {run} {}",
                                    plan.leg.name()
                                )
                            })?;
                            plans.push((plan, replay_path));
                        }
                    }
                }
            }

            oxide_driver::bot_eval::ensure_unique_execution_plans(
                plans.iter().map(|(plan, _)| plan),
            )?;

            let mut destinations: Vec<PathBuf> = plans
                .iter()
                .filter_map(|(_, replay_path)| replay_path.clone())
                .collect();
            destinations.extend(out.iter().cloned());
            oxide_driver::bot_eval::preflight_destinations(&destinations)?;

            let mut rows = Vec::with_capacity(plans.len());
            let mut evidence = oxide_driver::bot_eval::EvidenceBatch::default();
            for (plan, replay_path) in plans {
                let (mut row, replay) = oxide_driver::bot_eval::evaluate_plan_artifact_with(
                    &plan,
                    ticks,
                    stall_loop_limit,
                    &candidate,
                )?;
                if let Some(path) = replay_path {
                    evidence.stage_replay(&replay, &path)?;
                    row.replay = Some(path.display().to_string());
                }
                rows.push(row);
            }
            if let Some(path) = out.as_deref() {
                evidence.stage_jsonl(&rows, path)?;
            }
            evidence.publish()?;

            if let Some(path) = out {
                eprintln!(
                    "wrote {} bot evaluation rows to {}",
                    rows.len(),
                    path.display()
                );
            } else {
                for row in &rows {
                    println!("{}", serde_json::to_string(row)?);
                }
            }
        }
        Cmd::Replay {
            path,
            ticks,
            expect_hash,
            allow_version_mismatch,
            allow_long,
        } => {
            let replay = oxide_kit::load_replay(&path)?;
            let state =
                runner::run_replay_bounded(&replay, ticks, allow_version_mismatch, allow_long)?;
            let hash = hash_hex(state.hash());
            println!(
                "{}",
                serde_json::json!({
                    "tick": state.current_tick(),
                    "hash": hash,
                    "result": state.result(),
                    "commands": replay.commands.len(),
                })
            );
            if let Some(expected) = expect_hash
                && expected != hash
            {
                bail!("hash mismatch: expected {expected}, got {hash}");
            }
        }
        Cmd::ReplayInspect {
            path,
            ticks,
            fog_seat,
            map,
        } => {
            let replay = oxide_kit::load_replay(&path)
                .with_context(|| format!("loading {}", path.display()))?;
            let inspection = oxide_driver::replay_inspect::inspect(
                &replay,
                &ticks,
                fog_seat.map(oxide_sim::PlayerId),
                map,
            )?;
            println!("{}", serde_json::to_string_pretty(&inspection)?);
        }
        Cmd::ReplaySummary {
            path,
            until,
            every,
            json,
            minimaps,
        } => {
            let replay = oxide_kit::load_replay(&path)
                .with_context(|| format!("loading {}", path.display()))?;
            let opts = oxide_driver::replay_summary::SummaryOptions {
                until,
                every,
                minimaps: match minimaps.as_str() {
                    "all" => oxide_driver::replay_summary::MinimapMode::All,
                    "none" => oxide_driver::replay_summary::MinimapMode::None,
                    _ => oxide_driver::replay_summary::MinimapMode::Sparse,
                },
            };
            let report = oxide_driver::replay_summary::summarize(&replay, &opts)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render());
            }
        }
        Cmd::Render {
            scenario,
            ticks,
            bots,
            all_bots,
            out,
        } => {
            let mut scenario = runner::load_scenario(&scenario)?;
            if all_bots {
                oxide_kit::bench::all_bots(&mut scenario);
            }
            let outcome = runner::run_scenario(&scenario, ticks, bots || all_bots, false)?;
            render::save_png(&outcome.state, &out)?;
            eprintln!("wrote {}", out.display());
        }
        Cmd::Sweep {
            scenario,
            seeds,
            ticks,
            seed_base,
            out,
        } => {
            oxide_driver::sweep::sweep_report(&scenario, seeds, ticks, seed_base, out.as_deref())?;
        }
        Cmd::PaceSweep {
            dir,
            seeds,
            ticks,
            seed_base,
            out,
        } => {
            oxide_driver::pace::pace_sweep_report(&dir, seeds, ticks, seed_base, out.as_deref())?;
        }
        Cmd::SweepFactorial {
            scenario,
            factors,
            seeds,
            ticks,
            seed_base,
            out,
        } => {
            use oxide_driver::factorial::Factor;
            let enabled: Vec<Factor> = match factors.as_deref() {
                Some(list) => list
                    .split(',')
                    .map(|key| Factor::parse(key.trim()))
                    .collect::<anyhow::Result<_>>()?,
                None => Factor::ALL.to_vec(),
            };
            oxide_driver::factorial::factorial_report(
                &scenario,
                &enabled,
                seeds,
                ticks,
                seed_base,
                out.as_deref(),
            )?;
        }
        Cmd::Bench {
            units,
            ticks,
            scenario,
        } => {
            if let Some(path) = scenario {
                // Full-session bench: every seat thinks — the heaviest
                // honest shape (eight Overseer minds on the 4v4 map),
                // deciding whether a perf window is needed. Shipped
                // playable maps author a human seat, so every chair is
                // converted first; benching around an idle seat 0
                // under-measured the claim. The stable Overseer keeps
                // this performance fixture independent of bot tuning.
                let mut sc = runner::load_scenario(&path)?;
                oxide_kit::bench::all_bots(&mut sc);
                let mut state = sc.build()?;
                let mut bots = oxide_kit::bench::overseer_bots(&sc);
                // The timed loop stops at the decision: post-victory
                // ticks simulate a world with nothing left to decide
                // and average as free work, so a long --ticks quietly
                // inflated ticks/s (the once-recorded 25k+ figures).
                let start = std::time::Instant::now();
                let mut ran: u64 = 0;
                let mut tick_ns: Vec<u64> = Vec::with_capacity(ticks as usize);
                let mut bot_ns: Vec<u64> = Vec::with_capacity(ticks as usize);
                for _ in 0..ticks {
                    if state.result().is_some() {
                        break;
                    }
                    let tick_start = std::time::Instant::now();
                    let mut commands = Vec::new();
                    for bot in &mut bots {
                        commands.extend(bot.act(&state));
                    }
                    let bots_done = std::time::Instant::now();
                    state.tick(&commands);
                    bot_ns.push((bots_done - tick_start).as_nanos() as u64);
                    tick_ns.push(tick_start.elapsed().as_nanos() as u64);
                    ran += 1;
                }
                let secs = start.elapsed().as_secs_f64();
                let decided = if state.result().is_some() {
                    format!(" — decided at tick {}", state.current_tick())
                } else {
                    String::new()
                };
                println!(
                    "bench: {} ({} bot seats) x {ran} of {ticks} requested ticks in {secs:.2}s \
                     = {:.0} ticks/s{decided} (hash {:#x})",
                    sc.name,
                    bots.len(),
                    (ran as f64) / secs,
                    state.hash()
                );
                println!(
                    "bench-latency: whole tick {} | bot phase {}",
                    latency_summary(&tick_ns),
                    latency_summary(&bot_ns),
                );
                return Ok(());
            }
            let scenario = oxide_kit::bench::mass_battle(units, 9);
            let mut state = scenario.build()?;
            oxide_kit::bench::engage(&mut state);
            let start = std::time::Instant::now();
            let mut ran: u64 = 0;
            let mut tick_ns: Vec<u64> = Vec::with_capacity(ticks as usize);
            for _ in 0..ticks {
                if state.result().is_some() {
                    break;
                }
                let tick_start = std::time::Instant::now();
                state.tick(&[]);
                tick_ns.push(tick_start.elapsed().as_nanos() as u64);
                ran += 1;
            }
            let secs = start.elapsed().as_secs_f64();
            let decided = if state.result().is_some() {
                format!(" — decided at tick {}", state.current_tick())
            } else {
                String::new()
            };
            println!(
                "bench: {} units x {ran} of {ticks} requested ticks in {secs:.2}s \
                 = {:.0} ticks/s{decided} (hash {:#x})",
                units * 2,
                (ran as f64) / secs,
                state.hash(),
            );
            println!("bench-latency: whole tick {}", latency_summary(&tick_ns));
        }
        Cmd::Matchup {
            a,
            b,
            b_structures,
            factions,
            garrison_pitch,
        } => {
            let army_a = oxide_kit::matchup::parse_army(&a)?;
            let army_b = if b.trim().is_empty() {
                Vec::new()
            } else {
                oxide_kit::matchup::parse_army(&b)?
            };
            let garrison = match &b_structures {
                Some(spec) => oxide_kit::matchup::parse_garrison(spec)?,
                None => Vec::new(),
            };
            print!(
                "A = {a} ({} scrap)   B = {b} ({} scrap)",
                oxide_kit::matchup::army_cost(&army_a),
                oxide_kit::matchup::army_cost(&army_b),
            );
            if let Some(spec) = &b_structures {
                print!(
                    "  + garrison {spec} ({} scrap, pitch {garrison_pitch})",
                    oxide_kit::matchup::garrison_cost(&garrison)
                );
            }
            println!();
            println!("seats: {factions}");
            let arena = oxide_kit::matchup::Arena {
                factions,
                garrison_pitch,
                ..oxide_kit::matchup::Arena::default()
            };
            let out = oxide_kit::matchup::siege(&army_a, &army_b, &garrison, &arena)?;
            for leg in out.legs() {
                let verdict = leg
                    .verdict()
                    .map_or_else(|| "unresolved".to_string(), |v| v.to_string());
                println!(
                    "  A as player {} / B as player {}: A survives {:>4}  B survives {:>4}  \
                     [hp-weighted A {:>4}  B {:>4}]  ({} ticks, {}, verdict {})",
                    leg.a_player,
                    1 - leg.a_player,
                    leg.a_value,
                    leg.b_value,
                    leg.a_hp_value,
                    leg.b_hp_value,
                    leg.ticks,
                    leg.termination,
                    verdict,
                );
            }
            let verdict = out
                .verdict()
                .map_or_else(|| "unresolved".to_string(), |v| v.to_string());
            let flips = match out.verdict_flips_on_swap() {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unresolved",
            };
            println!(
                "paired mean surviving purchase value  A {:.1}  B {:.1}  \
                 (verdict {}, verdict flips on swap {})",
                out.a_mean_value(),
                out.b_mean_value(),
                verdict,
                flips,
            );
            println!(
                "paired mean hp-weighted surviving value  A {:.1}  B {:.1}",
                out.a_mean_hp_value(),
                out.b_mean_hp_value(),
            );
        }
        Cmd::MapAudit { scenario, json } => {
            let scenario = runner::load_scenario(&scenario)?;
            let audit = oxide_driver::audit::audit(&scenario)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&audit)?);
            } else {
                print!("{}", audit.table());
            }
        }
        Cmd::ReplayStats { path, every } => {
            let replay = oxide_kit::load_replay(&path)
                .with_context(|| format!("loading {}", path.display()))?;
            let stats = oxide_driver::stats::compute(&replay, every)?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        Cmd::Live { addr, cmd } => {
            if let LiveCmd::CaptureSequence {
                frames,
                ticks_between,
                present,
                out,
            } = cmd
            {
                return capture_sequence(&addr, frames, ticks_between, present, &out);
            }
            // Parse everything before touching the socket: a typo'd tile
            // should fail fast, not after connecting to a live game.
            let requests = live_requests(cmd)?;
            let mut client = Client::connect(&addr)?;
            let mut last: Option<Reply> = None;
            for request in requests {
                last = Some(client.call(request)?);
            }
            if let Some(reply) = last {
                println!("{}", serde_json::to_string_pretty(&reply)?);
            }
        }
        Cmd::ProfileShell {
            replay,
            from,
            to,
            speed,
            port,
            dev,
        } => {
            let report = oxide_driver::profile::run(&oxide_driver::profile::ProfileOptions {
                replay: &replay,
                from,
                to,
                speed,
                port,
                dev,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::Session {
            port,
            scenario,
            idle_timeout,
        } => oxide_driver::session::serve(
            port,
            &scenario,
            std::time::Duration::from_secs(idle_timeout),
        )?,
        Cmd::Smoke { addr, spawn } => smoke::run(&addr, spawn)?,
        Cmd::Shots {
            port,
            bless,
            dir,
            threshold,
        } => oxide_driver::shots::run(port, bless, &dir, threshold)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_bots_is_an_explicit_complete_match_mode() {
        let cli = Cli::try_parse_from(["oxide-driver", "run", "skirmish", "--all-bots"])
            .expect("all-bots run parses");
        let Cmd::Run { bots, all_bots, .. } = cli.cmd else {
            panic!("run parsed as another command")
        };
        assert!(!bots);
        assert!(all_bots);
        assert!(
            Cli::try_parse_from(["oxide-driver", "run", "skirmish", "--bots", "--all-bots",])
                .is_err(),
            "scenario-configured and all-seat modes are mutually exclusive"
        );
    }

    #[test]
    fn bot_eval_parses_exact_profile_and_paired_seed_cell() {
        let cli = Cli::try_parse_from([
            "oxide-driver",
            "bot-eval",
            "skirmish",
            "scenarios/powder-keg.json",
            "--ticks",
            "50000",
            "--scenario-seed-base",
            "71",
            "--personality-seed-base",
            "900",
            "--difficulty",
            "prime",
            "--stance",
            "aggressive",
            "--paired",
            "--candidate",
            "build-a",
        ])
        .expect("bot evaluation parses");
        let Cmd::BotEval {
            scenarios,
            ticks,
            runs,
            scenario_seed_base,
            personality_seed_base,
            difficulty,
            stance,
            opponent_difficulty,
            opponent_stance,
            same_personality_seed,
            paired,
            candidate,
            ..
        } = cli.cmd
        else {
            panic!("bot-eval parsed as another command")
        };
        assert_eq!(scenarios, ["skirmish", "scenarios/powder-keg.json"]);
        assert_eq!((ticks, runs), (50_000, 1));
        assert_eq!(scenario_seed_base, Some(71));
        assert_eq!(personality_seed_base, Some(900));
        assert_eq!(difficulty, oxide_sim::scenario::BotDifficulty::Prime);
        assert_eq!(stance, oxide_sim::scenario::BotStance::Aggressive);
        assert_eq!(opponent_difficulty, None);
        assert_eq!(opponent_stance, None);
        assert!(!same_personality_seed);
        assert!(paired);
        assert_eq!(candidate.as_deref(), Some("build-a"));
    }

    #[test]
    fn bot_eval_parses_cross_difficulty_shared_personality_comparison() {
        let cli = Cli::try_parse_from([
            "oxide-driver",
            "bot-eval",
            "skirmish",
            "--difficulty",
            "prime",
            "--opponent-difficulty",
            "standard",
            "--opponent-stance",
            "turtle",
            "--same-personality-seed",
            "--paired",
        ])
        .expect("cross-difficulty bot evaluation parses");
        let Cmd::BotEval {
            difficulty,
            stance,
            opponent_difficulty,
            opponent_stance,
            same_personality_seed,
            paired,
            ..
        } = cli.cmd
        else {
            panic!("bot-eval parsed as another command")
        };
        assert_eq!(difficulty, oxide_sim::scenario::BotDifficulty::Prime);
        assert_eq!(stance, oxide_sim::scenario::BotStance::Balanced);
        assert_eq!(
            opponent_difficulty,
            Some(oxide_sim::scenario::BotDifficulty::Standard)
        );
        assert_eq!(
            opponent_stance,
            Some(oxide_sim::scenario::BotStance::Turtle)
        );
        assert!(same_personality_seed);
        assert!(paired);
    }

    #[test]
    fn profile_shell_parses_an_exact_replay_window() {
        let cli = Cli::try_parse_from([
            "oxide-driver",
            "profile-shell",
            "match.json",
            "--from",
            "4500",
            "--to",
            "5750",
            "--speed",
            "8",
        ])
        .expect("profile command parses");
        let Cmd::ProfileShell {
            replay,
            from,
            to,
            speed,
            dev,
            ..
        } = cli.cmd
        else {
            panic!("profile-shell parsed as another command")
        };
        assert_eq!(replay, PathBuf::from("match.json"));
        assert_eq!((from, to), (4500, 5750));
        assert_eq!(speed, 8.0);
        assert!(!dev);
    }
}
