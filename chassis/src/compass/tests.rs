use super::*;

#[test]
fn heading_of_rounds_to_the_nearest_compass_step() {
    assert_eq!(heading_of(Vec2Fx::new(Fx::lit("3"), Fx::ZERO)), 0);
    assert_eq!(heading_of(Vec2Fx::new(Fx::ZERO, Fx::lit("-2"))), 192);
    assert_eq!(heading_of(Vec2Fx::new(Fx::lit("1"), Fx::lit("1"))), 32);
}

fn exhaustive_heading(v: Vec2Fx) -> u8 {
    let mut best = 0;
    let mut best_dot = v.x;
    for k in 1..=255 {
        let d = dir(k);
        let dot = d.x * v.x + d.y * v.y;
        if dot > best_dot {
            best = k;
            best_dot = dot;
        }
    }
    best
}

#[test]
fn quadrant_search_preserves_fixed_point_rounding_and_first_ties() {
    for x in -64..=64 {
        for y in -64..=64 {
            let v = Vec2Fx::new(Fx::from_bits(x), Fx::from_bits(y));
            assert_eq!(heading_of(v), exhaustive_heading(v), "{v:?}");
        }
    }
    for step in 0..=255u8 {
        let v = dir(step);
        let reflected_x = dir(128u8.wrapping_sub(step));
        let reflected_y = dir(step.wrapping_neg());
        assert_eq!(reflected_x, Vec2Fx::new(-v.x, v.y));
        assert_eq!(reflected_y, Vec2Fx::new(v.x, -v.y));
        for scale in [Fx::from_bits(1), Fx::lit("0.01"), Fx::ONE, Fx::lit("8192")] {
            let boundary = v + dir(step.wrapping_add(1));
            for dx in [-1, 0, 1] {
                for dy in [-1, 0, 1] {
                    let v = Vec2Fx::new(
                        boundary.x * scale + Fx::from_bits(dx),
                        boundary.y * scale + Fx::from_bits(dy),
                    );
                    assert_eq!(heading_of(v), exhaustive_heading(v), "{v:?}");
                }
            }
        }
    }
    let mut rng = crate::rng::Pcg32::new(0x1234, 7);
    for _ in 0..20_000 {
        let v = Vec2Fx::new(
            Fx::from_bits(i64::from(rng.next_u32().cast_signed()) << 13),
            Fx::from_bits(i64::from(rng.next_u32().cast_signed()) << 13),
        );
        assert_eq!(heading_of(v), exhaustive_heading(v), "{v:?}");
    }
}

#[test]
fn every_step_is_a_unit_vector() {
    for (k, v) in COMPASS_256.iter().enumerate() {
        let len_sq = v.x * v.x + v.y * v.y;
        let err = (len_sq - Fx::ONE).abs();
        assert!(err < Fx::lit("0.0001"), "step {k} has |v|^2 = {len_sq:?}");
    }
}

#[test]
fn quarter_turns_land_on_the_axes() {
    assert_eq!(dir(0), Vec2Fx::new(Fx::ONE, Fx::ZERO));
    assert_eq!(dir(64), Vec2Fx::new(Fx::ZERO, Fx::ONE));
    assert_eq!(dir(128), Vec2Fx::new(-Fx::ONE, Fx::ZERO));
    assert_eq!(dir(192), Vec2Fx::new(Fx::ZERO, -Fx::ONE));
}

#[test]
fn adjacent_steps_stay_adjacent() {
    // One step is about 1.4 degrees; consecutive directions must
    // never jump. Catches a shuffled or truncated table.
    for k in 0..=u8::MAX {
        let a = dir(k);
        let b = dir(k.wrapping_add(1));
        let dot = a.x * b.x + a.y * b.y;
        assert!(dot > Fx::lit("0.999"), "step {k} breaks continuity");
    }
}
