//! Oxide's replay file boundary.

use crate::GameReplay;
use chassis::replay::{Replay, ReplayError, ReplayMeta, TimedCommand};
use oxide_sim::scenario::{
    BotConfig, BuildingSpec, PlayerSpec, ScenarioMeta, ScenarioMode, UnitSpec,
};
use oxide_sim::{Faction, PlayerCommand, SIM_VERSION, Scenario};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayWire {
    meta: ReplayMeta,
    setup: ReplayScenarioWire,
    commands: Vec<TimedCommand<PlayerCommand>>,
    #[serde(default)]
    origin: Option<crate::recording::WorldOrigin>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayScenarioWire {
    #[serde(default)]
    mode: ScenarioMode,
    name: String,
    seed: u64,
    map: Vec<String>,
    players: Vec<ReplayPlayerSpecWire>,
    #[serde(default)]
    units: Vec<UnitSpec>,
    #[serde(default)]
    buildings: Vec<BuildingSpec>,
    #[serde(default)]
    meta: Option<ScenarioMeta>,
}

impl ReplayScenarioWire {
    fn into_current(self, recorded_version: &str) -> Result<Scenario, ReplayError> {
        let players = self
            .players
            .into_iter()
            .enumerate()
            .map(|(seat, player)| player.into_current(recorded_version, seat))
            .collect::<Result<_, _>>()?;
        Ok(Scenario {
            mode: self.mode,
            name: self.name,
            seed: self.seed,
            map: self.map,
            players,
            units: self.units,
            buildings: self.buildings,
            meta: self.meta,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayPlayerSpecWire {
    name: String,
    faction: Faction,
    #[serde(default)]
    team: Option<u8>,
    #[serde(default = "default_scrap")]
    scrap: u32,
    #[serde(default)]
    bot: bool,
    #[serde(default)]
    bot_config: Option<ReplayBotConfigWire>,
}

impl ReplayPlayerSpecWire {
    fn into_current(self, recorded_version: &str, seat: usize) -> Result<PlayerSpec, ReplayError> {
        let bot_config = match self.bot_config {
            None => None,
            Some(ReplayBotConfigWire::Current(config)) => Some(config),
            Some(ReplayBotConfigWire::Legacy(config)) => {
                if recorded_version == SIM_VERSION {
                    return Err(ReplayError::Invalid(format!(
                        "current-version replay carries retired bot config for player {seat}"
                    )));
                }
                config.validate().map_err(|reason| {
                    ReplayError::Invalid(format!(
                        "legacy bot config for player {seat} is invalid: {reason}"
                    ))
                })?;
                Some(BotConfig::default())
            }
        };

        Ok(PlayerSpec {
            name: self.name,
            faction: self.faction,
            team: self.team,
            scrap: self.scrap,
            bot: self.bot,
            bot_config,
        })
    }
}

fn default_scrap() -> u32 {
    100
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ReplayBotConfigWire {
    Current(BotConfig),
    Legacy(LegacyBotConfigWire),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyBotConfigWire {
    #[serde(rename = "level")]
    _level: LegacyBotLevel,
    #[serde(default)]
    aggression: Option<u32>,
    #[serde(default)]
    style: Option<LegacyNamedStyle>,
    #[serde(default)]
    variant: Option<u8>,
    #[serde(default, rename = "team_role")]
    _team_role: Option<LegacyTeamRole>,
}

impl LegacyBotConfigWire {
    fn validate(&self) -> Result<(), &'static str> {
        if self.aggression.is_some() && self.style.is_some() {
            return Err("aggression and style are mutually exclusive");
        }
        if self.variant.is_some() && self.style.is_none() {
            return Err("variant requires a named style");
        }
        if self.variant.is_some_and(|variant| variant > 2) {
            return Err("variant must be 0, 1, or 2");
        }
        if self.aggression.is_some_and(|aggression| aggression > 1_000) {
            return Err("aggression must be at most 1000");
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LegacyBotLevel {
    Easy,
    Medium,
    Hard,
    Expert,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyNamedStyle {
    Turtle,
    Balanced,
    Aggressive,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyTeamRole {
    Generalist,
    Vanguard,
    Industry,
    Support,
    Siege,
}

pub(crate) fn deserialize_replay<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<GameReplay, D::Error> {
    let ReplayWire {
        meta,
        setup,
        commands,
        origin,
    } = ReplayWire::deserialize(decoder)?;
    let setup = setup
        .into_current(&meta.sim_version)
        .map_err(serde::de::Error::custom)?;
    Ok(Replay {
        meta,
        setup,
        commands,
        origin,
    })
}

/// Absolute end tick of a replay: its recorded duration, or one past its last
/// command when the metadata omits it.
pub fn replay_duration(replay: &GameReplay) -> u64 {
    replay.meta.ticks.unwrap_or_else(|| {
        replay
            .commands
            .last()
            .map_or(replay.start_tick(), |command| {
                command.tick.saturating_add(1)
            })
    })
}

/// [`replay_duration`], refused beyond [`crate::MAX_REPLAY_TICKS`]. The bound
/// is on the effective duration: with the metadata absent the length falls
/// back to the final command's tick, and a single command stamped at a
/// billion once slipped past a metadata-only guard.
pub fn bounded_replay_duration(replay: &GameReplay) -> anyhow::Result<u64> {
    let total = replay_duration(replay);
    anyhow::ensure!(
        total <= crate::MAX_REPLAY_TICKS,
        "replay spans {total} ticks, beyond the {}-tick bound",
        crate::MAX_REPLAY_TICKS
    );
    Ok(total)
}

/// Feeds a replay's recorded commands back into a state one tick at a time.
pub struct ReplayPlayback<'a> {
    cursor: chassis::replay::ReplayCursor<'a, PlayerCommand>,
}

impl<'a> ReplayPlayback<'a> {
    /// Starts at the replay's first recorded command.
    pub fn new(replay: &'a GameReplay) -> Self {
        Self {
            cursor: replay.cursor(),
        }
    }

    /// Runs the state's current tick with the commands recorded for it.
    pub fn step(&mut self, state: &mut oxide_sim::State) -> oxide_sim::TickReport {
        let commands: Vec<PlayerCommand> = self
            .cursor
            .take_tick(state.current_tick())
            .iter()
            .map(|timed| timed.command.clone())
            .collect();
        state.tick(&commands)
    }

    /// Whether every recorded command has been played. A full-length
    /// playback that leaves commands behind means the replay's duration
    /// metadata is wrong.
    pub fn is_finished(&self) -> bool {
        self.cursor.is_finished()
    }
}

/// Loads an Oxide replay from disk.
///
/// Current-version setup data uses the same strict [`Scenario`] schema as an
/// authored map. A replay from another simulation version may carry one of the
/// retired bot-configuration shapes; those fields are validated and normalized
/// only here so the caller can reach the replay version check for deliberate
/// archaeology. No retired controller is reconstructed.
pub fn load_replay(path: impl AsRef<Path>) -> Result<GameReplay, ReplayError> {
    GameReplay::load_with_decoder(path, |bytes| {
        let ReplayWire {
            meta,
            setup,
            commands,
            origin,
        } = serde_json::from_slice(bytes)?;
        let setup = setup.into_current(&meta.sim_version)?;
        Ok(Replay {
            meta,
            setup,
            commands,
            origin,
        })
    })
}

#[cfg(test)]
mod tests;
