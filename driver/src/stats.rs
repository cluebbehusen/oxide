//! Match statistics from a replay: the record IS the match, so any
//! number worth showing afterward is a re-execution away. Sampled
//! series (scrap, army value, unit counts) plus loss totals — the
//! Result screen's data, and a driver subcommand for anyone else.

use crate::runner::GameReplay;
use anyhow::{Context, Result};
use oxide_sim::{Event, PlayerId, State};
use serde::Serialize;

/// One player's sampled series and totals.
#[derive(Debug, Serialize)]
pub struct PlayerStats {
    /// Seat index.
    pub seat: u8,
    /// Banked scrap at each sample point.
    pub scrap: Vec<u32>,
    /// Standing army value (sum of living units' costs) per sample.
    pub army_value: Vec<u32>,
    /// Units lost across the whole match.
    pub units_lost: u32,
    /// Buildings lost across the whole match.
    pub buildings_lost: u32,
}

/// The whole report.
#[derive(Debug, Serialize)]
pub struct MatchStats {
    /// Tick of each sample column.
    pub sample_ticks: Vec<u64>,
    /// Per-seat series, seat order.
    pub players: Vec<PlayerStats>,
    /// Final tick executed.
    pub final_tick: u64,
}

/// Re-executes a replay, sampling every `every` ticks. Deterministic:
/// the same replay yields the same report, bit for bit.
pub fn compute(replay: &GameReplay, every: u64) -> Result<MatchStats> {
    let every = every.max(1);
    let mut state = replay.setup.build().context("building scenario")?;
    let mut cursor = replay.cursor();
    let total = replay
        .meta
        .ticks
        .or_else(|| replay.commands.last().map(|c| c.tick + 1))
        .unwrap_or(0);

    let seats = state.players().len();
    let mut stats: Vec<PlayerStats> = (0..seats)
        .map(|seat| PlayerStats {
            seat: seat as u8,
            scrap: Vec::new(),
            army_value: Vec::new(),
            units_lost: 0,
            buildings_lost: 0,
        })
        .collect();
    let mut sample_ticks = Vec::new();

    let mut sample = |state: &State, stats: &mut Vec<PlayerStats>, ticks: &mut Vec<u64>| {
        ticks.push(state.current_tick());
        for (seat, entry) in stats.iter_mut().enumerate() {
            entry.scrap.push(state.players()[seat].scrap);
            entry.army_value.push(
                state
                    .units()
                    .iter()
                    .filter(|u| u.player == PlayerId(seat as u8))
                    .map(|u| u.kind.stats().cost)
                    .sum(),
            );
        }
    };

    sample(&state, &mut stats, &mut sample_ticks);
    for tick in 0..total {
        let commands: Vec<_> = cursor
            .take_tick(tick)
            .iter()
            .map(|t| t.command.clone())
            .collect();
        let report = state.tick(&commands);
        for event in &report.events {
            match event {
                Event::UnitDied { player, .. } => {
                    stats[player.0 as usize].units_lost += 1;
                }
                Event::BuildingDestroyed { player, .. } => {
                    stats[player.0 as usize].buildings_lost += 1;
                }
                _ => {}
            }
        }
        if state.current_tick().is_multiple_of(every) {
            sample(&state, &mut stats, &mut sample_ticks);
        }
    }
    Ok(MatchStats {
        sample_ticks,
        players: stats,
        final_tick: state.current_tick(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner;
    use oxide_sim::Scenario;

    #[test]
    fn stats_recompute_identically_from_the_record() {
        let mut scenario = Scenario::skirmish();
        for p in scenario.players.iter_mut() {
            p.bot = true;
            p.bot_config.get_or_insert(oxide_sim::scenario::BotConfig {
                level: oxide_sim::bot::Level::Medium,
                aggression: None,
            });
        }
        let outcome = runner::run_scenario(&scenario, 600, true, true).unwrap();
        let replay = outcome.replay.unwrap();
        let a = compute(&replay, 100).unwrap();
        let b = compute(&replay, 100).unwrap();
        assert_eq!(a.final_tick, 600);
        assert_eq!(a.sample_ticks, b.sample_ticks);
        assert_eq!(a.sample_ticks.first(), Some(&0));
        for (pa, pb) in a.players.iter().zip(&b.players) {
            assert_eq!(pa.scrap, pb.scrap, "the record computes one truth");
            assert_eq!(pa.army_value, pb.army_value);
        }
        // A bot match spends and fields: the series must move.
        assert!(
            a.players
                .iter()
                .any(|p| p.army_value.iter().any(|&v| v > 0)),
            "somebody fielded an army"
        );
    }
}
