use super::*;
use crate::checkpoint::{BotCheckpoint, MAX_BYTES, VERSION};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct BrainV1 {
    player: PlayerId,
    mind: Box<PlayerFacingMind>,
    policy: UtilityPolicy,
    exec: Executive,
    orientation: Option<Orientation>,
}

#[derive(Serialize)]
struct BrainRef<'a> {
    player: PlayerId,
    mind: &'a PlayerFacingMind,
    policy: &'a UtilityPolicy,
    exec: &'a Executive,
    orientation: Option<Orientation>,
}

impl Brain {
    pub(crate) fn checkpoint(&self) -> Result<BotCheckpoint, String> {
        let Self {
            player,
            dials: _,
            mind,
            policy,
            exec,
            orientation,
        } = self;
        let wire = BrainRef {
            player: *player,
            mind,
            policy,
            exec,
            orientation: *orientation,
        };
        let payload = encode(&wire)?;
        if payload.len() > MAX_BYTES {
            return Err("controller checkpoint exceeds byte limit".into());
        }
        Ok(BotCheckpoint {
            version: VERSION,
            payload,
        })
    }

    pub(crate) fn from_checkpoint(
        checkpoint: &BotCheckpoint,
        scenario: &oxide_sim::Scenario,
        state: &oxide_sim::State,
    ) -> Result<Self, String> {
        if checkpoint.version != VERSION {
            return Err("unsupported controller checkpoint version".into());
        }
        if checkpoint.payload.len() > MAX_BYTES {
            return Err("controller checkpoint exceeds byte limit".into());
        }
        let wire: BrainV1 = ciborium::from_reader(checkpoint.payload.as_slice())
            .map_err(|error| error.to_string())?;
        if encode(&wire)? != checkpoint.payload {
            return Err("noncanonical controller checkpoint".into());
        }
        let BrainV1 {
            player,
            mind,
            policy,
            exec,
            orientation,
        } = wire;
        let seat = scenario
            .players
            .get(usize::from(player.0))
            .ok_or("invalid controller seat")?;
        let config = seat
            .bot_config
            .filter(|_| seat.bot)
            .ok_or("checkpoint seat is not a configured bot")?;
        let map = PublicMapBriefing::from_scenario(scenario).map_err(|error| error.to_string())?;
        let expected = Self::scripted(player, config, Arc::new(map.clone()));
        if mind.profile != expected.mind.profile || *mind.public_map != map {
            return Err("controller checkpoint disagrees with scenario".into());
        }
        if state.map().width() != map.map_width() || state.map().height() != map.map_height() {
            return Err("controller checkpoint map mismatch".into());
        }
        if let Some(orientation) = orientation {
            if orientation.checkpoint_dimensions() != (map.map_width(), map.map_height()) {
                return Err("controller checkpoint orientation mismatch".into());
            }
            if mind.oriented_public_map.as_ref() != Some(&orientation.briefing(&map)) {
                return Err("controller checkpoint oriented briefing mismatch".into());
            }
        } else if mind.oriented_public_map.is_some() {
            return Err("oriented briefing without orientation".into());
        }
        if !policy.valid_checkpoint(
            mind.oriented_public_map.as_ref().unwrap_or(&map),
            state.current_tick(),
        ) {
            return Err("invalid controller planning continuation".into());
        }
        if !exec.valid_checkpoint(state.current_tick()) {
            return Err("invalid controller armies".into());
        }
        if !mind
            .intelligence
            .valid_checkpoint(&map, state.current_tick())
        {
            return Err("invalid controller intelligence".into());
        }
        Ok(Self {
            player,
            dials: expected.dials,
            mind,
            policy,
            exec,
            orientation,
        })
    }
}

fn encode(wire: &impl Serialize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    ciborium::into_writer(wire, &mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::seat_bots;

    #[test]
    fn checkpoint_continues_an_active_controller() {
        for config in [
            oxide_sim::scenario::BotConfig::default(),
            oxide_sim::scenario::BotConfig::scripted(
                oxide_sim::scenario::BotDifficulty::Prime,
                oxide_sim::scenario::BotStance::Aggressive,
                93,
            ),
        ] {
            let mut scenario = oxide_sim::Scenario::skirmish();
            scenario.players[1].bot = true;
            scenario.players[1].bot_config = Some(config);
            let mut state = scenario.build().unwrap();
            let mut bots = seat_bots(&scenario).unwrap();
            assert_eq!(bots.len(), 1);
            let mut produced = 0;
            for _ in 0..361 {
                let bot = &mut bots[0];
                let checkpoint = bot.checkpoint().unwrap();
                let mut restored =
                    crate::SeatBot::from_checkpoint(&checkpoint, &scenario, &state).unwrap();
                assert_eq!(bot.dials(), restored.dials());
                let commands = bot.act(&state);
                assert_eq!(commands, restored.act(&state));
                produced += commands.len();
                assert_eq!(
                    bot.checkpoint().unwrap().payload,
                    restored.checkpoint().unwrap().payload
                );
                state.tick(&commands);
            }
            assert!(produced > 1, "continuation must exercise real bot commands");
        }
    }

    #[test]
    fn checkpoint_rejects_wrong_identity_versions_and_corruption() {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.players[1].bot = true;
        scenario.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::default());
        let state = scenario.build().unwrap();
        let checkpoint = seat_bots(&scenario).unwrap()[0].checkpoint().unwrap();
        let mut bad = checkpoint.clone();
        bad.version += 1;
        assert!(Brain::from_checkpoint(&bad, &scenario, &state).is_err());
        bad = checkpoint.clone();
        bad.payload.push(0);
        assert!(Brain::from_checkpoint(&bad, &scenario, &state).is_err());
        bad.payload.clear();
        assert!(Brain::from_checkpoint(&bad, &scenario, &state).is_err());
        let mut wire: BrainV1 = ciborium::from_reader(checkpoint.payload.as_slice()).unwrap();
        wire.player = PlayerId(0);
        bad.payload = encode(&wire).unwrap();
        assert!(Brain::from_checkpoint(&bad, &scenario, &state).is_err());
        scenario.players[1]
            .bot_config
            .as_mut()
            .unwrap()
            .personality_seed += 1;
        assert!(Brain::from_checkpoint(&checkpoint, &scenario, &state).is_err());
    }
}
