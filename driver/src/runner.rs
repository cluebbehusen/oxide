//! Headless execution of scenarios and replays.

use anyhow::{Context, Result};
use chassis::replay::Replay;
use oxide_sim::bot::Bot;
use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario, State};

/// The concrete replay type for Oxide sessions.
pub type GameReplay = Replay<Scenario, PlayerCommand>;

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
    bots: &mut [Bot],
    mut replay: Option<&mut GameReplay>,
) -> oxide_sim::TickReport {
    let mut commands: Vec<PlayerCommand> = Vec::new();
    for bot in bots.iter_mut() {
        commands.extend(bot.act(state));
    }
    if let Some(replay) = replay.as_deref_mut() {
        for command in &commands {
            replay.record(state.tick, command.clone());
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
        Bot::for_scenario(scenario)
    } else {
        Vec::new()
    };
    let mut replay = record.then(|| Replay::new(SIM_VERSION, scenario.clone()));
    for _ in 0..ticks {
        step(&mut state, &mut bots, replay.as_mut());
    }
    if let Some(replay) = &mut replay {
        replay.meta.ticks = Some(state.tick);
    }
    Ok(RunOutcome { state, replay })
}

/// Re-executes a recorded run and returns the final state. With no override,
/// the length comes from the replay's own metadata (falling back to the last
/// command tick for hand-written files).
pub fn run_replay(replay: &GameReplay, ticks_override: Option<u64>) -> Result<State> {
    if replay.meta.sim_version != SIM_VERSION {
        eprintln!(
            "warning: replay was recorded on sim {} but this is {SIM_VERSION}; \
             reproduction is not guaranteed",
            replay.meta.sim_version
        );
    }
    let total = ticks_override
        .or(replay.meta.ticks)
        .unwrap_or_else(|| replay.commands.last().map_or(0, |c| c.tick + 1));
    let mut state = replay.setup.build().context("building replay setup")?;
    let mut cursor = replay.cursor();
    for _ in 0..total {
        let commands: Vec<PlayerCommand> = cursor
            .take_tick(state.tick)
            .iter()
            .map(|t| t.command.clone())
            .collect();
        state.tick(&commands);
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
