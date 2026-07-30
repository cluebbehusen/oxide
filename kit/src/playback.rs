//! Replay playback: a read-only walk through a recorded match.
//!
//! No recorder, no commands, no bots — the log is the match, and this
//! engine replays it through raw [`State::tick`] exactly as the record
//! dictates. Seeking backward restores the nearest forward checkpoint
//! (in-memory `State` clones taken every `CHECKPOINT_EVERY` ticks on
//! the way through) and re-simulates the suffix; save-is-a-replay means
//! a seeked position can never diverge from a straight run, and the
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
    /// Validates and opens a replay at tick 0. Cross-version records are
    /// refused — replays reproduce only on the sim that wrote them.
    pub fn load(replay: GameReplay) -> Result<Self> {
        replay
            .validate(Some(SIM_VERSION))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let state = replay.setup.build()?;
        let total = replay.meta.ticks.unwrap_or_else(|| {
            replay
                .commands
                .last()
                .map_or(0, |c| c.tick.saturating_add(1))
        });
        // Seeking is synchronous: a structurally valid file claiming an
        // absurd length would hang the viewer at the first End press.
        const MAX_INTERACTIVE_TICKS: u64 = 2_000_000;
        anyhow::ensure!(
            total <= MAX_INTERACTIVE_TICKS,
            "replay spans {total} ticks — beyond the {MAX_INTERACTIVE_TICKS}-tick interactive limit"
        );
        let cadence = checkpoint_cadence(total);
        Ok(Self {
            replay,
            state,
            next_cmd: 0,
            checkpoints: vec![],
            cadence,
            total,
        })
    }

    /// The recorded length in ticks.
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
        let target = target.min(self.total);
        // The best launch point is the richest checkpoint at or before
        // the target — used for backward seeks AND long forward jumps,
        // and never discarded: a deterministic stream keeps every
        // recorded checkpoint valid, so End → Home → End replays a
        // suffix, not the whole record.
        let best = self
            .checkpoints
            .iter()
            .rev()
            .find(|(t, _)| *t <= target)
            .cloned();
        let restore = match &best {
            _ if target < self.position() => true,
            Some((t, _)) => *t > self.position(),
            None => false,
        };
        if restore {
            let (_, state) =
                best.unwrap_or_else(|| (0, self.replay.setup.build().expect("validated at load")));
            self.state = state;
            self.next_cmd = self
                .replay
                .commands
                .partition_point(|c| c.tick < self.state.current_tick());
        }
        while self.position() < target {
            self.step();
        }
    }

    /// One budgeted slice of a seek toward `target`: restores the best
    /// checkpoint exactly like [`Playback::seek`], then simulates at
    /// most `budget` ticks. Returns true when the target is reached —
    /// callers loop across frames, so a long first seek costs a
    /// progress bar instead of a frozen render thread.
    pub fn seek_step(&mut self, target: u64, budget: u64) -> bool {
        let target = target.min(self.total);
        // Restoring is cheap and idempotent; re-deciding it every slice
        // keeps this a plain resumable loop with no extra state.
        let best = self
            .checkpoints
            .iter()
            .rev()
            .find(|(t, _)| *t <= target)
            .cloned();
        let restore = match &best {
            _ if target < self.position() => true,
            Some((t, _)) => *t > self.position(),
            None => false,
        };
        if restore {
            let (_, state) =
                best.unwrap_or_else(|| (0, self.replay.setup.build().expect("validated at load")));
            self.state = state;
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
        self.state.tick(&commands).events
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_budgeted_seek_lands_bit_identical_to_a_straight_one() {
        // The slices must be invisible: however many frames a seek is
        // spread across, the state it lands on is the state one big
        // seek produces.
        let replay = recorded_match();
        let mut straight = Playback::load(replay.clone()).unwrap();
        straight.seek(333);
        let expected = straight.state.hash();

        let mut sliced = Playback::load(replay).unwrap();
        let mut slices = 0;
        while !sliced.seek_step(333, 50) {
            slices += 1;
            assert!(slices < 100, "the budget must make progress");
        }
        assert_eq!(sliced.position(), 333);
        assert_eq!(sliced.state.hash(), expected, "slices changed the state");
        assert!(slices >= 5, "a 50-tick budget takes several frames");
    }

    use super::*;
    use crate::runner;
    use oxide_sim::Scenario;

    fn recorded_match() -> GameReplay {
        let mut scenario = Scenario::skirmish();
        for p in scenario.players.iter_mut() {
            p.bot = true;
        }
        runner::run_scenario(&scenario, 900, true, true)
            .unwrap()
            .replay
            .unwrap()
    }

    #[test]
    fn a_seeked_position_matches_the_straight_run() {
        let replay = recorded_match();
        let mut straight = Playback::load(replay.clone()).unwrap();
        straight.advance(700);
        let truth = straight.state.hash();

        let mut seeker = Playback::load(replay).unwrap();
        seeker.advance(900);
        assert!(seeker.at_end());
        seeker.seek(700); // backward across two checkpoints
        assert_eq!(seeker.position(), 700);
        assert_eq!(
            seeker.state.hash(),
            truth,
            "a seek is a re-simulation, not an approximation"
        );
        seeker.seek(123); // backward into the first checkpoint span
        seeker.seek(700); // and forward again
        assert_eq!(seeker.state.hash(), truth);
    }

    #[test]
    fn checkpoint_memory_is_bounded_at_every_length() {
        for total in [0, 900, 40_000, 500_000, 2_000_000] {
            let cadence = checkpoint_cadence(total);
            assert!(cadence >= CHECKPOINT_EVERY);
            assert!(cadence.is_power_of_two());
            assert!(
                total / cadence <= MAX_CHECKPOINTS,
                "{total} ticks at cadence {cadence} keeps too many clones"
            );
        }
    }

    #[test]
    fn an_absurd_claimed_length_is_refused_interactively() {
        let mut replay = recorded_match();
        replay.meta.ticks = Some(1_000_000_000);
        assert!(
            Playback::load(replay).is_err(),
            "a billion-tick claim must not hang the first End press"
        );
    }

    #[test]
    fn end_home_end_replays_a_suffix_not_the_record() {
        let replay = recorded_match();
        let mut pb = Playback::load(replay).unwrap();
        pb.advance(900);
        let full = pb.state.hash();
        pb.seek(0);
        assert_eq!(pb.position(), 0);
        pb.seek(900);
        assert_eq!(
            pb.state.hash(),
            full,
            "the round trip lands on the same bytes"
        );
        assert!(
            !pb.checkpoints.is_empty(),
            "forward checkpoints survive backward seeks"
        );
    }

    #[test]
    fn playback_never_outruns_the_record() {
        let replay = recorded_match();
        let mut pb = Playback::load(replay).unwrap();
        pb.advance(5_000);
        assert_eq!(pb.position(), 900, "the record ends where it ends");
        pb.seek(2_000);
        assert_eq!(pb.position(), 900);
    }
}
