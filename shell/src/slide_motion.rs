//! Presentation easing for collision slides.
//!
//! The simulation separates overlapping bodies with an instantaneous
//! displacement that ignores the hull's heading. Drawn literally, a body
//! snaps sideways at up to several times its drive speed while its hull keeps
//! pointing along the route. This module low-passes that displacement into a
//! bounded draw lag and leans the drawn hull toward the axis it is actually
//! travelling along. Simulation position and heading are never changed.

use macroquad::prelude::Vec2;

/// Share of the draw lag retained each tick; the rest is released toward the
/// simulation position, so a slide ramps in and out over a few ticks.
const LAG_RETAIN: f32 = 0.7;

/// Furthest the drawn body may trail its simulation position, in tiles.
/// Selection and targeting hit-test the simulation position, so this stays
/// well inside the smallest body radius.
pub(crate) const MAX_LAG: f32 = 0.15;

/// Largest drawn hull lean away from the simulation heading, in radians.
pub(crate) const MAX_YAW: f32 = 0.35;

/// Largest change of the lean per tick, in radians.
const YAW_RATE: f32 = 0.06;

/// Slide length, as a share of the body's top speed, at which the lean
/// reaches its full strength. Contact jitter below this barely leans.
const FULL_LEAN_SLIDE: f32 = 0.5;

const SETTLED: f32 = 1e-4;

#[derive(Debug)]
pub(crate) struct SlideMotion {
    tick: u64,
    lag: [Vec2; 2],
    yaw: [f32; 2],
}

impl SlideMotion {
    pub(crate) fn new(tick: u64) -> Self {
        Self {
            tick,
            lag: [Vec2::ZERO; 2],
            yaw: [0.0; 2],
        }
    }

    /// Folds one tick of motion in. `heading` is the simulation hull heading
    /// in radians with the same zero as the displacement vectors. `lean`
    /// is false for a body whose drawn heading must stay truthful, such as a
    /// fixed weapon holding an aim.
    pub(crate) fn observe(
        &mut self,
        tick: u64,
        heading: f32,
        propulsion: Vec2,
        correction: Vec2,
        top_speed: f32,
        lean: bool,
    ) {
        if self.tick == tick {
            return;
        }
        self.tick = tick;
        self.lag = [
            self.lag[1],
            ((self.lag[1] + correction) * LAG_RETAIN).clamp_length_max(MAX_LAG),
        ];
        if !lean {
            self.yaw = [0.0; 2];
            return;
        }
        let travel = propulsion + correction;
        let target = if correction != Vec2::ZERO && travel != Vec2::ZERO {
            let off_axis = travel.y.atan2(travel.x) - heading;
            // Period pi: a shove from ahead and one from behind roll the body
            // along the same axis, and a square sideways skid turns nothing.
            let strength = (correction.length() / (top_speed * FULL_LEAN_SLIDE)).min(1.0);
            MAX_YAW * (2.0 * off_axis).sin() * strength
        } else {
            0.0
        };
        self.yaw = [
            self.yaw[1],
            self.yaw[1] + (target - self.yaw[1]).clamp(-YAW_RATE, YAW_RATE),
        ];
    }

    /// How far behind the simulation position the body is drawn.
    pub(crate) fn lag(&self, alpha: f32) -> Vec2 {
        self.lag[0].lerp(self.lag[1], alpha.clamp(0.0, 1.0))
    }

    /// Drawn hull lean relative to the simulation heading.
    pub(crate) fn yaw(&self, alpha: f32) -> f32 {
        self.yaw[0] + (self.yaw[1] - self.yaw[0]) * alpha.clamp(0.0, 1.0)
    }

    /// Latest lean, for consumers that accumulate per tick.
    pub(crate) fn current_yaw(&self) -> f32 {
        self.yaw[1]
    }

    /// Whether the body is drawn exactly where and how the simulation says.
    pub(crate) fn settled(&self) -> bool {
        self.lag
            .iter()
            .all(|lag| lag.length_squared() < SETTLED * SETTLED)
            && self.yaw.iter().all(|yaw| yaw.abs() < SETTLED)
    }
}

#[cfg(test)]
mod tests;
