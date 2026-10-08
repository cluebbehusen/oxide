//! Headless execution of scenarios and replays.

use crate::controller::{SeatController, SeatTrace, record_events, seat_controllers};
use anyhow::{Context, Result};
use chassis::replay::Replay;
use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario, State};

/// The concrete replay type for Oxide sessions.
pub use crate::GameReplay;

/// A finished headless run.
pub struct RunOutcome {
    /// Final state.
    pub state: State,
    /// The recording, when one was requested.
    pub replay: Option<GameReplay>,
}

/// One authoritative tick plus the optional player-facing decisions produced
/// while its configured bots chose commands.
///
/// Decision traces are diagnostics only. The command list remains the sole
/// input to the simulation and replay recorder.
pub struct TracedStep {
    /// The simulation report produced by the tick.
    pub report: oxide_sim::TickReport,
    /// Fresh player-facing traces in bot-seat order; empty chairs contribute no rows.
    pub traces: Vec<SeatTrace>,
}

/// Advances one tick: bots think, commands are recorded, the sim steps.
/// This is the canonical composition — every runner and shell loop should
/// look like this.
pub fn step(
    state: &mut State,
    bots: &mut [SeatController],
    replay: Option<&mut GameReplay>,
) -> oxide_sim::TickReport {
    let commands = crate::bot_execution::commands(state, bots);
    let report = record_and_tick(state, &commands, replay);
    record_events(bots, &report);
    report
}

/// Advances one tick while collecting fresh player-facing decision traces.
///
/// This uses the same command recording and state-transition path as [`step`].
/// Callers that do not need diagnostics should keep using [`step`], which does
/// not allocate a trace collection or ask bots to construct traces. Diagnostic
/// collection runs serially; ordinary steps may think across seats in parallel.
pub fn step_traced(
    state: &mut State,
    bots: &mut [SeatController],
    replay: Option<&mut GameReplay>,
) -> TracedStep {
    let mut commands: Vec<PlayerCommand> = Vec::new();
    let mut traces = Vec::with_capacity(bots.len());
    for bot in bots.iter_mut() {
        let (bot_commands, trace) = bot.act_traced(state);
        commands.extend(bot_commands);
        if let Some(trace) = trace {
            traces.push(trace);
        }
    }
    let report = record_and_tick(state, &commands, replay);
    record_events(bots, &report);
    TracedStep { report, traces }
}

/// Records `commands` at the current tick, then executes them as one tick.
pub fn record_and_tick(
    state: &mut State,
    commands: &[PlayerCommand],
    replay: Option<&mut GameReplay>,
) -> oxide_sim::TickReport {
    if let Some(replay) = replay {
        for command in commands {
            replay.record(state.current_tick(), command.clone());
        }
    }
    state.tick(commands)
}

/// Runs `scenario` for `ticks` ticks (frozen post-victory ticks included, so
/// the count always lands where asked).
pub fn run_scenario(
    scenario: &Scenario,
    ticks: u64,
    with_bots: bool,
    record: bool,
) -> Result<RunOutcome> {
    let mut state = scenario.build().context("building scenario")?;
    let mut bots = if with_bots {
        seat_controllers(scenario).context("building public bot map briefing")?
    } else {
        Vec::new()
    };
    let mut replay = record.then(|| Replay::new(SIM_VERSION, scenario.clone()));
    for _ in 0..ticks {
        step(&mut state, &mut bots, replay.as_mut());
    }
    if let Some(replay) = &mut replay {
        replay.meta.ticks = Some(state.current_tick());
    }
    Ok(RunOutcome { state, replay })
}

/// Longest replay the driver runs without an explicit override — a forged
/// duration must not spin the process forever. ~28 game-hours.
pub use crate::MAX_REPLAY_TICKS;

/// Re-executes a recorded run and returns the final state. With no override,
/// the length comes from the replay's own metadata (falling back to the last
/// command tick for hand-written files), bounded by [`MAX_REPLAY_TICKS`].
///
/// The replay is validated first — structure always, version too unless
/// `allow_version_mismatch` (which downgrades the mismatch to a warning for
/// deliberate archaeology). Playback that fails to consume every command is
/// an error, not a shrug.
pub fn run_replay(
    replay: &GameReplay,
    ticks_override: Option<u64>,
    allow_version_mismatch: bool,
) -> Result<State> {
    run_replay_bounded(replay, ticks_override, allow_version_mismatch, false)
}

/// [`run_replay`] with the length bound overridable (`allow_long`) for
/// deliberate marathon reproductions.
pub fn run_replay_bounded(
    replay: &GameReplay,
    ticks_override: Option<u64>,
    allow_version_mismatch: bool,
    allow_long: bool,
) -> Result<State> {
    run_replay_observed(
        replay,
        ticks_override,
        None,
        allow_version_mismatch,
        allow_long,
        |_, _| {},
    )
}

/// [`run_replay_bounded`], stopping at `until` when it falls short of the run's
/// length and handing the state after each tick and that tick's report to
/// `observe`. A stop short of the length is a prefix: commands after it stay
/// unplayed. Only a run to its full length must consume every command.
pub fn run_replay_observed(
    replay: &GameReplay,
    ticks_override: Option<u64>,
    until: Option<u64>,
    allow_version_mismatch: bool,
    allow_long: bool,
    mut observe: impl FnMut(&State, &oxide_sim::TickReport),
) -> Result<State> {
    match replay.validate(Some(SIM_VERSION)) {
        Ok(()) => {}
        Err(err @ chassis::replay::ReplayError::VersionMismatch { .. })
            if allow_version_mismatch =>
        {
            eprintln!("warning: {err}; reproduction is not guaranteed");
        }
        Err(err) => return Err(err.into()),
    }
    let length = ticks_override.unwrap_or_else(|| crate::replay_duration(replay));
    let total = until.map_or(length, |until| until.min(length));
    anyhow::ensure!(
        allow_long || total <= MAX_REPLAY_TICKS,
        "replay claims {total} ticks (limit {MAX_REPLAY_TICKS}); pass --allow-long to run it anyway"
    );
    anyhow::ensure!(
        total >= replay.start_tick(),
        "requested end precedes recording origin"
    );
    let mut state = crate::recording::initial_state(replay)?;
    let mut playback = crate::ReplayPlayback::new(replay);
    for _ in state.current_tick()..total {
        let report = playback.step(&mut state);
        observe(&state, &report);
    }
    if total == length && !playback.is_finished() {
        anyhow::bail!(
            "playback of {total} ticks left recorded commands unconsumed — \
             the replay's duration metadata is wrong"
        );
    }
    Ok(state)
}

/// Loads a scenario by path, with `"skirmish"` as a built-in shorthand.
pub fn load_scenario(name: &str) -> Result<Scenario> {
    if name == "skirmish" {
        Ok(Scenario::skirmish())
    } else {
        Scenario::load(name).with_context(|| format!("loading scenario {name}"))
    }
}

#[cfg(test)]
mod tests;
