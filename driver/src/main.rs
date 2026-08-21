//! The `oxide-driver` CLI. Run with `--help` for the full tree; AGENTS.md
//! has the guided tour.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use oxide_driver::client::Client;
use oxide_driver::runner::{self, GameReplay};
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
        /// Let scenario-flagged bots play, driven by the shipped actor.
        #[arg(long)]
        bots: bool,
        /// Record and save a replay here.
        #[arg(long)]
        save_replay: Option<PathBuf>,
        /// Also print the ASCII map at the end.
        #[arg(long)]
        map: bool,
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
        /// Let scenario-flagged bots play during those ticks, driven by
        /// the shipped actor.
        #[arg(long)]
        bots: bool,
        /// Output PNG path.
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Bot-vs-bot composition probe across the shipped maps: what the
    /// armies were made of, cost-weighted, with a spam-detecting
    /// entropy — the balance review's measuring stick.
    BalanceProbe {
        /// Scenario directory.
        #[arg(long, default_value = "scenarios")]
        dir: String,
        /// Ladder level to probe ("easy".."expert").
        #[arg(long, default_value = "medium")]
        level: String,
        /// Raw skill-conditioning override 0-1000 (candidate --weights
        /// probes only). Omission keeps the resolved named profile.
        #[arg(long)]
        skill: Option<u32>,
        /// Raw aggression override 0-1000. Omission uses the same
        /// named style, variant, and team role as a shipped match.
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1000))]
        aggression: Option<u32>,
        /// Fix the named style ("turtle", "balanced", or "aggressive").
        /// Omission keeps the deterministic scenario-seed deal.
        #[arg(
            long,
            value_parser = ["turtle", "balanced", "aggressive"],
            conflicts_with_all = ["skill", "aggression", "blunder"]
        )]
        style: Option<String>,
        /// Fix the curated variant within --style (0, 1, or 2).
        /// Omission keeps the deterministic named-variant deal.
        #[arg(
            long,
            requires = "style",
            value_parser = clap::value_parser!(u8).range(0..=2)
        )]
        variant: Option<u8>,
        /// Exact hesitation rate per mille (candidate probes only).
        /// Supplying zero explicitly means no hesitation; omission uses
        /// the named level's handicap.
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1000))]
        blunder: Option<u32>,
        /// Think cadence in ticks (candidate probes only; defaults to
        /// the probed level's own cadence so a candidate measures the
        /// profile it would actually ship at).
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        cadence: Option<u64>,
        /// Seeds per map.
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick cap per match.
        #[arg(long, default_value_t = 20_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// Candidate weights JSON. Omission probes the scripted
        /// Overseer instead of a policy artifact.
        #[arg(long)]
        weights: Option<String>,
        /// Raw JSON output path.
        #[arg(long)]
        out: Option<String>,
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
        /// Tick cap per match (the 0.11 probes read at 40k).
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
        /// "scenarios/compass-grand.json" — eight scripted minds, the
        /// heaviest honest shape until the retrained actor ships).
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
    /// Serve training episodes over stdio (newline-delimited JSON).
    Gym,
    /// Tournament a quantized policy artifact against the Overseer and
    /// the rush canary (the gate measures the shipped integer bot).
    NeuralCup {
        /// Exported weights JSON (tools/train/export.py).
        #[arg(long)]
        weights: PathBuf,
        /// Seeds per matchup (each played from both seats).
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick cap for each tournament game.
        #[arg(long, default_value_t = 40_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// Raw decision-cadence override. Omission uses Expert's named
        /// cadence while keeping the candidate on the ladder profile.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        cadence: Option<u64>,
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Exact hesitation rate per mille. Supplying this, including
        /// zero, selects a raw profile; omission uses Expert's handicap.
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1000))]
        blunder: Option<u32>,
        /// Raw skill conditioning 0-1000. Supplying it selects a raw
        /// profile; omission uses the measured strategy condition.
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1000))]
        skill: Option<u32>,
        /// Raw aggression conditioning input. Omission exercises the
        /// deterministic canonical named-profile slate.
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1000))]
        aggression: Option<u32>,
        /// Override both duel rosters in west/east order (ff|fc|cf|cc).
        /// Omission preserves the scenario's authored factions.
        #[arg(long)]
        factions: Option<oxide_driver::gym::DuelFactions>,
    },
    /// Endgame diagnostic: a dominant army against a bare remnant, with
    /// and without intel of its base. Measures whether the shipped
    /// actor can finish a won game. Diagnostic only.
    CloseoutProbe {
        /// Exported weights JSON (tools/train/export.py).
        #[arg(long)]
        weights: PathBuf,
        /// Seeds per variant.
        #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick horizon per fixture.
        #[arg(long, default_value_t = 20_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// Exact hesitation per mille (0 = Expert clean).
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=1000))]
        blunder: u32,
        /// Think cadence in ticks.
        #[arg(long, default_value_t = 34, value_parser = clap::value_parser!(u64).range(1..))]
        cadence: u64,
    },
    /// Forced-doctrine A/B: the same policy plays itself with one seat
    /// compelled to keep a quota of the probed kind, separating
    /// "overpriced" from "never learned". Diagnostic only.
    ViabilityProbe {
        /// Exported weights JSON (tools/train/export.py).
        #[arg(long)]
        weights: PathBuf,
        /// Seeds per action (each played from both seats).
        #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick cap for each probed game.
        #[arg(long, default_value_t = 40_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Kinds of the probed unit or structure the doctrine keeps
        /// pressing toward while below this count.
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..))]
        quota: u32,
        /// Tick the doctrine wakes on. Forcing from tick zero measures
        /// "should you rush X", not composition viability.
        #[arg(long, default_value_t = 3_000)]
        start_tick: u64,
        /// One probe action by CLI name (e.g. "skyhook", "bastion").
        /// Omission sweeps the full train/build roster.
        #[arg(long)]
        action: Option<String>,
        /// Also write the JSON report here.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Exercise a candidate's repair verbs in deterministic wounded-state
    /// fixtures. Diagnostic only: observed, never rewarded.
    RepairProbe {
        /// Exported weights JSON (tools/train/export.py).
        #[arg(long)]
        weights: PathBuf,
        /// Tick cap for each deterministic seat/faction/seed case.
        #[arg(long, default_value_t = 4_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// Also write the JSON report here.
        #[arg(long)]
        out: Option<PathBuf>,
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

fn parse_level(level: &str) -> Result<oxide_sim::bot::Level> {
    Ok(match level {
        "easy" => oxide_sim::bot::Level::Easy,
        "medium" => oxide_sim::bot::Level::Medium,
        "hard" => oxide_sim::bot::Level::Hard,
        "expert" => oxide_sim::bot::Level::Expert,
        other => anyhow::bail!("unknown level '{other}'"),
    })
}

fn parse_named_style(style: &str) -> Result<oxide_sim::scenario::NamedStyle> {
    Ok(match style {
        "turtle" => oxide_sim::scenario::NamedStyle::Turtle,
        "balanced" => oxide_sim::scenario::NamedStyle::Balanced,
        "aggressive" => oxide_sim::scenario::NamedStyle::Aggressive,
        other => anyhow::bail!("unknown named style '{other}'"),
    })
}

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

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Run {
            scenario,
            ticks,
            bots,
            save_replay,
            map,
        } => {
            let scenario = runner::load_scenario(&scenario)?;
            let outcome = runner::run_scenario(&scenario, ticks, bots, save_replay.is_some())?;
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
        Cmd::Replay {
            path,
            ticks,
            expect_hash,
            allow_version_mismatch,
            allow_long,
        } => {
            let replay = GameReplay::load(&path)?;
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
            let replay =
                GameReplay::load(&path).with_context(|| format!("loading {}", path.display()))?;
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
            let replay =
                GameReplay::load(&path).with_context(|| format!("loading {}", path.display()))?;
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
            out,
        } => {
            let scenario = runner::load_scenario(&scenario)?;
            let outcome = runner::run_scenario(&scenario, ticks, bots, false)?;
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
        Cmd::BalanceProbe {
            dir,
            level,
            skill,
            aggression,
            style,
            variant,
            blunder,
            cadence,
            seeds,
            ticks,
            weights,
            out,
        } => {
            let level = parse_level(&level)?;
            oxide_driver::balance::balance_probe(
                &dir,
                level,
                &oxide_driver::balance::ProbeDials {
                    skill,
                    aggression,
                    style: style.as_deref().map(parse_named_style).transpose()?,
                    variant,
                    blunder,
                    cadence,
                },
                seeds,
                ticks,
                weights.as_deref(),
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
                // under-measured the claim. Bot seats proper are inert
                // until the retrained actor ships, so the bench drives
                // the Overseer per bot seat by hand.
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
            let replay =
                GameReplay::load(&path).with_context(|| format!("loading {}", path.display()))?;
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
        Cmd::Gym => oxide_driver::gym::serve()?,
        Cmd::NeuralCup {
            weights,
            seeds,
            ticks,
            cadence,
            scenario,
            blunder,
            skill,
            aggression,
            factions,
        } => oxide_driver::gym::neural_cup(
            &weights,
            seeds,
            &scenario,
            oxide_driver::gym::NeuralCupProfile {
                cadence,
                max_ticks: ticks,
                blunder,
                skill,
                aggression,
                factions,
            },
        )?,
        Cmd::CloseoutProbe {
            weights,
            seeds,
            ticks,
            blunder,
            cadence,
        } => oxide_driver::closeout::closeout_probe(&weights, seeds, ticks, blunder, cadence)?,
        Cmd::ViabilityProbe {
            weights,
            seeds,
            ticks,
            scenario,
            quota,
            start_tick,
            action,
            out,
        } => oxide_driver::viability::viability_probe(
            &weights,
            seeds,
            ticks,
            &scenario,
            quota,
            start_tick,
            action.as_deref(),
            out.as_deref(),
        )?,
        Cmd::RepairProbe {
            weights,
            ticks,
            out,
        } => oxide_driver::repair_probe::repair_probe(&weights, ticks, out.as_deref())?,
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
    fn balance_probe_accepts_an_exact_named_style_variant() {
        let cli = Cli::try_parse_from([
            "oxide-driver",
            "balance-probe",
            "--style",
            "turtle",
            "--variant",
            "1",
        ])
        .unwrap();
        let Cmd::BalanceProbe { style, variant, .. } = cli.cmd else {
            panic!("balance-probe parsed as another command")
        };
        assert_eq!(style.as_deref(), Some("turtle"));
        assert_eq!(variant, Some(1));
    }

    #[test]
    fn balance_probe_named_style_refuses_raw_profile_controls() {
        for raw in ["--skill", "--aggression", "--blunder"] {
            let error = Cli::try_parse_from([
                "oxide-driver",
                "balance-probe",
                "--style",
                "balanced",
                "--variant",
                "1",
                raw,
                "550",
            ])
            .err()
            .expect("named and raw profile selectors conflict");
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
    }

    #[test]
    fn balance_probe_variant_requires_a_named_style() {
        let error = Cli::try_parse_from(["oxide-driver", "balance-probe", "--variant", "1"])
            .err()
            .expect("variant without style is rejected");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
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
