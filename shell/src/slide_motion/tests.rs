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
fn forbidding_lean_clears_both_interpolation_endpoints_but_keeps_lag() {
    let mut slide = slid(40, Vec2::new(SPEED, 0.0), Vec2::new(0.0, 0.12), true);
    assert!(slide.current_yaw() > 0.3);
    slide.observe(41, 0.0, Vec2::ZERO, Vec2::ZERO, SPEED, false);
    for alpha in [0.0, 0.5, 1.0] {
        assert_eq!(slide.yaw(alpha), 0.0);
        assert!(slide.lag(alpha).length() > 0.0);
    }
}

#[test]
fn repeated_tick_does_not_double_count() {
    let mut slide = slid(3, Vec2::ZERO, Vec2::new(0.0, 0.04), true);
    let before = (slide.lag(0.5), slide.yaw(0.5));
    slide.observe(3, 0.0, Vec2::ZERO, Vec2::new(0.0, 0.04), SPEED, true);
    assert_eq!((slide.lag(0.5), slide.yaw(0.5)), before);
}
