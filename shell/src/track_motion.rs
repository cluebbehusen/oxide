//! Presentation odometry for independently driven track belts.

use macroquad::prelude::Vec2;

#[derive(Debug)]
pub(crate) struct TrackMotion {
    tick: u64,
    heading: f32,
    previous: [f32; 2],
    distance: [f32; 2],
}

impl TrackMotion {
    pub(crate) fn new(tick: u64, heading: f32) -> Self {
        Self {
            tick,
            heading,
            previous: [0.0; 2],
            distance: [0.0; 2],
        }
    }

    pub(crate) fn observe(&mut self, tick: u64, heading: f32, gauge: f32, propulsion: Vec2) {
        if self.tick == tick {
            return;
        }
        let turn = (heading - self.heading + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        let mid = self.heading + turn * 0.5;
        let forward = propulsion.dot(Vec2::new(mid.cos(), mid.sin()));
        self.previous = self.distance;
        self.distance[0] += forward + turn * gauge * 0.5;
        self.distance[1] += forward - turn * gauge * 0.5;
        self.tick = tick;
        self.heading = heading;
    }

    pub(crate) fn distances(&self, alpha: f32) -> [f32; 2] {
        std::array::from_fn(|side| {
            self.previous[side]
                + (self.distance[side] - self.previous[side]) * alpha.clamp(0.0, 1.0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn belts_stop_when_propulsion_stops() {
        let mut tracks = TrackMotion::new(0, 0.0);
        tracks.observe(1, 0.0, 0.8, Vec2::new(0.04, 0.0));
        assert_eq!(tracks.distances(1.0), [0.04, 0.04]);
        tracks.observe(2, 0.0, 0.8, Vec2::ZERO);
        assert_eq!(tracks.distances(1.0), [0.04, 0.04]);
        tracks.observe(3, 0.0, 0.8, Vec2::ZERO);
        assert_eq!(tracks.distances(0.5), [0.04, 0.04]);
    }

    #[test]
    fn pivot_counter_rotates_and_reverse_reverses_both_belts() {
        let mut tracks = TrackMotion::new(0, 0.0);
        tracks.observe(1, 0.1, 0.8, Vec2::ZERO);
        assert!(tracks.distance[0] > 0.0 && tracks.distance[1] < 0.0);
        let mut reverse = TrackMotion::new(0, 0.0);
        reverse.observe(1, 0.0, 0.8, Vec2::new(-0.02, 0.0));
        assert_eq!(reverse.distances(1.0), [-0.02, -0.02]);
    }

    #[test]
    fn repeated_tick_does_not_reset_interpolation_or_double_count_motion() {
        let mut tracks = TrackMotion::new(0, 0.0);
        tracks.observe(1, 0.0, 0.8, Vec2::new(0.04, 0.0));
        let before = tracks.distances(0.5);
        tracks.observe(1, 0.0, 0.8, Vec2::new(0.04, 0.0));
        assert_eq!(tracks.distances(0.5), before);
    }
}
