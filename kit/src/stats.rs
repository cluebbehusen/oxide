//! Match statistics: sampled series (scrap, army value, unit counts) plus
//! totals, computed by re-executing a replay (`compute`) or incrementally
//! during a live match (`LiveMatchStats`).

use crate::GameReplay;
use anyhow::Result;
use oxide_sim::{Event, PlayerId, State};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One player's sampled series and totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerStats {
    /// Seat index.
    pub seat: u8,
    /// Banked scrap at each sample point.
    pub scrap: Vec<u32>,
    /// Standing army value (sum of living units' costs) per sample.
    pub army_value: Vec<u32>,
    /// Living units by kind name at each sample point. `BTreeMap` keys keep
    /// the serialization deterministic.
    #[serde(deserialize_with = "deserialize_kinds")]
    pub kinds: Vec<BTreeMap<&'static str, u16>>,
    /// Scrap brought home by Harvesters across the whole match.
    pub scrap_collected: u32,
    /// Units completed across the whole match.
    pub units_trained: u32,
    /// Buildings completed across the whole match.
    pub buildings_completed: u32,
    /// Units lost across the whole match.
    pub units_lost: u32,
    /// Buildings lost across the whole match.
    pub buildings_lost: u32,
    /// Buildings deliberately taken apart by their own crew; never counted
    /// among losses.
    pub buildings_salvaged: u32,
}

/// The whole report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchStats {
    /// Tick of each sample column.
    pub sample_ticks: Vec<u64>,
    /// Per-seat series, seat order.
    pub players: Vec<PlayerStats>,
    /// Final tick executed.
    pub final_tick: u64,
}

/// Maximum retained graph columns while a live match runs. The closing
/// snapshot can append one exact final column beyond this bound.
const MAX_LIVE_SAMPLES: usize = 49;

/// Incremental statistics for a live match.
///
/// Totals consume the same deterministic tick events as [`compute`]. Graph
/// samples thin themselves by powers of two, keeping memory and end-of-match
/// work bounded no matter how long the session runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveMatchStats {
    stats: MatchStats,
    every: u64,
}

impl LiveMatchStats {
    pub(crate) fn validate_checkpoint(&self, state: &State) -> Result<()> {
        self.stats.validate_checkpoint(state)?;
        anyhow::ensure!(
            self.stats.final_tick == state.current_tick(),
            "statistics tick mismatch"
        );
        anyhow::ensure!(self.every.is_power_of_two(), "invalid statistics stride");
        anyhow::ensure!(
            self.stats.sample_ticks.len() <= MAX_LIVE_SAMPLES,
            "too many live samples"
        );
        anyhow::ensure!(
            self.stats
                .sample_ticks
                .iter()
                .all(|tick| tick.is_multiple_of(self.every)),
            "statistics samples disagree with stride"
        );
        Ok(())
    }
    /// Starts tracking from the current state, including an exact opening
    /// sample.
    pub fn new(state: &State) -> Self {
        let mut stats = MatchStats {
            sample_ticks: Vec::new(),
            players: blank_players(state.players().len()),
            final_tick: state.current_tick(),
        };
        sample(state, &mut stats.players, &mut stats.sample_ticks);
        Self { stats, every: 1 }
    }

    /// Consumes one completed tick's state and events.
    pub fn observe(&mut self, state: &State, events: &[Event]) {
        accumulate_events(&mut self.stats.players, events);
        self.stats.final_tick = state.current_tick();
        if state.current_tick().is_multiple_of(self.every) {
            sample(state, &mut self.stats.players, &mut self.stats.sample_ticks);
        }
        while self.stats.sample_ticks.len() > MAX_LIVE_SAMPLES {
            let next = self.every.saturating_mul(2);
            if next == self.every {
                break;
            }
            self.every = next;
            thin_samples(&mut self.stats, self.every);
        }
    }

    /// Clones the bounded report and appends an exact sample of `state` when
    /// the current thinning stride did not land on it.
    pub fn snapshot(&self, state: &State) -> MatchStats {
        let mut report = self.stats.clone();
        report.final_tick = state.current_tick();
        if report.sample_ticks.last() != Some(&state.current_tick()) {
            sample(state, &mut report.players, &mut report.sample_ticks);
        }
        report
    }
}

impl MatchStats {
    /// Validates a retained report against its session's seats and time boundary.
    pub fn validate_checkpoint(&self, state: &State) -> Result<()> {
        anyhow::ensure!(
            self.final_tick <= state.current_tick(),
            "statistics are from the future"
        );
        anyhow::ensure!(
            !self.sample_ticks.is_empty() && self.sample_ticks.len() <= MAX_LIVE_SAMPLES + 1,
            "invalid statistics sample count"
        );
        anyhow::ensure!(
            self.sample_ticks.windows(2).all(|pair| pair[0] < pair[1])
                && self
                    .sample_ticks
                    .last()
                    .is_some_and(|tick| *tick <= self.final_tick),
            "invalid statistics sample times"
        );
        anyhow::ensure!(
            self.players.len() == state.players().len(),
            "statistics seat count mismatch"
        );
        for (seat, player) in self.players.iter().enumerate() {
            let samples = self.sample_ticks.len();
            anyhow::ensure!(
                usize::from(player.seat) == seat
                    && player.scrap.len() == samples
                    && player.army_value.len() == samples
                    && player.kinds.len() == samples,
                "statistics columns or seat mismatch"
            );
        }
        Ok(())
    }
}

fn deserialize_kinds<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<BTreeMap<&'static str, u16>>, D::Error> {
    Vec::<BTreeMap<String, u16>>::deserialize(deserializer)?
        .into_iter()
        .map(|sample| {
            sample
                .into_iter()
                .map(|(name, count)| {
                    oxide_sim::UnitKind::ALL
                        .into_iter()
                        .find(|kind| kind.name() == name)
                        .map(|kind| (kind.name(), count))
                        .ok_or_else(|| {
                            serde::de::Error::custom(format!("unknown unit kind {name}"))
                        })
                })
                .collect()
        })
        .collect()
}

fn blank_players(seats: usize) -> Vec<PlayerStats> {
    (0..seats)
        .map(|seat| PlayerStats {
            seat: u8::try_from(seat).expect("seat indices fit in u8"),
            scrap: Vec::new(),
            army_value: Vec::new(),
            kinds: Vec::new(),
            scrap_collected: 0,
            units_trained: 0,
            buildings_completed: 0,
            units_lost: 0,
            buildings_lost: 0,
            buildings_salvaged: 0,
        })
        .collect()
}

fn sample(state: &State, stats: &mut [PlayerStats], ticks: &mut Vec<u64>) {
    ticks.push(state.current_tick());
    for (seat, entry) in stats.iter_mut().enumerate() {
        entry.scrap.push(state.players()[seat].scrap);
        let mut value = 0u32;
        let mut counts: BTreeMap<&'static str, u16> = BTreeMap::new();
        for unit in state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId::from_index(seat))
        {
            value = value.saturating_add(unit.kind.stats().cost);
            *counts.entry(unit.kind.name()).or_default() += 1;
        }
        entry.army_value.push(value);
        entry.kinds.push(counts);
    }
}

fn accumulate_events(stats: &mut [PlayerStats], events: &[Event]) {
    for event in events {
        match event {
            Event::ScrapDeposited { player, amount, .. } => {
                stats[player.0 as usize].scrap_collected = stats[player.0 as usize]
                    .scrap_collected
                    .saturating_add(*amount);
            }
            Event::UnitTrained { player, .. } => {
                stats[player.0 as usize].units_trained += 1;
            }
            Event::BuildingCompleted { player, .. } => {
                stats[player.0 as usize].buildings_completed += 1;
            }
            Event::UnitDied { player, .. } => {
                stats[player.0 as usize].units_lost += 1;
            }
            Event::BuildingDestroyed { player, .. } => {
                stats[player.0 as usize].buildings_lost += 1;
            }
            Event::BuildingSalvaged { player, .. } => {
                stats[player.0 as usize].buildings_salvaged += 1;
            }
            _ => {}
        }
    }
}

fn thin_samples(stats: &mut MatchStats, every: u64) {
    let keep: Vec<usize> = stats
        .sample_ticks
        .iter()
        .enumerate()
        .filter_map(|(index, tick)| tick.is_multiple_of(every).then_some(index))
        .collect();
    stats.sample_ticks = keep
        .iter()
        .map(|index| stats.sample_ticks[*index])
        .collect();
    for player in &mut stats.players {
        player.scrap = keep.iter().map(|index| player.scrap[*index]).collect();
        player.army_value = keep.iter().map(|index| player.army_value[*index]).collect();
        player.kinds = keep
            .iter()
            .map(|index| player.kinds[*index].clone())
            .collect();
    }
}

/// Re-executes a replay, sampling every `every` ticks. The same replay
/// yields the same report.
pub fn compute(replay: &GameReplay, every: u64) -> Result<MatchStats> {
    // Untrusted input: an out-of-order or cross-version record would
    // otherwise produce a plausible, wrong report instead of an error.
    replay
        .validate(Some(oxide_sim::SIM_VERSION))
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    let every = every.max(1);
    let total = crate::bounded_replay_duration(replay)?;
    let mut state = crate::recording::initial_state(replay)?;
    let mut playback = crate::ReplayPlayback::new(replay);

    let mut stats = blank_players(state.players().len());
    let mut sample_ticks = Vec::new();

    let start = state.current_tick();
    sample(&state, &mut stats, &mut sample_ticks);
    for _ in start..total {
        let report = playback.step(&mut state);
        accumulate_events(&mut stats, &report.events);
        if (state.current_tick() - start).is_multiple_of(every) {
            sample(&state, &mut stats, &mut sample_ticks);
        }
    }
    // The outcome always makes the record: without this, any length not
    // divisible by the stride reports stale closing numbers.
    if sample_ticks.last() != Some(&state.current_tick()) {
        sample(&state, &mut stats, &mut sample_ticks);
    }
    Ok(MatchStats {
        sample_ticks,
        players: stats,
        final_tick: state.current_tick(),
    })
}

#[cfg(test)]
mod tests;
