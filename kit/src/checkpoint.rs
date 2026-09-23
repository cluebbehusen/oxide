//! Same-version session continuation without executing historical ticks.
//!
//! This is an internal checkpoint contract, independent of player save files and
//! replay origins. Controller memory is part of continuation, not world state.

use crate::{GameReplay, stats::LiveMatchStats};
use anyhow::{Context, Result, ensure};
use oxide_bot::{SeatBot, checkpoint::BotCheckpoint};
use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario, State};
use serde::{Deserialize, Serialize};

/// Session envelope revision, separate from simulation and controller revisions.
pub const VERSION: u32 = 2;
/// Encoded checkpoint load bound, including the optional legacy command history.
pub const MAX_BYTES: usize = 256 * 1024 * 1024;

/// World and command sources at the boundary before the next tick executes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCheckpoint {
    version: u32,
    sim_version: String,
    scenario: Scenario,
    state: State,
    snapshot_binding: u64,
    bots: Vec<BotCheckpoint>,
    pending: Vec<PlayerCommand>,
    stats: Option<LiveMatchStats>,
}

/// Validated parts ready for a host to install together.
pub struct RestoredSession {
    /// Authored match setup.
    pub scenario: Scenario,
    /// Exact world at the saved boundary.
    pub state: State,
    /// Controllers in canonical seat order.
    pub bots: Vec<SeatBot>,
    /// Inputs that have not executed or entered the command record yet.
    pub pending: Vec<PlayerCommand>,
    /// Incremental statistics, when the host tracks them.
    pub stats: Option<LiveMatchStats>,
}

impl SessionCheckpoint {
    /// Starts a world-only recording at this session boundary.
    pub fn recording(&self) -> Result<GameReplay> {
        self.validate_world()?;
        Ok(GameReplay::with_origin(
            SIM_VERSION,
            self.scenario.clone(),
            crate::recording::WorldOrigin::capture(&self.scenario, &self.state)?,
        )?)
    }

    pub(crate) fn validate_origin(&self, replay: &GameReplay) -> Result<()> {
        replay.validate(Some(SIM_VERSION))?;
        self.clone().restore()?;
        ensure!(
            replay.origin.is_some()
                && replay.setup == self.scenario
                && replay.start_tick() == self.state.current_tick()
                && crate::recording::initial_state(replay)?.hash() == self.state.hash(),
            "recovery checkpoint does not match recording origin"
        );
        if crate::replay_duration(replay) > replay.start_tick() {
            let mut first = replay
                .commands
                .iter()
                .take_while(|timed| timed.tick == replay.start_tick());
            ensure!(
                self.pending
                    .iter()
                    .all(|pending| first.next().is_some_and(|timed| timed.command == *pending)),
                "recovery batch omits pending checkpoint commands"
            );
        }
        Ok(())
    }

    pub(crate) fn validate_first_batch(&self, commands: &[PlayerCommand]) -> Result<()> {
        ensure!(
            commands.starts_with(&self.pending),
            "recovery batch omits pending checkpoint commands"
        );
        Ok(())
    }

    /// Restores controllers, then observes only the recorded suffix. Pending
    /// inputs survive an empty suffix; a completed first tick consumes them once.
    pub fn resume_recording(self, replay: &GameReplay) -> Result<RestoredSession> {
        self.validate_origin(replay)?;
        let end = crate::bounded_replay_duration(replay)?;
        let mut session = self.restore()?;
        let mut cursor = replay.cursor();
        for tick in session.state.current_tick()..end {
            let commands: Vec<_> = cursor
                .take_tick(tick)
                .iter()
                .map(|timed| timed.command.clone())
                .collect();
            if tick == replay.start_tick() {
                ensure!(
                    commands.starts_with(&session.pending),
                    "recovery batch omits pending checkpoint commands"
                );
                session.pending.clear();
            }
            let _ = crate::bot_execution::commands(&session.state, &mut session.bots);
            let report = session.state.tick(&commands);
            if let Some(stats) = &mut session.stats {
                stats.observe(&session.state, &report.events);
            }
        }
        ensure!(
            cursor.is_finished(),
            "recovery suffix left unconsumed commands"
        );
        Ok(session)
    }

    /// Borrows a quiescent host after its tick and statistics update have finished.
    /// Does not run controllers, drain inputs, or advance the simulation.
    /// The host must supply the scenario that produced this world. The binding
    /// detects later mismatches; it does not prove the world's historical origin.
    pub fn capture(
        scenario: &Scenario,
        state: &State,
        bots: &[SeatBot],
        pending: &[PlayerCommand],
        stats: Option<&LiveMatchStats>,
    ) -> Result<Self> {
        let checkpoint = Self {
            version: VERSION,
            sim_version: SIM_VERSION.into(),
            scenario: scenario.clone(),
            state: state.clone(),
            snapshot_binding: snapshot_binding(scenario, state),
            bots: bots
                .iter()
                .map(SeatBot::checkpoint)
                .collect::<Result<_, _>>()
                .map_err(anyhow::Error::msg)?,
            pending: pending.to_vec(),
            stats: stats.cloned(),
        };
        checkpoint.validate_world()?;
        ensure!(
            bots.iter().map(SeatBot::player).collect::<Vec<_>>() == checkpoint.expected_seats(),
            "controller roster mismatch"
        );
        Ok(checkpoint)
    }

    fn expected_seats(&self) -> Vec<oxide_sim::PlayerId> {
        self.scenario
            .players
            .iter()
            .enumerate()
            .filter(|(_, seat)| seat.bot && seat.bot_config.is_some())
            .map(|(seat, _)| oxide_sim::PlayerId(seat as u8))
            .collect()
    }

    fn validate_world(&self) -> Result<()> {
        ensure!(
            self.version == VERSION,
            "unsupported session checkpoint version {}",
            self.version
        );
        ensure!(
            self.sim_version == SIM_VERSION,
            "checkpoint simulation version mismatch"
        );
        self.state
            .validate_invariants()
            .context("checkpoint world")?;
        ensure!(
            self.snapshot_binding == snapshot_binding(&self.scenario, &self.state),
            "checkpoint scenario/world binding mismatch"
        );
        validate_setup(&self.scenario, &self.state)?;
        ensure!(
            self.pending.len() <= 65_536
                && self
                    .pending
                    .iter()
                    .all(|command| usize::from(command.player.0) < self.state.players().len()),
            "invalid pending command roster or count"
        );
        if let Some(stats) = &self.stats {
            stats.validate_checkpoint(&self.state)?;
        }
        Ok(())
    }

    /// Validates all parts before handing a usable session to the host.
    pub fn restore(self) -> Result<RestoredSession> {
        self.validate_world()?;
        ensure!(
            self.bots.len() == self.expected_seats().len(),
            "controller roster mismatch"
        );
        let bots = self
            .bots
            .iter()
            .map(|checkpoint| SeatBot::from_checkpoint(checkpoint, &self.scenario, &self.state))
            .collect::<Result<Vec<_>, _>>()
            .map_err(anyhow::Error::msg)?;
        ensure!(
            bots.iter().map(SeatBot::player).collect::<Vec<_>>() == self.expected_seats(),
            "controller seat order mismatch"
        );
        Ok(RestoredSession {
            scenario: self.scenario,
            state: self.state,
            bots,
            pending: self.pending,
            stats: self.stats,
        })
    }
}

pub(crate) fn validate_setup(scenario: &Scenario, state: &State) -> Result<()> {
    let initial = scenario.build().context("checkpoint scenario")?;
    ensure!(
        initial.mode() == state.mode(),
        "checkpoint scenario mode mismatch"
    );
    ensure!(
        initial.map().width() == state.map().width()
            && initial.map().height() == state.map().height(),
        "checkpoint map dimensions mismatch"
    );
    ensure!(
        initial.players().len() == state.players().len()
            && initial
                .players()
                .iter()
                .zip(state.players())
                .all(|(a, b)| a.faction == b.faction && a.team == b.team),
        "checkpoint seats mismatch"
    );
    Ok(())
}

pub(crate) fn snapshot_binding(scenario: &Scenario, state: &State) -> u64 {
    chassis::hash::state_hash(&(scenario, state.hash()))
}

/// A checkpoint paired with its current recording. The recording may start
/// from a scenario or a world origin; the checkpoint needs no earlier commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedCheckpoint {
    session: SessionCheckpoint,
    recorder: GameReplay,
}

impl RecordedCheckpoint {
    /// Retains the legacy recorder without replaying it during restoration.
    pub fn capture(session: SessionCheckpoint, recorder: &GameReplay) -> Result<Self> {
        let mut recorder = recorder.clone();
        recorder.meta.ticks = Some(session.state.current_tick());
        let checkpoint = Self { session, recorder };
        checkpoint.validate_record()?;
        Ok(checkpoint)
    }

    fn validate_record(&self) -> Result<()> {
        self.recorder
            .validate(Some(SIM_VERSION))
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ensure!(
            self.recorder.setup == self.session.scenario,
            "checkpoint recorder scenario mismatch"
        );
        ensure!(
            self.recorder.meta.ticks == Some(self.session.state.current_tick()),
            "checkpoint recorder tick mismatch"
        );
        Ok(())
    }

    /// Checks the recorder envelope and restores the session without ticking.
    pub fn restore(self) -> Result<(RestoredSession, GameReplay)> {
        self.validate_record()?;
        Ok((self.session.restore()?, self.recorder))
    }

    /// Loads bounded bytes. This does not load an ordinary player save or replay.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_BYTES, "checkpoint exceeds byte limit");
        serde_json::from_slice(bytes).context("decoding recorded checkpoint")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::{Command, PlayerId};

    fn checkpoint() -> RecordedCheckpoint {
        let mut scenario = Scenario::skirmish();
        for seat in &mut scenario.players {
            seat.bot = false;
            seat.bot_config = None;
        }
        let mut state = scenario.build().unwrap();
        let mut stats = LiveMatchStats::new(&state);
        for _ in 0..105 {
            let report = state.tick(&[]);
            stats.observe(&state, &report.events);
        }
        let pending = vec![PlayerCommand {
            player: PlayerId(0),
            command: Command::Stop { units: vec![] },
        }];
        let core =
            SessionCheckpoint::capture(&scenario, &state, &[], &pending, Some(&stats)).unwrap();
        RecordedCheckpoint::capture(core, &GameReplay::new(SIM_VERSION, scenario)).unwrap()
    }

    #[test]
    fn checkpoint_round_trip_is_observational_and_keeps_statistics_stride() {
        let original = checkpoint();
        let bytes = serde_json::to_vec(&original).unwrap();
        let decoded = RecordedCheckpoint::from_bytes(&bytes).unwrap();
        let (mut a, _) = original.restore().unwrap();
        let (mut b, _) = decoded.restore().unwrap();
        assert_eq!(a.state.current_tick(), 105);
        assert_eq!(a.pending, b.pending);
        assert_eq!(a.state.hash(), b.state.hash());
        for _ in 0..120 {
            let report = a.state.tick(&std::mem::take(&mut a.pending));
            let other = b.state.tick(&std::mem::take(&mut b.pending));
            assert_eq!(report.events, other.events);
            a.stats.as_mut().unwrap().observe(&a.state, &report.events);
            b.stats.as_mut().unwrap().observe(&b.state, &other.events);
            assert_eq!(
                a.stats.as_ref().unwrap().snapshot(&a.state),
                b.stats.as_ref().unwrap().snapshot(&b.state)
            );
        }
    }

    #[test]
    fn checkpoint_rejects_a_world_with_different_scenario_rules() {
        let mut scenario = Scenario::skirmish();
        for player in &mut scenario.players {
            player.bot = false;
            player.bot_config = None;
        }
        let state = scenario.build().unwrap();
        scenario.mode = oxide_sim::scenario::ScenarioMode::Sandbox;
        let error = SessionCheckpoint::capture(&scenario, &state, &[], &[], None).unwrap_err();
        assert!(error.to_string().contains("scenario mode mismatch"));
        let state = scenario.build().unwrap();
        let restored = SessionCheckpoint::capture(&scenario, &state, &[], &[], None)
            .unwrap()
            .restore()
            .unwrap();
        assert_eq!(
            restored.state.mode(),
            oxide_sim::scenario::ScenarioMode::Sandbox
        );
    }

    #[test]
    fn checkpoint_rejects_incompatible_or_inconsistent_parts() {
        let original = checkpoint();
        for version in [1, VERSION + 1] {
            let mut bad = original.clone();
            bad.session.version = version;
            assert!(bad.restore().is_err());
        }
        let mut bad = original.clone();
        bad.session.sim_version = "other".into();
        assert!(bad.restore().is_err());
        let mut bad = original.clone();
        bad.session.pending[0].player = PlayerId(255);
        assert!(bad.restore().is_err());
        let mut bad = original.clone();
        bad.recorder.meta.ticks = Some(104);
        assert!(bad.restore().is_err());
        let mut bad = original.clone();
        bad.recorder.setup.name.push('!');
        assert!(bad.restore().is_err());
        let mut bad = original.clone();
        bad.session.scenario.players[0].bot = true;
        bad.session.scenario.players[0].bot_config = Some(Default::default());
        assert!(bad.session.restore().is_err());
        let json = serde_json::to_value(original).unwrap();
        for (field, value) in [
            ("every", serde_json::json!(0)),
            ("every", serde_json::json!(3)),
            ("stats", serde_json::json!({})),
        ] {
            let mut bad = json.clone();
            bad["session"]["stats"][field] = value;
            let result = serde_json::from_value::<RecordedCheckpoint>(bad)
                .map_err(anyhow::Error::from)
                .and_then(RecordedCheckpoint::restore);
            assert!(result.is_err());
        }
        assert!(RecordedCheckpoint::from_bytes(b"{}").is_err());
    }

    #[test]
    fn checkpoint_rejects_changes_to_the_captured_setup_and_world_pair() {
        let original = checkpoint();
        for change_world in [false, true] {
            let mut bad = original.clone();
            if change_world {
                bad.session.state.tick(&[]);
                bad.recorder.meta.ticks = Some(bad.session.state.current_tick());
            } else {
                bad.session.scenario.seed += 1;
                bad.recorder.setup = bad.session.scenario.clone();
            }
            let bytes = serde_json::to_vec(&bad).unwrap();
            let error = RecordedCheckpoint::from_bytes(&bytes)
                .unwrap()
                .restore()
                .err()
                .unwrap();
            assert!(
                error
                    .to_string()
                    .contains("scenario/world binding mismatch")
            );
        }
        let mut bad = original.session;
        bad.scenario.seed += 1;
        let error = bad.restore().err().unwrap();
        assert!(
            error
                .to_string()
                .contains("scenario/world binding mismatch")
        );
    }
}
