//! Headless execution of scenarios and replays.

use anyhow::{Context, Result};
use chassis::replay::Replay;
use oxide_sim::bot::{DecisionTrace, SeatBot, TracedBotAct, seat_bots};
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
    /// Fresh player-facing traces in bot-seat order. Empty chairs and the
    /// frozen Overseer do not contribute rows.
    pub traces: Vec<DecisionTrace>,
}

/// Advances one tick: bots think, commands are recorded, the sim steps.
/// This is the canonical composition — every runner and shell loop should
/// look like this.
pub fn step(
    state: &mut State,
    bots: &mut [SeatBot],
    replay: Option<&mut GameReplay>,
) -> oxide_sim::TickReport {
    let mut commands: Vec<PlayerCommand> = Vec::new();
    for bot in bots.iter_mut() {
        commands.extend(bot.act(state));
    }
    record_and_tick(state, commands, replay)
}

/// Advances one tick while collecting fresh player-facing decision traces.
///
/// This uses the same command recording and state-transition path as [`step`].
/// Callers that do not need diagnostics should keep using [`step`], which does
/// not allocate a trace collection or ask bots to construct traces.
pub fn step_traced(
    state: &mut State,
    bots: &mut [SeatBot],
    replay: Option<&mut GameReplay>,
) -> TracedStep {
    let mut commands: Vec<PlayerCommand> = Vec::new();
    let mut traces = Vec::with_capacity(bots.len());
    for bot in bots.iter_mut() {
        let TracedBotAct {
            commands: bot_commands,
            trace,
        } = bot.act_traced(state);
        commands.extend(bot_commands);
        if let Some(trace) = trace {
            traces.push(trace);
        }
    }
    let report = record_and_tick(state, commands, replay);
    TracedStep { report, traces }
}

fn record_and_tick(
    state: &mut State,
    commands: Vec<PlayerCommand>,
    replay: Option<&mut GameReplay>,
) -> oxide_sim::TickReport {
    if let Some(replay) = replay {
        for command in &commands {
            replay.record(state.current_tick(), command.clone());
        }
    }
    state.tick(&commands)
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
        seat_bots(scenario).context("building public bot map briefing")?
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
    match replay.validate(Some(SIM_VERSION)) {
        Ok(()) => {}
        Err(err @ chassis::replay::ReplayError::VersionMismatch { .. })
            if allow_version_mismatch =>
        {
            eprintln!("warning: {err}; reproduction is not guaranteed");
        }
        Err(err) => return Err(err.into()),
    }
    let total = ticks_override.or(replay.meta.ticks).unwrap_or_else(|| {
        replay
            .commands
            .last()
            .map_or(0, |c| c.tick.saturating_add(1))
    });
    anyhow::ensure!(
        allow_long || total <= MAX_REPLAY_TICKS,
        "replay claims {total} ticks (limit {MAX_REPLAY_TICKS}); pass --allow-long to run it anyway"
    );
    let mut state = replay.setup.build().context("building replay setup")?;
    let mut cursor = replay.cursor();
    for _ in 0..total {
        let commands: Vec<PlayerCommand> = cursor
            .take_tick(state.current_tick())
            .iter()
            .map(|t| t.command.clone())
            .collect();
        state.tick(&commands);
    }
    if !cursor.is_finished() {
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
mod tests {
    use super::*;

    #[test]
    fn traced_step_preserves_the_authoritative_command_and_tick_path() {
        let scenario = Scenario::skirmish();
        let mut ordinary_state = scenario.build().unwrap();
        let mut traced_state = scenario.build().unwrap();
        let mut ordinary_bots = seat_bots(&scenario).unwrap();
        let mut traced_bots = seat_bots(&scenario).unwrap();
        let mut ordinary_replay = GameReplay::new(SIM_VERSION, scenario.clone());
        let mut traced_replay = GameReplay::new(SIM_VERSION, scenario);
        let mut traces = Vec::new();

        for _ in 0..25 {
            let ordinary_report = step(
                &mut ordinary_state,
                &mut ordinary_bots,
                Some(&mut ordinary_replay),
            );
            let traced = step_traced(
                &mut traced_state,
                &mut traced_bots,
                Some(&mut traced_replay),
            );
            assert_eq!(traced.report, ordinary_report);
            traces.extend(traced.traces);
        }

        assert!(!traces.is_empty(), "the configured bot should think");
        assert!(traces.iter().all(|trace| trace.player.0 == 1));
        assert!(traces.iter().all(|trace| trace.tick < 25));
        assert_eq!(traced_state.hash(), ordinary_state.hash());
        assert_eq!(
            serde_json::to_vec(&traced_replay).unwrap(),
            serde_json::to_vec(&ordinary_replay).unwrap(),
            "diagnostics must not alter replay commands"
        );
    }
}
