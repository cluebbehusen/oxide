//! Fixed-point math for simulation code.
//!
//! Sim crates deny float arithmetic outright; this module is what they use
//! instead. [`Fx`] is signed Q32.32 — enough integer range for any sane map
//! and enough fraction for smooth sub-tile movement, with all operations
//! bit-exact across platforms. Square root is implemented on top of integer
//! `isqrt`, so it is deterministic too (`libm` never gets involved).

use fixed::types::I32F32;
use serde::{Deserialize, Serialize};

/// The one fixed-point type used throughout simulation code (signed Q32.32).
pub type Fx = I32F32;

/// One half, as a constant (tile centers sit at `tile + 0.5`).
pub const HALF: Fx = Fx::lit("0.5");

/// Deterministic square root. Panics if `x` is negative.
///
/// Exact where the argument is a perfect square, and within one ulp of the
/// true value otherwise: for `x` with raw value `r`, computes
/// `isqrt(r << 32)`, which is `floor(sqrt(x))` in Q32.32.
pub fn sqrt(x: Fx) -> Fx {
    assert!(x >= Fx::ZERO, "sqrt of negative fixed-point value: {x}");
    let wide = (x.to_bits() as u128) << 32;
    Fx::from_bits(wide.isqrt() as i64)
}

/// A 2D vector of [`Fx`] components.
///
/// Field-by-field `Ord` (x, then y) exists so vectors can serve as
/// deterministic tie-breakers, not because the ordering is meaningful.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Vec2Fx {
    /// X component, in world units (1.0 = one tile).
    pub x: Fx,
    /// Y component, in world units (1.0 = one tile).
    pub y: Fx,
}

impl Vec2Fx {
    /// The zero vector.
    pub const ZERO: Self = Self {
        x: Fx::ZERO,
        y: Fx::ZERO,
    };

    /// Builds a vector from components.
    pub const fn new(x: Fx, y: Fx) -> Self {
        Self { x, y }
    }

    /// Squared length. Prefer this over [`Self::length`] for comparisons —
    /// it avoids the square root entirely.
    pub fn length_sq(self) -> Fx {
        self.x * self.x + self.y * self.y
    }

    /// Length, via deterministic [`sqrt`].
    pub fn length(self) -> Fx {
        sqrt(self.length_sq())
    }

    /// Squared distance to `other`.
    pub fn dist_sq(self, other: Self) -> Fx {
        (other - self).length_sq()
    }

    /// Distance to `other`, via deterministic [`sqrt`].
    pub fn dist(self, other: Self) -> Fx {
        (other - self).length()
    }

    /// Moves from `self` toward `target` by at most `max_step`, arriving
    /// exactly (no overshoot, no orbiting).
    ///
    /// Off-axis steps may fall short of `max_step` by a few ulps because the
    /// direction ratio truncates; deterministic, and irrelevant at game
    /// scale.
    pub fn move_toward(self, target: Self, max_step: Fx) -> Self {
        let delta = target - self;
        let dist = delta.length();
        if dist <= max_step {
            target
        } else {
            // dist > max_step >= 0, so the ratio is in (0, 1) and division
            // cannot overflow or divide by zero. Vec2Fx's scalar operation
            // preserves exact negation, so opposite rays take opposite steps.
            self + delta * (max_step / dist)
        }
    }
}

impl core::ops::Add for Vec2Fx {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl core::ops::Sub for Vec2Fx {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl core::ops::Neg for Vec2Fx {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl core::ops::Mul<Fx> for Vec2Fx {
    type Output = Self;
    fn mul(self, rhs: Fx) -> Self {
        Self::new(
            sign_symmetric_mul(self.x, rhs),
            sign_symmetric_mul(self.y, rhs),
        )
    }
}

impl core::ops::Div<Fx> for Vec2Fx {
    type Output = Self;
    fn div(self, rhs: Fx) -> Self {
        Self::new(
            sign_symmetric_div(self.x, rhs),
            sign_symmetric_div(self.y, rhs),
        )
    }
}

/// Restores a signed raw result when it fits. The magnitude is wider than an
/// `i64`, so `Fx::MIN` is representable without ever negating it.
fn signed_magnitude(magnitude: u128, negative: bool) -> Option<Fx> {
    let signed = if negative {
        -(magnitude as i128)
    } else {
        magnitude as i128
    };
    i64::try_from(signed).ok().map(Fx::from_bits)
}

/// Multiplies raw magnitudes, truncating toward zero before restoring sign.
/// The fixed crate's signed multiply shifts a negative wide product and thus
/// rounds it one ulp below the corresponding positive result. Geometry needs
/// the stronger identity `(-v) * s == -(v * s)` for half-turn parity.
fn sign_symmetric_mul(lhs: Fx, rhs: Fx) -> Fx {
    let negative = (lhs < Fx::ZERO) != (rhs < Fx::ZERO);
    let magnitude =
        ((lhs.to_bits().unsigned_abs() as u128) * (rhs.to_bits().unsigned_abs() as u128)) >> 32;
    signed_magnitude(magnitude, negative).unwrap_or_else(|| lhs * rhs)
}

/// Divides raw magnitudes, preserving the fixed operator's division-by-zero
/// and overflow behavior while making every representable result sign
/// symmetric.
fn sign_symmetric_div(lhs: Fx, rhs: Fx) -> Fx {
    if rhs == Fx::ZERO {
        return lhs / rhs;
    }
    let negative = (lhs < Fx::ZERO) != (rhs < Fx::ZERO);
    let numerator = (lhs.to_bits().unsigned_abs() as u128) << 32;
    let magnitude = numerator / (rhs.to_bits().unsigned_abs() as u128);
    signed_magnitude(magnitude, negative).unwrap_or_else(|| lhs / rhs)
}

impl core::ops::AddAssign for Vec2Fx {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl core::ops::SubAssign for Vec2Fx {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

#[cfg(test)]
mod tests {
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
}
