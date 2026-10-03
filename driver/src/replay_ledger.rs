//! `replay-ledger`: the impact ledger over recorded games, for balancing and
//! review. Each replay or match recording is re-executed through
//! [`crate::ledger::ImpactLedger`]; the text report pools seats by faction
//! and controller.

use crate::ledger::{ImpactLedger, LedgerPool, SeatLedger, WORTH_PERIOD};
use anyhow::{Context, Result, bail};
use oxide_kit::GameReplay;
use oxide_sim::scenario::BotConfig;
use oxide_sim::{Faction, SIM_VERSION};
use serde::Serialize;
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
    /// Faction.
    pub faction: Faction,
    /// Team.
    pub team: u8,
    /// The seat's bot configuration; absent for a human seat.
    pub bot_config: Option<BotConfig>,
    /// What its units and buildings did.
    pub ledger: SeatLedger,
}

/// One recorded game.
#[derive(Debug, Clone, Serialize)]
pub struct GameLedger {
    /// Where it was read from.
    pub path: PathBuf,
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
    let mut ledger = ImpactLedger::new(&state);
    let mut playback = oxide_kit::ReplayPlayback::new(replay);
    while state.current_tick() < total {
        let report = playback.step(&mut state);
        ledger.observe(&state, &report);
    }
    let ledgers = ledger.finish(state.current_tick());
    let seats = replay
        .setup
        .players
        .iter()
        .zip(ledgers)
        .enumerate()
        .map(|(seat, (spec, ledger))| LedgerSeat {
            seat: seat as u8,
            name: spec.name.clone(),
            faction: spec.faction,
            team: state.players()[seat].team,
            bot_config: spec.bot.then(|| spec.bot_config.unwrap_or_default()),
            ledger,
        })
        .collect();
    Ok(GameLedger {
        path: path.to_owned(),
        ticks: state.current_tick(),
        seats,
    })
}

/// Loads `path` as a replay or match recording, refusing a player save.
pub fn load(path: &Path) -> Result<GameReplay> {
    let head = std::fs::read(path)
        .map(|bytes| bytes.starts_with(SAVE_MAGIC))
        .with_context(|| format!("reading {}", path.display()))?;
    if head {
        bail!(
            "{} is a player save, which keeps no command history to re-execute; \
             use the match recording (match-*.json) instead",
            path.display()
        );
    }
    oxide_kit::load_replay(path).with_context(|| format!("loading {}", path.display()))
}

/// The label a seat pools under: its faction and controller.
fn group(seat: &LedgerSeat) -> String {
    let controller = match seat.bot_config {
        None => "human".to_owned(),
        Some(config) => {
            let controller = serde_json::to_value(config.controller)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            format!("{controller} {}", config.difficulty)
        }
    };
    let faction = serde_json::to_value(seat.faction)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    format!("{faction}, {controller}")
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
                .filter(|(index, _)| {
                    ((*index as u64 + 1) * WORTH_PERIOD) % (5 * MINUTE) < WORTH_PERIOD
                })
                .map(|(index, worth)| {
                    format!("{}m {worth}", (index as u64 + 1) * WORTH_PERIOD / MINUTE)
                })
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
