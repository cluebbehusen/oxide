use super::*;

fn fx(n: i64) -> Fx {
    Fx::from_num(n)
}

#[test]
fn sqrt_of_perfect_squares_is_exact() {
    for n in [0i64, 1, 4, 9, 16, 144, 10_000] {
        assert_eq!(sqrt(fx(n)), fx(n.isqrt()));
    }
}

#[test]
fn sqrt_of_two_matches_reference_bits() {
    // sqrt(2) = 1.41421356... — floor in Q32.32 is 0x1_6A09E667.
    let two = fx(2);
    assert_eq!(sqrt(two).to_bits(), 0x1_6A09_E667);
}

#[test]
fn sqrt_is_monotonic_near_boundaries() {
    let below = Fx::from_bits(fx(4).to_bits() - 1);
    assert!(sqrt(below) < fx(2));
    assert_eq!(sqrt(fx(4)), fx(2));
}

#[test]
#[should_panic(expected = "sqrt of negative")]
fn sqrt_of_negative_panics() {
    sqrt(fx(-1));
}

#[test]
fn length_of_pythagorean_triple_is_exact() {
    let v = Vec2Fx::new(fx(3), fx(4));
    assert_eq!(v.length(), fx(5));
    assert_eq!(v.length_sq(), fx(25));
}

#[test]
fn move_toward_arrives_exactly_without_overshoot() {
    let from = Vec2Fx::ZERO;
    let to = Vec2Fx::new(fx(3), fx(4));
    // Distance is 5; four steps of 1.5 covers 6, so we must land exactly.
    let step = Fx::lit("1.5");
    let mut pos = from;
    for _ in 0..4 {
        pos = pos.move_toward(to, step);
    }
    assert_eq!(pos, to);
}

#[test]
fn move_toward_step_length_is_respected() {
    let from = Vec2Fx::ZERO;
    let to = Vec2Fx::new(fx(10), Fx::ZERO);
    let stepped = from.move_toward(to, Fx::ONE);
    assert_eq!(stepped.y, Fx::ZERO);
    // Truncation may leave the step a few ulps short, never long.
    assert!(stepped.x <= Fx::ONE);
    assert!(stepped.x >= Fx::ONE - Fx::DELTA * 16);
}

#[test]
fn move_toward_zero_distance_stays_put() {
    let p = Vec2Fx::new(fx(2), fx(2));
    assert_eq!(p.move_toward(p, Fx::ONE), p);
}

#[test]
fn move_toward_is_equivariant_under_half_turns() {
    let world_center_twice = Vec2Fx::new(fx(48), fx(30));
    let from = Vec2Fx::new(Fx::lit("6.5"), Fx::lit("8.5"));
    let target = Vec2Fx::new(Fx::lit("7.5"), Fx::lit("4.5"));
    let mirrored_from = world_center_twice - from;
    let mirrored_target = world_center_twice - target;
    let step = Fx::lit("0.125");

    let advance_twice = |mut pos: Vec2Fx, goal| {
        for _ in 0..2 {
            pos = pos.move_toward(goal, step);
        }
        pos
    };
    assert_eq!(
        world_center_twice - advance_twice(from, target),
        advance_twice(mirrored_from, mirrored_target),
        "opposite movement rays must accumulate the same fixed-point step"
    );
}

#[test]
fn vector_scalar_operations_preserve_exact_negation() {
    let vector = Vec2Fx::new(Fx::lit("0.713579"), Fx::lit("-0.248163"));
    for scalar in [Fx::lit("0.1729"), Fx::lit("-0.1729")] {
        assert_eq!((-vector) * scalar, -(vector * scalar));
        assert_eq!((-vector) / scalar, -(vector / scalar));
    }
}

#[test]
fn vector_scalar_operations_preserve_extreme_and_exact_signed_semantics() {
    let extremes = Vec2Fx::new(Fx::MIN, Fx::MAX);
    assert_eq!(extremes * Fx::ONE, extremes);
    assert_eq!(extremes / Fx::ONE, extremes);
    assert_eq!(extremes * Fx::ZERO, Vec2Fx::ZERO);
    assert_eq!(Vec2Fx::ZERO * Fx::MIN, Vec2Fx::ZERO);

    let exact = Vec2Fx::new(fx(2), fx(-4));
    assert_eq!(exact * Fx::lit("-0.5"), Vec2Fx::new(fx(-1), fx(2)));
    assert_eq!(exact / fx(-2), Vec2Fx::new(fx(-1), fx(2)));
}

#[test]
#[should_panic(expected = "attempt to divide by zero")]
fn vector_division_by_zero_still_panics() {
    let _ = Vec2Fx::new(Fx::MIN, Fx::ONE) / Fx::ZERO;
}

#[test]
#[should_panic(expected = "overflow")]
fn vector_multiplication_overflow_still_panics() {
    let _ = Vec2Fx::new(Fx::MIN, Fx::ZERO) * fx(-1);
}

#[test]
#[should_panic(expected = "overflow")]
fn vector_division_overflow_still_panics() {
    let _ = Vec2Fx::new(Fx::MIN, Fx::ZERO) / fx(-1);
}
