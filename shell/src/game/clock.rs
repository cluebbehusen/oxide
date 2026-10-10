//! A session's clock: whether time runs, how fast, and the tick debt a
//! frame carries. The live match and the replay viewer each own one; the
//! picture only learns where between ticks it is drawn.

use super::{MAX_TICKS_PER_FRAME, TICK_DT};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Clock {
    /// Time stands still; the picture shows the last executed tick.
    pub(crate) paused: bool,
    /// Wall-clock multiplier.
    pub(crate) speed: f64,
    /// Seconds of play owed since the last executed tick.
    pub(crate) accum: f32,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            paused: false,
            speed: 1.0,
            accum: 0.0,
        }
    }
}

impl Clock {
    /// How far the clock sits between the last executed tick and the next,
    /// 0..1.
    pub(crate) fn tick_fraction(&self) -> f32 {
        (self.accum / TICK_DT).clamp(0.0, 1.0)
    }

    /// Interpolation factor for rendering between ticks: a paused picture
    /// shows the last tick whole.
    pub(crate) fn render_alpha(&self) -> f32 {
        if self.paused {
            1.0
        } else {
            self.tick_fraction()
        }
    }

    /// Takes `dt` seconds of wall time at this clock's speed and returns the
    /// whole ticks now due, at most a frame's cap. Debt past the cap is
    /// dropped, as after a hitch.
    pub(crate) fn due_ticks(&mut self, dt: f32) -> u64 {
        self.accum += dt * crate::numeric::to_f32(self.speed);
        let ticks = crate::numeric::to_u64(self.accum / TICK_DT);
        self.accum -= ticks as f32 * TICK_DT;
        ticks.min(u64::from(MAX_TICKS_PER_FRAME))
    }
}

#[cfg(test)]
mod tests;
