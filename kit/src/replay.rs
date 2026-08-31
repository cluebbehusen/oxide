//! Oxide's replay file boundary.

use crate::GameReplay;
use chassis::replay::{Replay, ReplayError, ReplayMeta, TimedCommand};
use oxide_sim::scenario::{BotConfig, BuildingSpec, PlayerSpec, ScenarioMeta, UnitSpec};
use oxide_sim::{Faction, PlayerCommand, SIM_VERSION, Scenario};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayWire {
    meta: ReplayMeta,
    setup: ReplayScenarioWire,
    commands: Vec<TimedCommand<PlayerCommand>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayScenarioWire {
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
        } = serde_json::from_slice(bytes)?;
        let setup = setup.into_current(&meta.sim_version)?;
        Ok(Replay {
            meta,
            setup,
            commands,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct TempReplay(PathBuf);

    impl Drop for TempReplay {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn write_fixture(bytes: &[u8]) -> TempReplay {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "oxide-kit-replay-compat-{}-{id}.json",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("fixture is written");
        TempReplay(path)
    }

    fn replay_with(version: &str, config: Value) -> TempReplay {
        let replay = GameReplay::new(version, Scenario::skirmish());
        let mut document = serde_json::to_value(replay).expect("current replay serializes");
        document["setup"]["players"][1]["bot_config"] = config;
        write_fixture(&serde_json::to_vec(&document).expect("fixture serializes"))
    }

    fn current_document() -> Value {
        serde_json::to_value(GameReplay::new(SIM_VERSION, Scenario::skirmish()))
            .expect("current replay serializes")
    }

    #[test]
    fn current_replay_round_trips_through_the_strict_wire() {
        let document = current_document();
        let fixture = write_fixture(&serde_json::to_vec(&document).expect("fixture serializes"));

        let loaded = load_replay(&fixture.0).expect("current replay loads");
        assert_eq!(loaded.meta.sim_version, SIM_VERSION);
        assert_eq!(loaded.setup, Scenario::skirmish());
        assert!(loaded.commands.is_empty());
    }

    #[test]
    fn foreign_replay_normalizes_known_legacy_bot_metadata() {
        let known = [
            json!({"level": "medium"}),
            json!({
                "level": "expert",
                "style": "aggressive",
                "variant": 2,
                "team_role": "vanguard"
            }),
        ];

        for config in known {
            let fixture = replay_with("0.0.0-legacy", config);
            let replay = load_replay(&fixture.0).expect("known legacy setup loads");
            assert_eq!(
                replay.setup.players[1].bot_config,
                Some(BotConfig::default())
            );
        }
    }

    #[test]
    fn current_replay_rejects_legacy_bot_metadata() {
        let fixture = replay_with(SIM_VERSION, json!({"level": "medium"}));

        assert!(load_replay(&fixture.0).is_err());
    }

    #[test]
    fn foreign_replay_rejects_malformed_legacy_bot_metadata() {
        let malformed = [
            json!({"level": "impossible"}),
            json!({"level": "medium", "mystery": 1}),
            json!({"level": "medium", "aggression": 500, "style": "balanced"}),
            json!({"level": "medium", "variant": 1}),
            json!({"level": "medium", "aggression": 1_001}),
            json!({"level": "medium", "controller": "scripted"}),
        ];

        for config in malformed {
            let fixture = replay_with("0.0.0-legacy", config.clone());
            assert!(
                load_replay(&fixture.0).is_err(),
                "malformed legacy config was accepted: {config}"
            );
        }
    }

    #[test]
    fn foreign_replay_still_accepts_the_current_bot_shape() {
        let fixture = replay_with("0.0.0-legacy", json!({"controller": "scripted"}));
        let replay = load_replay(&fixture.0).expect("current setup shape remains readable");

        assert_eq!(
            replay.setup.players[1].bot_config,
            Some(BotConfig::default())
        );
    }

    #[test]
    fn current_replay_rejects_unknown_fields_at_strict_setup_boundaries() {
        let mut documents = Vec::new();

        let mut replay = current_document();
        replay["unexpected"] = json!(true);
        documents.push(("replay", replay));

        let mut setup = current_document();
        setup["setup"]["unexpected"] = json!(true);
        documents.push(("setup", setup));

        let mut player = current_document();
        player["setup"]["players"][1]["unexpected"] = json!(true);
        documents.push(("player", player));

        let mut config = current_document();
        config["setup"]["players"][1]["bot_config"]["unexpected"] = json!(true);
        documents.push(("bot config", config));

        for (boundary, document) in documents {
            let fixture =
                write_fixture(&serde_json::to_vec(&document).expect("fixture serializes"));
            assert!(
                load_replay(&fixture.0).is_err(),
                "unknown field was accepted at the {boundary} boundary"
            );
        }
    }

    #[test]
    fn current_replay_rejects_duplicate_bot_config_fields() {
        let json = serde_json::to_string(&GameReplay::new(SIM_VERSION, Scenario::skirmish()))
            .expect("current replay serializes");
        let needle = r#""bot_config":{"controller":"scripted"}"#;
        let replacement = concat!(
            r#""bot_config":{"controller":"scripted"},"#,
            r#""bot_config":{"controller":"scripted"}"#
        );
        let duplicate = json.replacen(needle, replacement, 1);
        assert_ne!(duplicate, json, "fixture must duplicate a real field");
        let fixture = write_fixture(duplicate.as_bytes());

        assert!(load_replay(&fixture.0).is_err());
    }
}
