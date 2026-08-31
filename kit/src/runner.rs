//! Headless execution of scenarios and replays.

use anyhow::{Context, Result};
use chassis::replay::Replay;
use oxide_sim::bot::{SeatBot, seat_bots};
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
