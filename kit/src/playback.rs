//! Replay playback: a read-only walk through a recorded match.
//!
//! No recorder, no commands, no bots — the log is the match, and this
//! engine replays it through raw [`State::tick`] exactly as the record
//! dictates. Seeking backward restores the nearest forward checkpoint
//! (in-memory `State` clones taken every `CHECKPOINT_EVERY` ticks on
//! the way through) and re-simulates the suffix. A seeked position must
//! agree with a straight run from the recording origin, and the
//! test below holds that as a hash identity.

use crate::GameReplay;
use anyhow::Result;
use oxide_sim::{SIM_VERSION, State};

/// Minimum checkpoint cadence in ticks; the real cadence stretches so
/// no record ever holds more than [`MAX_CHECKPOINTS`] clones — a
/// 2M-tick replay of a 256x256 world must not exhaust memory for
/// seek convenience.
const CHECKPOINT_EVERY: u64 = 1024;

/// Upper bound on retained state clones.
const MAX_CHECKPOINTS: u64 = 64;

/// A loaded replay with a current position.
pub struct Playback {
    replay: GameReplay,
    /// The world at the current position.
    pub state: State,
    /// Motion emitted by the most recently simulated tick.
    pub last_motion: Vec<oxide_sim::GroundMotion>,
    /// Index into `replay.commands` of the first command not yet fed.
    next_cmd: usize,
    /// Forward checkpoints, ascending by tick.
    checkpoints: Vec<(u64, State)>,
    /// Ticks between retained checkpoints for this record.
    cadence: u64,
    total: u64,
}

/// Ticks between retained checkpoints for a record of `total` ticks:
/// never denser than [`CHECKPOINT_EVERY`], never more than
/// [`MAX_CHECKPOINTS`] clones, power-of-two for stable stamping.
fn checkpoint_cadence(total: u64) -> u64 {
    CHECKPOINT_EVERY
        .max(total.div_ceil(MAX_CHECKPOINTS))
        .next_power_of_two()
}

impl Playback {
    /// Validates and opens a replay at its origin. Cross-version records are
    /// refused — replays reproduce only on the sim that wrote them.
    pub fn load(replay: GameReplay) -> Result<Self> {
        // Seeking is synchronous: a structurally valid file claiming an
        // absurd length would hang the viewer at the first End press.
        const MAX_INTERACTIVE_TICKS: u64 = 2_000_000;
        replay
            .validate(Some(SIM_VERSION))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let state = crate::recording::initial_state(&replay)?;
        let total = crate::replay_duration(&replay);
        anyhow::ensure!(
            total <= MAX_INTERACTIVE_TICKS,
            "replay spans {total} ticks, beyond the {MAX_INTERACTIVE_TICKS}-tick interactive limit"
        );
        let cadence = checkpoint_cadence(total - replay.start_tick());
        Ok(Self {
            replay,
            state,
            last_motion: Vec::new(),
            next_cmd: 0,
            checkpoints: vec![],
            cadence,
            total,
        })
    }

    /// First absolute tick available in the recording.
    pub fn start(&self) -> u64 {
        self.replay.start_tick()
    }

    /// The absolute end tick.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The current position.
    pub fn position(&self) -> u64 {
        self.state.current_tick()
    }

    /// Whether the record is exhausted.
    pub fn at_end(&self) -> bool {
        self.position() >= self.total
    }

    /// Advances up to `ticks`, stopping at the end of the record, and
    /// returns every event the replayed world emitted on the way — the
    /// viewer's presentation feed.
    pub fn advance(&mut self, ticks: u64) -> Vec<oxide_sim::Event> {
        let mut events = Vec::new();
        for _ in 0..ticks {
            if self.at_end() {
                break;
            }
            events.extend(self.step());
        }
        events
    }

    /// Jumps to `target` (clamped to the record). Backward: restore the
    /// nearest checkpoint at or before the target and re-simulate the
    /// suffix — bit-identical to having played straight there.
    pub fn seek(&mut self, target: u64) {
        self.seek_step(target, u64::MAX);
    }

    /// One budgeted slice of a seek toward `target`: restores the best
    /// checkpoint exactly like [`Playback::seek`], then simulates at
    /// most `budget` ticks. Returns true when the target is reached —
    /// callers loop across frames, so a long first seek costs a
    /// progress bar instead of a frozen render thread.
    pub fn seek_step(&mut self, target: u64, budget: u64) -> bool {
        let target = target.clamp(self.start(), self.total);
        // Inspect checkpoints by reference: most forward slices need no restore.
        let best = self.checkpoints.iter().rev().find(|(t, _)| *t <= target);
        let restore = match &best {
            _ if target < self.position() => true,
            Some((t, _)) => *t > self.position(),
            None => false,
        };
        if restore {
            self.state = best.map_or_else(
                || crate::recording::initial_state(&self.replay).expect("validated at load"),
                |(_, state)| state.clone(),
            );
            self.last_motion.clear();
            self.next_cmd = self
                .replay
                .commands
                .partition_point(|c| c.tick < self.state.current_tick());
        }
        let mut ran = 0;
        while self.position() < target && ran < budget {
            self.step();
            ran += 1;
        }
        self.position() >= target
    }

    fn step(&mut self) -> Vec<oxide_sim::Event> {
        let tick = self.state.current_tick();
        // Re-walked spans skip stamping (a later checkpoint already
        // exists); fresh ground appends, keeping the vec sorted.
        if tick.is_multiple_of(self.cadence)
            && self.checkpoints.last().is_none_or(|(t, _)| tick > *t)
        {
            self.checkpoints.push((tick, self.state.clone()));
        }
        let mut commands = Vec::new();
        while let Some(c) = self.replay.commands.get(self.next_cmd) {
            if c.tick != tick {
                break;
            }
            commands.push(c.command.clone());
            self.next_cmd += 1;
        }
        // Raw tick, never a recorder: playback must not re-record.
        let report = self.state.tick(&commands);
        self.last_motion = report.movement;
        report.events
    }
}

#[cfg(test)]
mod tests;
