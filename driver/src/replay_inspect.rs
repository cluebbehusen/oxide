//! Deterministic, machine-readable replay inspection.
//!
//! This is deliberately a read-only view over the same replay execution path
//! as the rest of the driver. A snapshot at tick `N` is the state whose
//! [`oxide_sim::State::current_tick`] is exactly `N`: commands stamped `N`
//! have not executed yet.

use crate::runner::{GameReplay, MAX_REPLAY_TICKS};
use anyhow::{Context, Result};
use chassis::replay::ReplayMeta;
use oxide_protocol::{FogView, StateFilter, StateView, hash_hex};
use oxide_sim::scenario::BotConfig;
use oxide_sim::{Command, Faction, GameResult, PlayerCommand, PlayerId, SIM_VERSION};
use serde::Serialize;
use std::collections::BTreeMap;

/// Version of the serialized [`ReplayInspection`] contract.
pub const REPLAY_INSPECTION_SCHEMA_VERSION: u32 = 1;

/// A stable machine-readable inspection of one replay or save.
#[derive(Debug, Serialize)]
pub struct ReplayInspection {
    /// Serialization contract version.
    pub schema_version: u32,
    /// Provenance embedded in the replay.
    pub metadata: ReplayMeta,
    /// Reproducible starting-match facts.
    pub scenario: ScenarioSummary,
    /// Outcome after the replay's complete recorded duration.
    pub final_state: FinalStateSummary,
    /// Per-seat command counts and longest inactivity windows.
    pub command_activity: Vec<SeatCommandActivity>,
    /// Exact state snapshots, sorted by tick with duplicates removed.
    pub snapshots: Vec<ReplaySnapshot>,
}

/// Scenario facts useful when orienting a replay.
#[derive(Debug, Serialize)]
pub struct ScenarioSummary {
    /// Scenario display name.
    pub name: String,
    /// Deterministic scenario seed.
    pub seed: u64,
    /// Map width in tiles.
    pub map_width: i32,
    /// Map height in tiles.
    pub map_height: i32,
    /// Seats in player-id order.
    pub players: Vec<ReplayPlayerSummary>,
}

/// One replay seat's starting identity and controller configuration.
#[derive(Debug, Serialize)]
pub struct ReplayPlayerSummary {
    /// Player id.
    pub seat: u8,
    /// Display name.
    pub name: String,
    /// Unit roster and sprite tint.
    pub faction: Faction,
    /// Normalized team id used by the simulation.
    pub team: u8,
    /// Whether the scenario assigns this seat to a built-in bot.
    pub bot: bool,
    /// Authored neural ladder configuration, or `None` for a human or legacy bot.
    pub bot_config: Option<BotConfig>,
}

/// Final replay outcome and state fingerprint.
#[derive(Debug, Serialize)]
pub struct FinalStateSummary {
    /// Final state tick after executing the complete replay duration.
    pub tick: u64,
    /// Exact deterministic state hash.
    pub hash: String,
    /// Match result, or `None` when the saved session was still in progress.
    pub result: Option<GameResult>,
    /// Seats belonging to the victorious team, empty for a draw or unfinished match.
    pub winner_seats: Vec<u8>,
    /// Living units at the final tick.
    pub units: u64,
    /// Standing buildings at the final tick.
    pub buildings: u64,
    /// Commands recorded across all seats.
    pub recorded_commands: u64,
}

/// Command activity for one player id.
#[derive(Debug, Serialize)]
pub struct SeatCommandActivity {
    /// Player id that issued the commands.
    pub seat: u8,
    /// Number of recorded commands, including multiple commands on one tick.
    pub command_count: u64,
    /// First tick carrying a command from this seat.
    pub first_command_tick: Option<u64>,
    /// Last tick carrying a command from this seat.
    pub last_command_tick: Option<u64>,
    /// Counts by the command's stable snake-case variant name.
    pub by_type: BTreeMap<String, u64>,
    /// Longest span between match boundaries and distinct command ticks.
    pub longest_silence: CommandSilence,
}

/// One seat's longest command-free span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CommandSilence {
    /// Tick at the start of the span.
    pub from_tick: u64,
    /// Tick at the end of the span.
    pub to_tick: u64,
    /// `to_tick - from_tick`.
    pub duration_ticks: u64,
    /// What establishes the start of the span.
    pub start_boundary: SilenceBoundary,
    /// What establishes the end of the span.
    pub end_boundary: SilenceBoundary,
}

/// Boundary kind for a command-silence span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SilenceBoundary {
    /// The replay starts at tick zero.
    MatchStart,
    /// The seat issued one or more commands at this tick.
    Command,
    /// The replay's recorded duration ends at this tick.
    MatchEnd,
}

/// Exact state at one requested replay tick.
#[derive(Debug, Serialize)]
pub struct ReplaySnapshot {
    /// Requested state tick. Commands stamped with this tick execute next.
    pub tick: u64,
    /// Omniscient readable state plus its exact deterministic hash.
    pub state: StateView,
    /// Fog-honest knowledge for the requested seat, when supplied.
    pub fog: Option<FogView>,
}

#[derive(Default)]
struct ActivityBuilder {
    command_count: u64,
    first_command_tick: Option<u64>,
    last_command_tick: Option<u64>,
    by_type: BTreeMap<String, u64>,
    command_ticks: Vec<u64>,
}

/// Re-executes `replay` once and returns snapshots and forensic summaries.
///
/// Requested ticks are sorted and deduplicated. With no requested ticks, the
/// final state is captured. Tick `N` means the state at the start of that
/// simulation tick, before commands stamped `N` execute.
pub fn inspect(
    replay: &GameReplay,
    requested_ticks: &[u64],
    fog_seat: Option<PlayerId>,
    include_map: bool,
) -> Result<ReplayInspection> {
    replay.validate(Some(SIM_VERSION))?;
    let total = replay_duration(replay);
    anyhow::ensure!(
        total <= MAX_REPLAY_TICKS,
        "replay spans {total} ticks, beyond the {MAX_REPLAY_TICKS}-tick bound"
    );

    let mut state = replay.setup.build().context("building replay setup")?;
    if let Some(seat) = fog_seat {
        anyhow::ensure!(
            state.try_player(seat).is_some(),
            "fog seat {} is outside this replay's {} seats",
            seat.0,
            state.players().len()
        );
    }

    let mut ticks = if requested_ticks.is_empty() {
        vec![total]
    } else {
        requested_ticks.to_vec()
    };
    ticks.sort_unstable();
    ticks.dedup();
    if let Some(&tick) = ticks.iter().find(|&&tick| tick > total) {
        anyhow::bail!("snapshot tick {tick} exceeds replay duration {total}");
    }

    let command_activity = command_activity(replay, total);
    let scenario = scenario_summary(replay, &state);
    let mut snapshots = Vec::with_capacity(ticks.len());
    let mut next_snapshot = 0;
    if ticks.first() == Some(&0) {
        snapshots.push(capture_snapshot(&state, fog_seat, include_map));
        next_snapshot = 1;
    }

    let mut cursor = replay.cursor();
    for tick in 0..total {
        let commands: Vec<PlayerCommand> = cursor
            .take_tick(tick)
            .iter()
            .map(|timed| timed.command.clone())
            .collect();
        state.tick(&commands);
        if ticks.get(next_snapshot) == Some(&state.current_tick()) {
            snapshots.push(capture_snapshot(&state, fog_seat, include_map));
            next_snapshot += 1;
        }
    }
    anyhow::ensure!(
        cursor.is_finished(),
        "playback of {total} ticks left recorded commands unconsumed"
    );
    debug_assert_eq!(next_snapshot, ticks.len());

    let final_state = FinalStateSummary {
        tick: state.current_tick(),
        hash: hash_hex(state.hash()),
        result: state.result(),
        winner_seats: state.winners().into_iter().map(|seat| seat.0).collect(),
        units: state.units().len() as u64,
        buildings: state.buildings().len() as u64,
        recorded_commands: replay.commands.len() as u64,
    };

    Ok(ReplayInspection {
        schema_version: REPLAY_INSPECTION_SCHEMA_VERSION,
        metadata: replay.meta.clone(),
        scenario,
        final_state,
        command_activity,
        snapshots,
    })
}

fn replay_duration(replay: &GameReplay) -> u64 {
    replay
        .meta
        .ticks
        .unwrap_or_else(|| replay.commands.last().map_or(0, |command| command.tick + 1))
}

fn scenario_summary(replay: &GameReplay, initial_state: &oxide_sim::State) -> ScenarioSummary {
    let players = replay
        .setup
        .players
        .iter()
        .enumerate()
        .map(|(seat, spec)| ReplayPlayerSummary {
            seat: seat as u8,
            name: spec.name.clone(),
            faction: spec.faction,
            team: initial_state.players()[seat].team,
            bot: spec.bot,
            bot_config: spec.bot_config,
        })
        .collect();
    ScenarioSummary {
        name: replay.setup.name.clone(),
        seed: replay.setup.seed,
        map_width: initial_state.map().width(),
        map_height: initial_state.map().height(),
        players,
    }
}

fn capture_snapshot(
    state: &oxide_sim::State,
    fog_seat: Option<PlayerId>,
    include_map: bool,
) -> ReplaySnapshot {
    ReplaySnapshot {
        tick: state.current_tick(),
        state: StateView::capture(
            state,
            StateFilter {
                map: include_map,
                ..StateFilter::default()
            },
        ),
        fog: fog_seat.map(|seat| FogView::capture(state, seat)),
    }
}

fn command_activity(replay: &GameReplay, total: u64) -> Vec<SeatCommandActivity> {
    let mut builders: BTreeMap<u8, ActivityBuilder> = replay
        .setup
        .players
        .iter()
        .enumerate()
        .map(|(seat, _)| (seat as u8, ActivityBuilder::default()))
        .collect();
    for timed in &replay.commands {
        let seat = timed.command.player.0;
        let entry = builders.entry(seat).or_default();
        entry.command_count += 1;
        entry.first_command_tick.get_or_insert(timed.tick);
        entry.last_command_tick = Some(timed.tick);
        *entry
            .by_type
            .entry(command_name(&timed.command.command).to_owned())
            .or_default() += 1;
        if entry.command_ticks.last() != Some(&timed.tick) {
            entry.command_ticks.push(timed.tick);
        }
    }
    builders
        .into_iter()
        .map(|(seat, entry)| SeatCommandActivity {
            seat,
            command_count: entry.command_count,
            first_command_tick: entry.first_command_tick,
            last_command_tick: entry.last_command_tick,
            by_type: entry.by_type,
            longest_silence: longest_silence(&entry.command_ticks, total),
        })
        .collect()
}

fn longest_silence(command_ticks: &[u64], total: u64) -> CommandSilence {
    if command_ticks.is_empty() {
        return CommandSilence {
            from_tick: 0,
            to_tick: total,
            duration_ticks: total,
            start_boundary: SilenceBoundary::MatchStart,
            end_boundary: SilenceBoundary::MatchEnd,
        };
    }

    let mut best: Option<CommandSilence> = None;
    let mut consider = |from_tick, to_tick, start_boundary, end_boundary| {
        let candidate = CommandSilence {
            from_tick,
            to_tick,
            duration_ticks: to_tick - from_tick,
            start_boundary,
            end_boundary,
        };
        if best.is_none_or(|current| candidate.duration_ticks > current.duration_ticks) {
            best = Some(candidate);
        }
    };

    if command_ticks[0] > 0 {
        consider(
            0,
            command_ticks[0],
            SilenceBoundary::MatchStart,
            SilenceBoundary::Command,
        );
    }
    for pair in command_ticks.windows(2) {
        consider(
            pair[0],
            pair[1],
            SilenceBoundary::Command,
            SilenceBoundary::Command,
        );
    }
    let last = command_ticks[command_ticks.len() - 1];
    consider(
        last,
        total,
        SilenceBoundary::Command,
        SilenceBoundary::MatchEnd,
    );
    best.expect("a nonempty command timeline always has a closing boundary")
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Move { .. } => "move",
        Command::Attack { .. } => "attack",
        Command::AttackMove { .. } => "attack_move",
        Command::Harvest { .. } => "harvest",
        Command::Patrol { .. } => "patrol",
        Command::Stop { .. } => "stop",
        Command::Train { .. } => "train",
        Command::Build { .. } => "build",
        Command::Cancel { .. } => "cancel",
        Command::Repair { .. } => "repair",
        Command::Salvage { .. } => "salvage",
        Command::CancelTrain { .. } => "cancel_train",
        Command::SetRally { .. } => "set_rally",
        Command::Surrender => "surrender",
        Command::RepairUnit { .. } => "repair_unit",
        Command::Advance { .. } => "advance",
        Command::FocusFire { .. } => "focus_fire",
        Command::CancelFound { .. } => "cancel_found",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chassis::replay::Replay;
    use oxide_sim::{PlayerCommand, Scenario, UnitId};

    fn stop(player: u8, unit: UnitId) -> PlayerCommand {
        PlayerCommand {
            player: PlayerId(player),
            command: Command::Stop { units: vec![unit] },
        }
    }

    fn fixture() -> GameReplay {
        let scenario = Scenario::skirmish();
        let state = scenario.build().expect("skirmish builds");
        let unit = |seat| {
            state
                .units()
                .iter()
                .find(|unit| unit.player == PlayerId(seat))
                .expect("each skirmish seat starts with a unit")
                .id
        };
        let mut replay = Replay::new(SIM_VERSION, scenario);
        replay.record(0, stop(0, unit(0)));
        replay.record(3, stop(1, unit(1)));
        replay.record(5, stop(0, unit(0)));
        replay.record(8, stop(1, unit(1)));
        replay.record(10, stop(0, unit(0)));
        replay.meta.ticks = Some(12);
        replay
    }

    #[test]
    fn inspection_captures_exact_sorted_ticks_and_fog() {
        let report =
            inspect(&fixture(), &[12, 5, 0, 5], Some(PlayerId(1)), true).expect("fixture inspects");

        assert_eq!(report.schema_version, 1);
        assert_eq!(report.scenario.name, "Skirmish Basin");
        assert_eq!(report.scenario.players[1].seat, 1);
        assert!(report.scenario.players[1].bot);
        assert!(report.scenario.players[1].bot_config.is_some());
        assert_eq!(
            report
                .snapshots
                .iter()
                .map(|snapshot| snapshot.tick)
                .collect::<Vec<_>>(),
            vec![0, 5, 12]
        );
        for snapshot in &report.snapshots {
            assert_eq!(snapshot.state.tick, snapshot.tick);
            assert!(snapshot.state.map.is_some());
            let fog = snapshot.fog.as_ref().expect("fog requested");
            assert_eq!(fog.tick, snapshot.tick);
            assert_eq!(fog.player, 1);
        }
        assert_eq!(
            report.final_state.hash,
            report.snapshots.last().expect("final snapshot").state.hash
        );
    }

    #[test]
    fn activity_reports_command_counts_types_and_earliest_longest_gap() {
        let report = inspect(&fixture(), &[], None, false).expect("fixture inspects");
        let seat0 = &report.command_activity[0];
        assert_eq!(seat0.command_count, 3);
        assert_eq!(seat0.first_command_tick, Some(0));
        assert_eq!(seat0.last_command_tick, Some(10));
        assert_eq!(seat0.by_type.get("stop"), Some(&3));
        assert_eq!(
            seat0.longest_silence,
            CommandSilence {
                from_tick: 0,
                to_tick: 5,
                duration_ticks: 5,
                start_boundary: SilenceBoundary::Command,
                end_boundary: SilenceBoundary::Command,
            }
        );

        let seat1 = &report.command_activity[1];
        assert_eq!(seat1.command_count, 2);
        assert_eq!(
            seat1.longest_silence,
            CommandSilence {
                from_tick: 3,
                to_tick: 8,
                duration_ticks: 5,
                start_boundary: SilenceBoundary::Command,
                end_boundary: SilenceBoundary::Command,
            }
        );
    }

    #[test]
    fn inspection_rejects_snapshot_and_fog_seats_outside_the_replay() {
        let replay = fixture();
        let tick_error = inspect(&replay, &[13], None, false).expect_err("tick is too late");
        assert!(
            tick_error
                .to_string()
                .contains("exceeds replay duration 12")
        );

        let fog_error =
            inspect(&replay, &[], Some(PlayerId(2)), false).expect_err("seat does not exist");
        assert!(
            fog_error
                .to_string()
                .contains("outside this replay's 2 seats")
        );
    }
}
