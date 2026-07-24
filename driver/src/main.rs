//! The `oxide-driver` CLI. Run with `--help` for the full tree; AGENTS.md
//! has the guided tour.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use oxide_driver::client::Client;
use oxide_driver::runner::{self, GameReplay};
use oxide_driver::{render, smoke};
use oxide_protocol::{Key, MouseButton, RawEvent, Reply, Request, StateFilter, hash_hex};
use oxide_sim::{BuildingId, Command, PlayerId, Target, UnitId, UnitKind};
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
        /// Seeds per map.
        #[arg(long, default_value_t = 3)]
        seeds: u64,
        /// Tick cap per match.
        #[arg(long, default_value_t = 20_000)]
        ticks: u64,
        /// Candidate weights JSON (defaults to the embedded artifact).
        #[arg(long)]
        weights: Option<String>,
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
        /// Seeds to run (deterministic each).
        #[arg(long, default_value_t = 5)]
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

#[derive(Subcommand)]
enum LiveCmd {
    /// Tick, pause state, scenario, versions.
    Status,
    /// Structured sim snapshot.
    State {
        /// Include the ASCII map.
        #[arg(long)]
        map: bool,
    },
    /// Camera pose and visible world rect.
    Camera,
    /// Shell mode and active menu state.
    Ui,
    /// Canonical state fingerprint.
    Hash,
    /// Fast-forward N ticks (works while paused — that's the point).
    Advance {
        /// Tick count.
        ticks: u64,
    },
    /// Stop the wall clock (rendering continues).
    Pause,
    /// Restart the wall clock.
    Resume,
    /// Wall-clock speed multiplier.
    Speed {
        /// e.g. 4.0 for fast-forward, 0.25 for slow motion.
        multiplier: f64,
    },
    /// Send a raw sim command as JSON (see oxide-sim's Command).
    Send {
        /// Acting player index.
        player: u8,
        /// Command JSON, e.g. '{"type":"stop","units":[3]}'.
        json: String,
    },
    /// Attack-move units to a tile (engage everything on the way).
    AttackMove {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Goal as "x,y".
        #[arg(long)]
        to: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Resume a session from a replay file (fast-forwards, keeps recording).
    LoadReplay {
        /// Replay JSON path.
        path: String,
    },
    /// Move units to a tile.
    Move {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Goal as "x,y".
        #[arg(long)]
        to: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Walk units on a looping circuit, engaging everything met.
    Patrol {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Waypoint as "x,y"; repeat for each stop on the circuit.
        #[arg(long = "via")]
        via: Vec<String>,
    },
    /// Attack an enemy unit.
    AttackUnit {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Victim unit id.
        #[arg(long)]
        target: u32,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Attack an enemy building.
    AttackBuilding {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Victim building id.
        #[arg(long)]
        target: u32,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Put harvesters on a scrap node.
    Harvest {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// Node tile as "x,y".
        #[arg(long)]
        node: String,
        /// Append behind current orders instead of replacing them.
        #[arg(long)]
        queue: bool,
    },
    /// Queue a unit at a Foundry.
    Train {
        /// Acting player index.
        player: u8,
        /// Producing building id.
        #[arg(long)]
        building: u32,
        /// What to train.
        #[arg(long, value_enum)]
        kind: UnitKindArg,
    },
    /// Start a construction site with a harvester.
    Build {
        /// Acting player index.
        player: u8,
        /// Candidate builder unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// What to construct.
        #[arg(long, value_enum)]
        kind: BuildingKindArg,
        /// Anchor tile as "x,y" (top-left of the footprint).
        #[arg(long)]
        at: String,
    },
    /// Send harvesters to weld a damaged own built building.
    Repair {
        /// Acting player index.
        player: u8,
        /// Welder unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
        /// The building to weld.
        #[arg(long)]
        building: u32,
    },
    /// Scrap an own unfinished site for a partial refund.
    Cancel {
        /// Acting player index.
        player: u8,
        /// The site's building id.
        #[arg(long)]
        building: u32,
    },
    /// Set (or clear) a building's rally point.
    Rally {
        /// Acting player index.
        player: u8,
        /// The building id.
        #[arg(long)]
        building: u32,
        /// Rally tile as "x,y" (omit with --clear).
        #[arg(long, conflicts_with = "clear")]
        tile: Option<String>,
        /// Clear the rally instead.
        #[arg(long)]
        clear: bool,
    },
    /// Halt units.
    Stop {
        /// Acting player index.
        player: u8,
        /// Unit ids, comma-separated.
        #[arg(long, value_delimiter = ',')]
        units: Vec<u32>,
    },
    /// Inject a scroll-wheel event into the input funnel.
    InjectWheel {
        /// Notches; positive zooms in.
        delta: f32,
    },
    /// Inject a key press (and release).
    InjectKey {
        /// A mapped key: arrows, h/s/a/p/r/b/n/x, enter, escape, space,
        /// f1, shift, ctrl, or 1-9.
        key: String,
    },
    /// Inject a key press WITHOUT the release — held-key states (panning,
    /// modifiers) stay held until inject-key-up.
    InjectKeyDown {
        /// A mapped key, as inject-key accepts.
        key: String,
    },
    /// Inject a key release without a press.
    InjectKeyUp {
        /// A mapped key, as inject-key accepts.
        key: String,
    },
    /// Inject a chord: every key pressed in order, then released in
    /// reverse — `ctrl+1` assigns a control group exactly like a hand.
    InjectChord {
        /// Keys joined with '+', e.g. "ctrl+1" or "shift+f1".
        keys: String,
    },
    /// Inject a cursor move.
    InjectMouseMove {
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Inject a mouse-button press without releasing it.
    InjectMouseDown {
        /// "left", "right", or "middle".
        button: String,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Inject a mouse-button release without pressing it.
    InjectMouseUp {
        /// "left", "right", or "middle".
        button: String,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Inject a full click (down + up) at a window position.
    InjectClick {
        /// "left", "right", or "middle".
        button: String,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Drag between two window positions over several rendered frames.
    InjectDrag {
        /// Start as "x,y" window coordinates.
        #[arg(long)]
        from: String,
        /// End as "x,y" window coordinates.
        #[arg(long)]
        to: String,
        /// Mouse-move events between press and release (1-120).
        #[arg(long, default_value_t = 6)]
        steps: u32,
        /// "left", "right", or "middle".
        #[arg(long, default_value = "left")]
        button: String,
    },
    /// Capture the current frame to a PNG.
    Screenshot {
        /// Output path (shell-relative unless absolute).
        #[arg(short, long)]
        out: Option<String>,
    },
    /// Capture a frame sequence with sim ticks between frames, plus a
    /// downscaled contact sheet for reading motion at a glance.
    CaptureSequence {
        /// Frames to capture (2-64).
        #[arg(long, default_value_t = 8)]
        frames: u32,
        /// Sim ticks advanced between frames.
        #[arg(long, default_value_t = 5)]
        ticks_between: u64,
        /// Output directory for frame-NNN.png and sheet.png.
        #[arg(short, long)]
        out: std::path::PathBuf,
    },
    /// Toggle the debug overlay.
    Overlay,
    /// Swap in another scenario file.
    Load {
        /// Scenario JSON path.
        path: String,
    },
    /// Save the session replay.
    SaveReplay {
        /// Output path.
        path: String,
    },
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
        Cmd::BalanceProbe {
            dir,
            level,
            seeds,
            ticks,
            weights,
            out,
        } => {
            let level = match level.as_str() {
                "easy" => oxide_sim::bot::Level::Easy,
                "medium" => oxide_sim::bot::Level::Medium,
                "hard" => oxide_sim::bot::Level::Hard,
                "expert" => oxide_sim::bot::Level::Expert,
                other => anyhow::bail!("unknown level '{other}'"),
            };
            oxide_driver::balance::balance_probe(
                &dir,
                level,
                seeds,
                ticks,
                weights.as_deref(),
                out.as_deref(),
            )?;
        }
        Cmd::Bench { units, ticks } => {
            let scenario = oxide_kit::bench::mass_battle(units, 9);
            let mut state = scenario.build()?;
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
        Cmd::Matchup { a, b, seeds } => {
            let army_a = oxide_kit::matchup::parse_army(&a)?;
            let army_b = oxide_kit::matchup::parse_army(&b)?;
            println!(
                "A = {a} ({} scrap)   B = {b} ({} scrap)",
                oxide_kit::matchup::army_cost(&army_a),
                oxide_kit::matchup::army_cost(&army_b),
            );
            let (mut a_total, mut b_total) = (0u64, 0u64);
            for seed in 0..seeds {
                let out = oxide_kit::matchup::duel(&army_a, &army_b, 42 + seed, 8_000)?;
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

fn parse_tile(s: &str) -> Result<chassis::grid::TilePos> {
    let (x, y) = s
        .split_once(',')
        .with_context(|| format!("expected \"x,y\", got {s:?}"))?;
    Ok(chassis::grid::TilePos::new(
        x.trim().parse()?,
        y.trim().parse()?,
    ))
}

fn parse_point(s: &str) -> Result<(f32, f32)> {
    let (x, y) = s
        .split_once(',')
        .with_context(|| format!("expected \"x,y\", got {s:?}"))?;
    let point = (x.trim().parse::<f32>()?, y.trim().parse::<f32>()?);
    if point.0.is_finite() && point.1.is_finite() {
        Ok(point)
    } else {
        bail!("point coordinates must be finite")
    }
}

fn parse_mouse_button(s: &str) -> Result<MouseButton> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        other => bail!("unknown button {other:?}"),
    })
}

/// Clap-native unit kinds — typos die in argument parsing with the full
/// list of choices, before anything touches the socket.
#[derive(Clone, Copy, clap::ValueEnum)]
enum UnitKindArg {
    Harvester,
    Sentinel,
    Scuttler,
    Lancer,
    Bombard,
    Flakhound,
    Stinger,
    Buzzard,
    Darter,
    Talon,
    Wisp,
}

impl From<UnitKindArg> for UnitKind {
    fn from(k: UnitKindArg) -> Self {
        match k {
            UnitKindArg::Harvester => UnitKind::Harvester,
            UnitKindArg::Sentinel => UnitKind::Sentinel,
            UnitKindArg::Scuttler => UnitKind::Scuttler,
            UnitKindArg::Lancer => UnitKind::Lancer,
            UnitKindArg::Bombard => UnitKind::Bombard,
            UnitKindArg::Flakhound => UnitKind::Flakhound,
            UnitKindArg::Stinger => UnitKind::Stinger,
            UnitKindArg::Buzzard => UnitKind::Buzzard,
            UnitKindArg::Darter => UnitKind::Darter,
            UnitKindArg::Talon => UnitKind::Talon,
            UnitKindArg::Wisp => UnitKind::Wisp,
        }
    }
}

/// Buildable kinds only — the Foundry is scenario-authored and rejecting
/// it at the parser teaches that faster than a sim rejection would.
#[derive(Clone, Copy, clap::ValueEnum)]
enum BuildingKindArg {
    Turret,
    Fabricator,
    FlakTurret,
    Bastion,
    Array,
    Reclaimer,
}

impl From<BuildingKindArg> for oxide_sim::BuildingKind {
    fn from(k: BuildingKindArg) -> Self {
        match k {
            BuildingKindArg::Turret => oxide_sim::BuildingKind::Turret,
            BuildingKindArg::Fabricator => oxide_sim::BuildingKind::Fabricator,
            BuildingKindArg::FlakTurret => oxide_sim::BuildingKind::FlakTurret,
            BuildingKindArg::Bastion => oxide_sim::BuildingKind::Bastion,
            BuildingKindArg::Array => oxide_sim::BuildingKind::Array,
            BuildingKindArg::Reclaimer => oxide_sim::BuildingKind::Reclaimer,
        }
    }
}

fn parse_key(s: &str) -> Result<Key> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "h" => Key::H,
        "s" => Key::S,
        "a" => Key::A,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "o" => Key::O,
        "q" => Key::Q,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "y" => Key::Y,
        "z" => Key::Z,
        "p" => Key::P,
        "r" => Key::R,
        "b" => Key::B,
        "n" => Key::N,
        "x" => Key::X,
        "enter" | "return" => Key::Enter,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "escape" | "esc" => Key::Escape,
        "space" => Key::Space,
        "f1" => Key::F1,
        "shift" => Key::Shift,
        "ctrl" => Key::Ctrl,
        "1" => Key::Num1,
        "2" => Key::Num2,
        "3" => Key::Num3,
        "4" => Key::Num4,
        "5" => Key::Num5,
        "6" => Key::Num6,
        "7" => Key::Num7,
        "8" => Key::Num8,
        "9" => Key::Num9,
        other => bail!("unknown key {other:?}"),
    })
}

fn units(ids: Vec<u32>) -> Vec<UnitId> {
    ids.into_iter().map(UnitId).collect()
}

fn live_requests(cmd: LiveCmd) -> Result<Vec<Request>> {
    Ok(vec![match cmd {
        LiveCmd::Status => Request::Status,
        LiveCmd::State { map } => Request::QueryState {
            filter: StateFilter {
                map,
                ..StateFilter::default()
            },
        },
        LiveCmd::Camera => Request::QueryCamera,
        LiveCmd::Ui => Request::QueryUi,
        LiveCmd::Hash => Request::StateHash,
        LiveCmd::Advance { ticks } => Request::AdvanceTicks { ticks },
        LiveCmd::Pause => Request::Pause,
        LiveCmd::Resume => Request::Resume,
        LiveCmd::Speed { multiplier } => Request::SetSpeed { multiplier },
        LiveCmd::Send { player, json } => Request::SendCommand {
            player: PlayerId(player),
            command: serde_json::from_str(&json).context("parsing command JSON")?,
        },
        LiveCmd::Move {
            player,
            units: ids,
            to,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Move {
                units: units(ids),
                goal: parse_tile(&to)?,
                queue,
            },
        },
        LiveCmd::Patrol {
            player,
            units: ids,
            via,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Patrol {
                units: units(ids),
                waypoints: via
                    .iter()
                    .map(|w| parse_tile(w))
                    .collect::<Result<Vec<_>>>()?,
            },
        },
        LiveCmd::AttackMove {
            player,
            units: ids,
            to,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::AttackMove {
                units: units(ids),
                goal: parse_tile(&to)?,
                queue,
            },
        },
        LiveCmd::LoadReplay { path } => Request::LoadReplay { path },
        LiveCmd::AttackUnit {
            player,
            units: ids,
            target,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Attack {
                units: units(ids),
                target: Target::Unit(UnitId(target)),
                queue,
            },
        },
        LiveCmd::AttackBuilding {
            player,
            units: ids,
            target,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Attack {
                units: units(ids),
                target: Target::Building(BuildingId(target)),
                queue,
            },
        },
        LiveCmd::Harvest {
            player,
            units: ids,
            node,
            queue,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Harvest {
                units: units(ids),
                node: parse_tile(&node)?,
                queue,
            },
        },
        LiveCmd::Train {
            player,
            building,
            kind,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Train {
                building: BuildingId(building),
                kind: kind.into(),
            },
        },
        LiveCmd::Stop { player, units: ids } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Stop { units: units(ids) },
        },
        LiveCmd::Build {
            player,
            units: ids,
            kind,
            at,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Build {
                units: units(ids),
                kind: kind.into(),
                anchor: parse_tile(&at)?,
            },
        },
        LiveCmd::Repair {
            player,
            units: ids,
            building,
        } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Repair {
                units: units(ids),
                building: BuildingId(building),
            },
        },
        LiveCmd::Cancel { player, building } => Request::SendCommand {
            player: PlayerId(player),
            command: Command::Cancel {
                building: BuildingId(building),
            },
        },
        LiveCmd::Rally {
            player,
            building,
            tile,
            clear,
        } => {
            let rally = match (tile, clear) {
                (Some(t), _) => Some(parse_tile(&t)?),
                (None, true) => None,
                (None, false) => bail!("pass --tile x,y or --clear"),
            };
            Request::SendCommand {
                player: PlayerId(player),
                command: Command::SetRally {
                    building: BuildingId(building),
                    rally,
                },
            }
        }
        LiveCmd::InjectWheel { delta } => Request::InjectEvent {
            event: RawEvent::Wheel { delta },
        },
        LiveCmd::InjectKey { key } => {
            // A tap: down then up, so held-key panning can't get stuck on.
            let key = parse_key(&key)?;
            return Ok(vec![
                Request::InjectEvent {
                    event: RawEvent::KeyDown { key },
                },
                Request::InjectEvent {
                    event: RawEvent::KeyUp { key },
                },
            ]);
        }
        LiveCmd::InjectKeyDown { key } => Request::InjectEvent {
            event: RawEvent::KeyDown {
                key: parse_key(&key)?,
            },
        },
        LiveCmd::InjectKeyUp { key } => Request::InjectEvent {
            event: RawEvent::KeyUp {
                key: parse_key(&key)?,
            },
        },
        LiveCmd::InjectChord { keys } => {
            let keys: Vec<Key> = keys
                .split('+')
                .map(|part| parse_key(part.trim()))
                .collect::<Result<_>>()?;
            if keys.is_empty() {
                bail!("a chord needs at least one key");
            }
            // Down in written order, up in reverse — modifiers wrap the
            // core key the way a hand holds them.
            let mut requests: Vec<Request> = keys
                .iter()
                .map(|&key| Request::InjectEvent {
                    event: RawEvent::KeyDown { key },
                })
                .collect();
            requests.extend(keys.iter().rev().map(|&key| Request::InjectEvent {
                event: RawEvent::KeyUp { key },
            }));
            return Ok(requests);
        }
        LiveCmd::InjectMouseMove { x, y } => Request::InjectEvent {
            event: RawEvent::MouseMove { x, y },
        },
        LiveCmd::InjectMouseDown { button, x, y } => Request::InjectEvent {
            event: RawEvent::MouseDown {
                button: parse_mouse_button(&button)?,
                x,
                y,
            },
        },
        LiveCmd::InjectMouseUp { button, x, y } => Request::InjectEvent {
            event: RawEvent::MouseUp {
                button: parse_mouse_button(&button)?,
                x,
                y,
            },
        },
        LiveCmd::InjectClick { button, x, y } => {
            let button = parse_mouse_button(&button)?;
            // A click is a pair; the shell treats a lone down as a drag start.
            return Ok(vec![
                Request::InjectEvent {
                    event: RawEvent::MouseDown { button, x, y },
                },
                Request::InjectEvent {
                    event: RawEvent::MouseUp { button, x, y },
                },
            ]);
        }
        LiveCmd::InjectDrag {
            from,
            to,
            steps,
            button,
        } => {
            if !(1..=120).contains(&steps) {
                bail!("drag steps must be within 1..=120");
            }
            let (from_x, from_y) = parse_point(&from)?;
            let (to_x, to_y) = parse_point(&to)?;
            let button = parse_mouse_button(&button)?;
            let mut requests = Vec::with_capacity(steps as usize + 2);
            requests.push(Request::InjectEvent {
                event: RawEvent::MouseDown {
                    button,
                    x: from_x,
                    y: from_y,
                },
            });
            for step in 1..=steps {
                let t = step as f32 / steps as f32;
                requests.push(Request::InjectEvent {
                    event: RawEvent::MouseMove {
                        x: from_x + (to_x - from_x) * t,
                        y: from_y + (to_y - from_y) * t,
                    },
                });
            }
            requests.push(Request::InjectEvent {
                event: RawEvent::MouseUp {
                    button,
                    x: to_x,
                    y: to_y,
                },
            });
            return Ok(requests);
        }
        LiveCmd::Screenshot { out } => Request::Screenshot { path: out },
        LiveCmd::CaptureSequence { .. } => {
            bail!("capture-sequence is executed directly, not mapped to requests")
        }
        LiveCmd::Overlay => Request::ToggleOverlay,
        LiveCmd::Load { path } => Request::LoadScenario { path },
        LiveCmd::SaveReplay { path } => Request::SaveReplay { path },
    }])
}

/// Drives a capture run: advance, screenshot, repeat, then tile every
/// frame (quarter scale) into one contact sheet for reading motion at a
/// glance. Frames land as `frame-NNN.png` beside `sheet.png`.
fn capture_sequence(
    addr: &str,
    frames: u32,
    ticks_between: u64,
    out: &std::path::Path,
) -> Result<()> {
    if !(2..=64).contains(&frames) {
        bail!("frames must be within 2..=64");
    }
    std::fs::create_dir_all(out)?;
    let out = out.canonicalize()?;
    let mut client = Client::connect(addr)?;
    let mut paths = Vec::new();
    for i in 0..frames {
        if i > 0 {
            client.call(Request::AdvanceTicks {
                ticks: ticks_between,
            })?;
        }
        let path = out.join(format!("frame-{i:03}.png"));
        client.call(Request::Screenshot {
            path: Some(path.to_string_lossy().into_owned()),
        })?;
        paths.push(path);
    }

    let first = tiny_skia::Pixmap::decode_png(&std::fs::read(&paths[0])?)
        .context("decoding first frame")?;
    const SHEET_SCALE: f32 = 0.25;
    let tile_w = (first.width() as f32 * SHEET_SCALE).ceil() as u32;
    let tile_h = (first.height() as f32 * SHEET_SCALE).ceil() as u32;
    let columns = (frames as f32).sqrt().ceil() as u32;
    let rows = frames.div_ceil(columns);
    let mut sheet = tiny_skia::Pixmap::new(columns * tile_w, rows * tile_h)
        .context("allocating contact sheet")?;
    for (i, path) in paths.iter().enumerate() {
        let frame = tiny_skia::Pixmap::decode_png(&std::fs::read(path)?)
            .with_context(|| format!("decoding {}", path.display()))?;
        let (col, row) = (i as u32 % columns, i as u32 / columns);
        sheet.draw_pixmap(
            0,
            0,
            frame.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::from_scale(SHEET_SCALE, SHEET_SCALE)
                .post_translate((col * tile_w) as f32, (row * tile_h) as f32),
            None,
        );
    }
    let sheet_path = out.join("sheet.png");
    sheet.save_png(&sheet_path)?;
    eprintln!("wrote {} frames and {}", frames, sheet_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_presses_in_order_and_releases_in_reverse() {
        let requests = live_requests(LiveCmd::InjectChord {
            keys: "ctrl+1".to_string(),
        })
        .unwrap();
        let events: Vec<&RawEvent> = requests
            .iter()
            .map(|r| match r {
                Request::InjectEvent { event } => event,
                other => panic!("chords are pure injections, got {other:?}"),
            })
            .collect();
        assert!(
            matches!(
                events[..],
                [
                    RawEvent::KeyDown { key: Key::Ctrl },
                    RawEvent::KeyDown { key: Key::Num1 },
                    RawEvent::KeyUp { key: Key::Num1 },
                    RawEvent::KeyUp { key: Key::Ctrl },
                ]
            ),
            "modifiers must wrap the core key: {events:?}"
        );
    }

    #[test]
    fn a_chord_of_nonsense_fails_before_touching_the_socket() {
        assert!(
            live_requests(LiveCmd::InjectChord {
                keys: "ctrl+florb".to_string(),
            })
            .is_err()
        );
    }

    #[test]
    fn drag_expands_to_press_moves_and_release() {
        let requests = live_requests(LiveCmd::InjectDrag {
            from: "10,20".to_string(),
            to: "40,50".to_string(),
            steps: 3,
            button: "left".to_string(),
        })
        .unwrap();
        assert_eq!(requests.len(), 5);
        assert_eq!(
            requests[0],
            Request::InjectEvent {
                event: RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: 10.0,
                    y: 20.0,
                }
            }
        );
        assert_eq!(
            requests[2],
            Request::InjectEvent {
                event: RawEvent::MouseMove { x: 30.0, y: 40.0 }
            }
        );
        assert_eq!(
            requests[4],
            Request::InjectEvent {
                event: RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: 40.0,
                    y: 50.0,
                }
            }
        );
    }

    #[test]
    fn drag_rejects_unbounded_event_counts() {
        let err = live_requests(LiveCmd::InjectDrag {
            from: "0,0".to_string(),
            to: "1,1".to_string(),
            steps: 121,
            button: "left".to_string(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("1..=120"));
    }

    #[test]
    fn every_protocol_key_is_cli_addressable() {
        for key in [
            "up", "down", "left", "right", "h", "s", "a", "p", "r", "b", "n", "x", "enter",
            "escape", "space", "f1", "shift", "ctrl", "1", "2", "3", "4", "5", "6", "7", "8", "9",
        ] {
            assert!(parse_key(key).is_ok(), "missing CLI spelling for {key}");
        }
    }
}
