//! Match statistics from a replay: the record IS the match, so any
//! number worth showing afterward is a re-execution away. Sampled
//! series (scrap, army value, unit counts) plus loss totals — the
//! Result screen's data, and a driver subcommand for anyone else.

use crate::GameReplay;
use anyhow::{Context, Result};
use oxide_sim::{Event, PlayerId, State};
use serde::Serialize;
use std::collections::BTreeMap;

/// One player's sampled series and totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlayerStats {
    /// Seat index.
    pub seat: u8,
    /// Banked scrap at each sample point.
    pub scrap: Vec<u32>,
    /// Standing army value (sum of living units' costs) per sample.
    pub army_value: Vec<u32>,
    /// Living units by kind name at each sample point — the
    /// composition timeline a viewer can band-chart. BTreeMap keys keep
    /// the serialization deterministic.
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
    /// Buildings deliberately taken apart by their own crew — never
    /// counted among losses.
    pub buildings_salvaged: u32,
}

/// The whole report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
pub struct LiveMatchStats {
    stats: MatchStats,
    every: u64,
}

impl LiveMatchStats {
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

fn blank_players(seats: usize) -> Vec<PlayerStats> {
    (0..seats)
        .map(|seat| PlayerStats {
            seat: seat as u8,
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
            .filter(|unit| unit.player == PlayerId(seat as u8))
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
            Event::ScrapDeposited { player, amount } => {
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

/// Re-executes a replay, sampling every `every` ticks. Deterministic:
/// the same replay yields the same report, bit for bit.
pub fn compute(replay: &GameReplay, every: u64) -> Result<MatchStats> {
    // Untrusted input: an out-of-order or cross-version record would
    // otherwise produce a plausible, wrong report instead of an error.
    replay
        .validate(Some(oxide_sim::SIM_VERSION))
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    let every = every.max(1);
    // Bound the EFFECTIVE duration: with meta.ticks absent the run
    // length falls back to the final command's tick, and a single
    // command stamped at a billion once slipped past a meta-only guard.
    let total = replay
        .meta
        .ticks
        .or_else(|| replay.commands.last().map(|c| c.tick + 1))
        .unwrap_or(0);
    anyhow::ensure!(
        total <= crate::MAX_REPLAY_TICKS,
        "replay spans {total} ticks, beyond the {}-tick bound",
        crate::MAX_REPLAY_TICKS
    );
    let mut state = replay.setup.build().context("building scenario")?;
    let mut cursor = replay.cursor();

    let mut stats = blank_players(state.players().len());
    let mut sample_ticks = Vec::new();

    sample(&state, &mut stats, &mut sample_ticks);
    for tick in 0..total {
        let commands: Vec<_> = cursor
            .take_tick(tick)
            .iter()
            .map(|t| t.command.clone())
            .collect();
        let report = state.tick(&commands);
        accumulate_events(&mut stats, &report.events);
        if state.current_tick().is_multiple_of(every) {
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
mod tests {
    use super::*;
    use crate::runner;
    use oxide_sim::Scenario;

    /// Plays `scenario` with the Overseer driving every bot seat and
    /// records the run — a scripted, deterministic activity fixture.
    /// `opening` commands are injected (and recorded) on the first
    /// tick, ahead of the bots' own.
    fn record_overseer_match(
        scenario: &Scenario,
        ticks: u64,
        opening: Vec<oxide_sim::PlayerCommand>,
    ) -> GameReplay {
        let mut state = scenario.build().expect("scenario builds");
        let mut bots = crate::bench::overseer_bots(scenario);
        let mut replay: GameReplay =
            chassis::replay::Replay::new(oxide_sim::SIM_VERSION, scenario.clone());
        let mut opening = Some(opening);
        for _ in 0..ticks {
            let mut commands = opening.take().unwrap_or_default();
            for bot in &mut bots {
                commands.extend(bot.act(&state));
            }
            for command in &commands {
                replay.record(state.current_tick(), command.clone());
            }
            state.tick(&commands);
        }
        replay.meta.ticks = Some(state.current_tick());
        replay
    }

    fn track_replay(replay: &GameReplay) -> MatchStats {
        let total = replay.meta.ticks.expect("recorded duration");
        let mut state = replay.setup.build().expect("scenario builds");
        let mut cursor = replay.cursor();
        let mut live = LiveMatchStats::new(&state);
        for tick in 0..total {
            let commands: Vec<_> = cursor
                .take_tick(tick)
                .iter()
                .map(|timed| timed.command.clone())
                .collect();
            let report = state.tick(&commands);
            live.observe(&state, &report.events);
        }
        live.snapshot(&state)
    }

    #[test]
    fn a_claimed_billion_ticks_is_an_error_not_a_hang() {
        let mut scenario = Scenario::skirmish();
        crate::bench::all_bots(&mut scenario);
        let outcome = runner::run_scenario(&scenario, 60, true, true).unwrap();
        let mut replay = outcome.replay.unwrap();
        replay.meta.ticks = Some(1_000_000_000);
        assert!(compute(&replay, 100).is_err());
    }

    #[test]
    fn the_final_state_is_always_sampled() {
        let mut scenario = Scenario::skirmish();
        crate::bench::all_bots(&mut scenario);
        // 100 ticks with stride 41: without the closing sample the last
        // column would sit at tick 82 and closing numbers would be stale.
        let outcome = runner::run_scenario(&scenario, 100, true, true).unwrap();
        let stats = compute(&outcome.replay.unwrap(), 41).unwrap();
        assert_eq!(stats.sample_ticks.last(), Some(&100));
        assert_eq!(stats.final_tick, 100);
    }

    #[test]
    fn stats_recompute_identically_from_the_record() {
        let mut scenario = Scenario::skirmish();
        for p in scenario.players.iter_mut() {
            p.bot = true;
            p.bot_config
                .get_or_insert(oxide_sim::scenario::BotConfig::default());
        }
        let replay = record_overseer_match(&scenario, 600, Vec::new());
        let a = compute(&replay, 100).unwrap();
        let b = compute(&replay, 100).unwrap();
        assert_eq!(a.final_tick, 600);
        assert_eq!(a.sample_ticks, b.sample_ticks);
        assert_eq!(a.sample_ticks.first(), Some(&0));
        for (pa, pb) in a.players.iter().zip(&b.players) {
            assert_eq!(pa.scrap, pb.scrap, "the record computes one truth");
            assert_eq!(pa.army_value, pb.army_value);
            assert_eq!(pa.scrap_collected, pb.scrap_collected);
            assert_eq!(pa.units_trained, pb.units_trained);
            assert_eq!(pa.buildings_completed, pb.buildings_completed);
        }
        // A bot match spends and fields: the series must move.
        assert!(
            a.players
                .iter()
                .any(|p| p.army_value.iter().any(|&v| v > 0)),
            "somebody fielded an army"
        );
        assert!(
            a.players.iter().any(|p| p.scrap_collected > 0),
            "somebody delivered salvage"
        );
        assert!(
            a.players.iter().any(|p| p.units_trained > 0),
            "somebody completed production"
        );
    }

    #[test]
    fn live_tracking_matches_tick_by_tick_replay_statistics() {
        let mut scenario = Scenario::skirmish();
        for player in &mut scenario.players {
            player.bot = true;
        }
        let replay = record_overseer_match(&scenario, 40, Vec::new());
        assert_eq!(track_replay(&replay), compute(&replay, 1).unwrap());
    }

    #[test]
    fn live_tracking_stays_bounded_and_keeps_the_exact_final_tick() {
        let mut state = Scenario::skirmish().build().unwrap();
        let mut live = LiveMatchStats::new(&state);
        for _ in 0..5_000 {
            let report = state.tick(&[]);
            live.observe(&state, &report.events);
        }
        let report = live.snapshot(&state);
        assert!(report.sample_ticks.len() <= MAX_LIVE_SAMPLES + 1);
        assert_eq!(report.sample_ticks.first(), Some(&0));
        assert_eq!(report.sample_ticks.last(), Some(&5_000));
        assert_eq!(report.final_tick, 5_000);
        assert!(report.sample_ticks.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn thinned_live_bot_totals_match_recomputed_event_totals() {
        let mut scenario = Scenario::skirmish();
        for player in &mut scenario.players {
            player.bot = true;
        }
        // The Overseer harvests and trains but never constructs on this
        // map, so one scripted Turret keeps the fixture's construction
        // totals real.
        let built = scenario
            .build()
            .expect("scenario builds")
            .units()
            .iter()
            .find(|u| u.player == PlayerId(0) && u.kind == oxide_sim::UnitKind::Harvester)
            .map(|u| u.id)
            .expect("skirmish starts seat 0 with a harvester");
        let opening = vec![oxide_sim::PlayerCommand {
            player: PlayerId(0),
            command: oxide_sim::Command::Build {
                units: vec![built],
                kind: oxide_sim::BuildingKind::Turret,
                anchor: chassis::grid::TilePos::new(10, 4),
                queue: false,
                defer: false,
            },
        }];
        let replay = record_overseer_match(&scenario, 2_000, opening);
        let live = track_replay(&replay);
        let recomputed = compute(&replay, 200).unwrap();

        assert_eq!(live.final_tick, recomputed.final_tick);
        assert!(
            live.sample_ticks
                .windows(2)
                .any(|ticks| ticks[1] - ticks[0] > 1),
            "the fixture must cross adaptive thinning"
        );
        assert!(live.sample_ticks.len() <= MAX_LIVE_SAMPLES + 1);
        for (actual, expected) in live.players.iter().zip(&recomputed.players) {
            assert_eq!(actual.scrap_collected, expected.scrap_collected);
            assert_eq!(actual.units_trained, expected.units_trained);
            assert_eq!(actual.buildings_completed, expected.buildings_completed);
            assert_eq!(actual.units_lost, expected.units_lost);
            assert_eq!(actual.buildings_lost, expected.buildings_lost);
            assert_eq!(actual.buildings_salvaged, expected.buildings_salvaged);
            assert_eq!(actual.scrap.last(), expected.scrap.last());
            assert_eq!(actual.army_value.last(), expected.army_value.last());
        }
        assert!(
            live.players.iter().any(|player| {
                player.scrap_collected > 0
                    && player.units_trained > 0
                    && player.buildings_completed > 0
            }),
            "the thinned fixture must include real economy and construction events"
        );
    }

    #[test]
    fn deliberate_salvage_is_not_counted_as_a_building_loss() {
        use chassis::fx::Vec2Fx;
        use oxide_sim::BuildingId;

        let mut players = blank_players(2);
        accumulate_events(
            &mut players,
            &[
                Event::BuildingDestroyed {
                    building: BuildingId(7),
                    player: PlayerId(0),
                    pos: Vec2Fx::ZERO,
                },
                Event::BuildingSalvaged {
                    building: BuildingId(8),
                    player: PlayerId(0),
                    pos: Vec2Fx::ZERO,
                    refund: 40,
                },
            ],
        );

        assert_eq!(players[0].buildings_lost, 1);
        assert_eq!(players[0].buildings_salvaged, 1);
        assert_eq!(players[1].buildings_lost, 0);
        assert_eq!(players[1].buildings_salvaged, 0);
    }
}
