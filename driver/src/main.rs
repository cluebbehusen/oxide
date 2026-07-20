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
        #[arg(long, default_value_t = 30)]
        seeds: u64,
        /// Decision cadence the network trained at.
        #[arg(long, default_value_t = 16)]
        cadence: u64,
        /// Scenario path, or "skirmish".
        #[arg(long, default_value = "skirmish")]
        scenario: String,
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
        /// up/down/left/right/h/s/p/escape/space/f1.
        key: String,
    },
    /// Inject a cursor move.
    InjectMouseMove {
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Inject a full click (down + up) at a window position.
    InjectClick {
        /// "left" or "right".
        button: String,
        /// Window x.
        x: f32,
        /// Window y.
        y: f32,
    },
    /// Capture the current frame to a PNG.
    Screenshot {
        /// Output path (shell-relative unless absolute).
        #[arg(short, long)]
        out: Option<String>,
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
        Cmd::Live { addr, cmd } => {
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
        } => oxide_driver::gym::neural_cup(&weights, seeds, cadence, &scenario)?,
        Cmd::Smoke { addr, spawn } => smoke::run(&addr, spawn)?,
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

/// Clap-native unit kinds — typos die in argument parsing with the full
/// list of choices, before anything touches the socket.
#[derive(Clone, Copy, clap::ValueEnum)]
enum UnitKindArg {
    Harvester,
    Sentinel,
    Scuttler,
    Lancer,
}

impl From<UnitKindArg> for UnitKind {
    fn from(k: UnitKindArg) -> Self {
        match k {
            UnitKindArg::Harvester => UnitKind::Harvester,
            UnitKindArg::Sentinel => UnitKind::Sentinel,
            UnitKindArg::Scuttler => UnitKind::Scuttler,
            UnitKindArg::Lancer => UnitKind::Lancer,
        }
    }
}

/// Buildable kinds only — the Foundry is scenario-authored and rejecting
/// it at the parser teaches that faster than a sim rejection would.
#[derive(Clone, Copy, clap::ValueEnum)]
enum BuildingKindArg {
    Turret,
    Fabricator,
}

impl From<BuildingKindArg> for oxide_sim::BuildingKind {
    fn from(k: BuildingKindArg) -> Self {
        match k {
            BuildingKindArg::Turret => oxide_sim::BuildingKind::Turret,
            BuildingKindArg::Fabricator => oxide_sim::BuildingKind::Fabricator,
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
        "p" => Key::P,
        "r" => Key::R,
        "b" => Key::B,
        "n" => Key::N,
        "x" => Key::X,
        "enter" | "return" => Key::Enter,
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
        LiveCmd::InjectMouseMove { x, y } => Request::InjectEvent {
            event: RawEvent::MouseMove { x, y },
        },
        LiveCmd::InjectClick { button, x, y } => {
            let button = match button.as_str() {
                "left" => MouseButton::Left,
                "right" => MouseButton::Right,
                other => bail!("unknown button {other:?}"),
            };
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
        LiveCmd::Screenshot { out } => Request::Screenshot { path: out },
        LiveCmd::Overlay => Request::ToggleOverlay,
        LiveCmd::Load { path } => Request::LoadScenario { path },
        LiveCmd::SaveReplay { path } => Request::SaveReplay { path },
    }])
}
