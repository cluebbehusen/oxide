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
        /// Let scenario-flagged bots play.
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
    /// Render a scenario state to a PNG (software rasterizer, no window).
    Render {
        /// Scenario path, or "skirmish".
        scenario: String,
        /// Ticks to simulate first.
        #[arg(long, default_value_t = 0)]
        ticks: u64,
        /// Let scenario-flagged bots play during those ticks.
        #[arg(long)]
        bots: bool,
        /// Output PNG path.
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Measure a map: room per seat, route lengths by domain, resources,
    /// artillery pressure, spawn spacing.
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
        /// Raw skill-knob override 0-1000 (candidate --weights probes
        /// only; locates points between the shipped levels).
        #[arg(long)]
        skill: Option<u32>,
        /// Explicit blunder rate per mille (candidate probes only;
        /// 0 derives from skill — separates the two dials the skill
        /// knob normally moves together).
        #[arg(long, default_value_t = 0)]
        blunder: u32,
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
        /// Candidate weights JSON (defaults to the embedded artifact).
        #[arg(long)]
        weights: Option<String>,
        /// Raw JSON output path.
        #[arg(long)]
        out: Option<String>,
    },
    /// Decisiveness seed sweep: N seeds of bot-vs-bot on one 1v1 map,
    /// each seed played in both personality orientations — do games
    /// END, and does a seat lean survive the exchange? The 0.12 bot
    /// phases gate on this.
    Sweep {
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Ladder level to sweep ("easy".."expert").
        #[arg(long, default_value = "medium")]
        level: String,
        /// Seeds (each played twice: dealt and personality-swapped).
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
    /// Head-to-head duel between two ladder profiles (optionally with
    /// candidate skill/cadence dial overrides), each seed fought from
    /// both seats — the re-metering experiments' measuring stick.
    Duel {
        /// Side A level ("easy".."expert").
        #[arg(long)]
        a: String,
        /// Side A skill-knob override (candidate dials).
        #[arg(long)]
        a_skill: Option<u32>,
        /// Side A cadence override.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        a_cadence: Option<u64>,
        /// Side B level ("easy".."expert").
        #[arg(long)]
        b: String,
        /// Side B skill-knob override.
        #[arg(long)]
        b_skill: Option<u32>,
        /// Side B cadence override.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        b_cadence: Option<u64>,
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Seeds (each fought from both seats).
        #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u64).range(1..))]
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
    /// Widened scripted-yardstick measurement for one ladder profile
    /// (optionally with candidate dials): the profile vs all four
    /// scripted tiers over N seeds per tier, both seats — the
    /// doctrinal strength instrument for re-metering.
    Yardstick {
        /// Profile level ("easy".."expert").
        #[arg(long, default_value = "medium")]
        level: String,
        /// Skill-knob override (candidate dials).
        #[arg(long)]
        skill: Option<u32>,
        /// Cadence override.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        cadence: Option<u64>,
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Seeds per tier (each fought from both seats).
        #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Tick cap per match.
        #[arg(long, default_value_t = 40_000, value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// First scenario seed (the gate's slate starts at 3000).
        #[arg(long, default_value_t = 3_000)]
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
        /// Bench a shipped scenario with its bots thinking instead of
        /// the synthetic mass battle (e.g. "scenarios/compass-grand.json"
        /// — eight neural minds, the heaviest honest shape).
        #[arg(long)]
        scenario: Option<String>,
    },
    /// Par-cost arena duel between two hand-picked armies (no
    /// economy): the balance review's controlled experiment.
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
        /// Seeds to run (deterministic each).
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
    },
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
    /// Serve training episodes over stdio (newline-delimited JSON).
    Gym,
    /// Tournament a quantized policy artifact against the scripted
    /// tiers (the promotion gate measures the shipped integer bot).
    NeuralCup {
        /// Exported weights JSON (tools/train/export.py).
        #[arg(long)]
        weights: PathBuf,
        /// Seeds per matchup (each played from both seats).
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
        seeds: u64,
        /// Decision cadence the network trained at.
        #[arg(long, default_value_t = 16)]
        cadence: u64,
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
        /// Blunder rate per mille (0 = derive from skill).
        #[arg(long, default_value_t = 0)]
        blunder: u32,
        /// Skill knob 0-1000 (conditioning input + derived blunders).
        #[arg(long, default_value_t = 1000)]
        skill: u32,
        /// Aggression knob 0-1000 (personality conditioning input).
        #[arg(long, default_value_t = 500)]
        aggression: u32,
    },
    /// Automated end-to-end check against a live shell.
    Smoke {
        /// Shell debug-server address.
        #[arg(long, default_value = "127.0.0.1:4123")]
        addr: String,
        /// Spawn `cargo run -p oxide-shell` first and kill it after.
        #[arg(long)]
        spawn: bool,
    },
    /// Perceptual-diff screenshot suite: ten canonical screens from a
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
            level,
            seeds,
            ticks,
            seed_base,
            out,
        } => {
            let level = parse_level(&level)?;
            oxide_driver::sweep::sweep_report(
                &scenario,
                level,
                seeds,
                ticks,
                seed_base,
                out.as_deref(),
            )?;
        }
        Cmd::Duel {
            a,
            a_skill,
            a_cadence,
            b,
            b_skill,
            b_cadence,
            scenario,
            seeds,
            ticks,
            seed_base,
            out,
        } => {
            let a = oxide_driver::sweep::DuelSide {
                level: parse_level(&a)?,
                skill: a_skill,
                cadence: a_cadence,
            };
            let b = oxide_driver::sweep::DuelSide {
                level: parse_level(&b)?,
                skill: b_skill,
                cadence: b_cadence,
            };
            oxide_driver::sweep::duel_report(
                &scenario,
                &a,
                &b,
                seeds,
                ticks,
                seed_base,
                out.as_deref(),
            )?;
        }
        Cmd::Yardstick {
            level,
            skill,
            cadence,
            scenario,
            seeds,
            ticks,
            seed_base,
            out,
        } => {
            let side = oxide_driver::sweep::DuelSide {
                level: parse_level(&level)?,
                skill,
                cadence,
            };
            oxide_driver::sweep::yardstick_report(
                &scenario,
                &side,
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
                // honest shape (eight neural minds on the 4v4 map),
                // deciding whether a perf window is needed. Shipped
                // playable maps author a human seat, so every chair is
                // converted first; benching around an idle seat 0
                // under-measured the claim.
                let mut sc = runner::load_scenario(&path)?;
                oxide_kit::bench::all_bots(&mut sc);
                let mut state = sc.build()?;
                let mut bots = oxide_sim::bot::seat_bots(&sc);
                let start = std::time::Instant::now();
                for _ in 0..ticks {
                    let mut commands = Vec::new();
                    for bot in &mut bots {
                        commands.extend(bot.act(&state));
                    }
                    state.tick(&commands);
                }
                let secs = start.elapsed().as_secs_f64();
                println!(
                    "bench: {} ({} bot seats) x {ticks} ticks in {secs:.2}s = {:.0} ticks/s (hash {:#x})",
                    sc.name,
                    bots.len(),
                    f64::from(ticks) / secs,
                    state.hash()
                );
                return Ok(());
            }
            let scenario = oxide_kit::bench::mass_battle(units, 9);
            let mut state = scenario.build()?;
            oxide_kit::bench::engage(&mut state);
            let start = std::time::Instant::now();
            for _ in 0..ticks {
                state.tick(&[]);
            }
            let secs = start.elapsed().as_secs_f64();
            println!(
                "bench: {} units x {ticks} ticks in {secs:.2}s = {:.0} ticks/s (hash {:#x})",
                units * 2,
                f64::from(ticks) / secs,
                state.hash(),
            );
        }
        Cmd::Matchup {
            a,
            b,
            b_structures,
            seeds,
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
                    "  + garrison {spec} ({} scrap)",
                    oxide_kit::matchup::garrison_cost(&garrison)
                );
            }
            println!();
            let (mut a_total, mut b_total) = (0u64, 0u64);
            for seed in 0..seeds {
                let out = oxide_kit::matchup::siege(&army_a, &army_b, &garrison, 42 + seed, 8_000)?;
                a_total += u64::from(out.a_value);
                b_total += u64::from(out.b_value);
                println!(
                    "  seed {seed}: A survives {:>4}  B survives {:>4}  ({} ticks)",
                    out.a_value, out.b_value, out.ticks
                );
            }
            println!(
                "mean surviving value  A {}  B {}",
                a_total / seeds,
                b_total / seeds
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
                out,
            } = cmd
            {
                return capture_sequence(&addr, frames, ticks_between, &out);
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
        Cmd::Gym => oxide_driver::gym::serve()?,
        Cmd::NeuralCup {
            weights,
            seeds,
            cadence,
            scenario,
            blunder,
            skill,
            aggression,
        } => oxide_driver::gym::neural_cup(
            &weights, seeds, cadence, &scenario, blunder, skill, aggression,
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
