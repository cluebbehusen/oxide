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
        let travel = propulsion + correction;
        let target = if lean && correction != Vec2::ZERO && travel != Vec2::ZERO {
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
mod tests {
    use super::*;

    const SPEED: f32 = 0.11;

    fn slid(ticks: u64, propulsion: Vec2, correction: Vec2, lean: bool) -> SlideMotion {
        let mut slide = SlideMotion::new(0);
        for tick in 1..=ticks {
            slide.observe(tick, 0.0, propulsion, correction, SPEED, lean);
        }
        slide
    }

    #[test]
    fn a_slide_ramps_in_instead_of_snapping() {
        let correction = Vec2::new(0.0, 0.04);
        let first = slid(1, Vec2::ZERO, correction, true);
        let drawn_step = correction - first.lag(1.0);
        assert!(drawn_step.y > 0.0 && drawn_step.y < correction.y * 0.5);
        let steady = slid(40, Vec2::ZERO, correction, true);
        assert!((steady.lag(1.0) - steady.lag(0.0)).length() < 1e-5);
    }

    #[test]
    fn lag_is_bounded_and_releases_to_the_simulation_position() {
        let mut slide = slid(40, Vec2::ZERO, Vec2::new(0.155, 0.0), true);
        assert!(slide.lag(1.0).length() <= MAX_LAG + 1e-6);
        for tick in 41..120 {
            slide.observe(tick, 0.0, Vec2::ZERO, Vec2::ZERO, SPEED, true);
        }
        assert!(slide.settled());
    }

    #[test]
    fn hull_leans_toward_a_diagonal_slide_and_never_past_it() {
        let propulsion = Vec2::new(SPEED, 0.0);
        let correction = Vec2::new(0.0, 0.12);
        let slide = slid(40, propulsion, correction, true);
        let off_axis = (propulsion + correction)
            .y
            .atan2((propulsion + correction).x);
        assert!(slide.yaw(1.0) > 0.1);
        assert!(slide.yaw(1.0) <= MAX_YAW && slide.yaw(1.0) < off_axis);
        let mirrored = slid(40, propulsion, -correction, true);
        assert!((mirrored.yaw(1.0) + slide.yaw(1.0)).abs() < 1e-6);
    }

    #[test]
    fn axial_and_square_sideways_shoves_do_not_lean_a_parked_hull() {
        for correction in [
            Vec2::new(0.05, 0.0),
            Vec2::new(-0.05, 0.0),
            Vec2::new(0.0, 0.05),
        ] {
            assert!(slid(40, Vec2::ZERO, correction, true).yaw(1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn a_rearward_diagonal_shove_leans_along_the_same_axis_as_its_reverse() {
        let ahead = slid(40, Vec2::ZERO, Vec2::new(0.04, 0.04), true);
        let behind = slid(40, Vec2::ZERO, Vec2::new(-0.04, -0.04), true);
        assert!((ahead.yaw(1.0) - behind.yaw(1.0)).abs() < 1e-6);
    }

    #[test]
    fn lean_turns_at_a_bounded_rate_and_is_refused_on_request() {
        let correction = Vec2::new(0.0, 0.12);
        let first = slid(1, Vec2::new(SPEED, 0.0), correction, true);
        assert!(first.yaw(1.0) <= YAW_RATE + 1e-6);
        assert_eq!(first.yaw(0.0), 0.0);
        let held = slid(40, Vec2::new(SPEED, 0.0), correction, false);
        assert_eq!(held.yaw(1.0), 0.0);
        assert!(held.lag(1.0).length() > 0.0);
    }

    #[test]
    fn repeated_tick_does_not_double_count() {
        let mut slide = slid(3, Vec2::ZERO, Vec2::new(0.0, 0.04), true);
        let before = (slide.lag(0.5), slide.yaw(0.5));
        slide.observe(3, 0.0, Vec2::ZERO, Vec2::new(0.0, 0.04), SPEED, true);
        assert_eq!((slide.lag(0.5), slide.yaw(0.5)), before);
    }
}
