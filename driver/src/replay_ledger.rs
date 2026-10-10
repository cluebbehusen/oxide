//! `replay-ledger`: the impact ledger over recorded games, for balancing and
//! review. Each replay or match recording is re-executed through
//! [`crate::ledger::ImpactLedger`]; the text report pools seats by
//! controller.

use crate::ledger::{ImpactLedger, LedgerPool, SeatLedger, WORTH_PERIOD};
use anyhow::{Context, Result, bail};
use oxide_kit::GameReplay;
use oxide_sim::SIM_VERSION;
use oxide_sim::scenario::BotConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Magic bytes that open a player save.
const SAVE_MAGIC: &[u8] = b"OXIDESAV";

/// Ticks per minute.
const MINUTE: u64 = oxide_sim::TICKS_PER_SECOND as u64 * 60;

/// One seat of a recorded game.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerSeat {
    /// Player seat.
    pub seat: u8,
    /// Seat name.
    pub name: String,
    /// Team.
    pub team: u8,
    /// Who played the seat.
    pub controller: SeatPlayer,
    /// What its units and buildings did.
    pub ledger: SeatLedger,
}

/// Who played a seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "player", rename_all = "snake_case")]
pub enum SeatPlayer {
    /// A person.
    Human,
    /// A bot, with its configuration.
    Bot {
        /// The configuration.
        config: BotConfig,
    },
    /// The recording names controllers it could not be read for.
    Unknown,
}

/// One recorded game.
#[derive(Debug, Clone, Serialize)]
pub struct GameLedger {
    /// Where it was read from.
    pub path: PathBuf,
    /// Tick the recording begins at: zero, or later for one that starts from
    /// a saved world.
    pub start: u64,
    /// Ticks re-executed.
    pub ticks: u64,
    /// Seats, by seat.
    pub seats: Vec<LedgerSeat>,
}

/// Re-executes `replay` through the ledger.
pub fn ledger(path: &Path, replay: &GameReplay) -> Result<GameLedger> {
    replay.validate(Some(SIM_VERSION))?;
    let total = oxide_kit::bounded_replay_duration(replay)?;
    let mut state = oxide_kit::recording::initial_state(replay)?;
    let start = state.current_tick();
    let mut ledger = ImpactLedger::new(&state);
    let mut playback = oxide_kit::ReplayPlayback::new(replay);
    while state.current_tick() < total {
        let report = playback.step(&mut state);
        ledger.observe(&state, &report);
    }
    let ledgers = ledger.finish(&state);
    let evaluated = evaluation_controllers(replay);
    let seats = replay
        .setup
        .players
        .iter()
        .zip(ledgers)
        .enumerate()
        .map(|(seat, (spec, ledger))| LedgerSeat {
            seat: u8::try_from(seat).expect("seat indices fit in u8"),
            name: spec.name.clone(),
            team: state.players()[seat].team,
            controller: if spec.bot {
                SeatPlayer::Bot {
                    config: spec.bot_config.unwrap_or_default(),
                }
            } else {
                match &evaluated {
                    None => SeatPlayer::Human,
                    Some(Ok(controllers)) => match controllers.get(seat) {
                        Some(Some(recorded)) => SeatPlayer::Bot {
                            config: recorded.config,
                        },
                        Some(None) => SeatPlayer::Human,
                        None => SeatPlayer::Unknown,
                    },
                    Some(Err(_)) => SeatPlayer::Unknown,
                }
            },
            ledger,
        })
        .collect();
    Ok(GameLedger {
        path: path.to_owned(),
        start,
        ticks: state.current_tick() - start,
        seats,
    })
}

/// A controller as an evaluation replay records it.
#[derive(Deserialize)]
struct RecordedController {
    config: BotConfig,
}

/// The controllers an evaluation replay drove its seats with: evaluation
/// clears the scenario's bot seats and records each seat's controller in
/// the replay description instead. `None` for other recordings.
fn evaluation_controllers(
    replay: &GameReplay,
) -> Option<serde_json::Result<Vec<Option<RecordedController>>>> {
    let description = replay.meta.description.as_deref()?;
    // The controllers come last, after fields such as the candidate that may
    // themselves contain the marker.
    let (_, controllers) = description.rsplit_once("; controllers=")?;
    Some(serde_json::from_str(controllers))
}

/// Loads `path` as a replay or match recording, refusing a player save.
pub fn load(path: &Path) -> Result<GameReplay> {
    let mut head = [0_u8; SAVE_MAGIC.len()];
    let save = std::fs::File::open(path)
        .and_then(|mut file| std::io::Read::read(&mut file, &mut head))
        .with_context(|| format!("reading {}", path.display()))?
        == head.len()
        && head == SAVE_MAGIC;
    if save {
        bail!(
            "{} is a player save, which keeps no command history to re-execute; \
             use the match recording (match-*.json) instead",
            path.display()
        );
    }
    oxide_kit::load_replay(path).with_context(|| format!("loading {}", path.display()))
}

/// The label a seat pools under: its controller and, for a bot, its
/// difficulty.
fn group(seat: &LedgerSeat) -> String {
    match seat.controller {
        SeatPlayer::Human => "human".to_owned(),
        SeatPlayer::Unknown => "unknown controller".to_owned(),
        SeatPlayer::Bot { config } => format!("bot {}", config.difficulty),
    }
}

/// The text report over `games`.
pub fn render(games: &[GameLedger]) -> String {
    let mut pools: BTreeMap<String, LedgerPool> = BTreeMap::new();
    for game in games {
        for seat in &game.seats {
            pools.entry(group(seat)).or_default().add(&seat.ledger);
        }
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} game{}; value dealt is scrap of damage done, credited to whoever hit",
        games.len(),
        if games.len() == 1 { "" } else { "s" }
    );
    for (label, pool) in &pools {
        let _ = writeln!(out, "\n== {label} ({} seat-games)", pool.seats);
        pool.render(&mut out, "  ", None);
    }
    if let [game] = games {
        let _ = writeln!(out, "\n== net worth by minute (army, buildings and bank)");
        for seat in &game.seats {
            let samples: Vec<String> = seat
                .ledger
                .worth
                .iter()
                .enumerate()
                .map(|(index, worth)| (game.start + (index as u64 + 1) * WORTH_PERIOD, worth))
                .filter(|(tick, _)| tick % (5 * MINUTE) < WORTH_PERIOD)
                .map(|(tick, worth)| format!("{}m {worth}", tick / MINUTE))
                .collect();
            let _ = writeln!(
                out,
                "  seat {} {}: {}",
                seat.seat,
                seat.name,
                samples.join(", ")
            );
        }
    }
    out
}

#[cfg(test)]
mod tests;
