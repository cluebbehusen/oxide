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
fn a_shove_rolls_the_belts_only_along_the_hull() {
    let mut tracks = TrackMotion::new(0, 0.0);
    tracks.observe(1, 0.0, 0.8, Vec2::new(0.0, 0.05));
    assert_eq!(tracks.distances(1.0), [0.0, 0.0]);
    tracks.observe(2, 0.0, 0.8, Vec2::new(-0.03, 0.05));
    assert_eq!(tracks.distances(1.0), [-0.03, -0.03]);
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
