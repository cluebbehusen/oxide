//! Fixed-point math for simulation code.
//!
//! Sim crates deny float arithmetic; they use this module instead. [`Fx`] is
//! signed Q32.32 and every operation is bit-exact across platforms. Square
//! root is built on integer `isqrt`, so it is deterministic too.

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
    let wide = u128::from(x.to_bits().cast_unsigned()) << 32;
    Fx::from_bits(i64::try_from(wide.isqrt()).expect("the root of a 96-bit value fits in 48 bits"))
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

    /// Squared length. Prefer this over [`Self::length`] for comparisons; it
    /// avoids the square root.
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
    /// direction ratio truncates.
    #[must_use]
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
    let magnitude = i128::try_from(magnitude).ok()?;
    let signed = if negative { -magnitude } else { magnitude };
    i64::try_from(signed).ok().map(Fx::from_bits)
}

/// Multiplies raw magnitudes, truncating toward zero before restoring sign.
/// The fixed crate's signed multiply shifts a negative wide product and thus
/// rounds it one ulp below the corresponding positive result. Geometry needs
/// the stronger identity `(-v) * s == -(v * s)` for half-turn parity.
fn sign_symmetric_mul(lhs: Fx, rhs: Fx) -> Fx {
    let negative = (lhs < Fx::ZERO) != (rhs < Fx::ZERO);
    let magnitude =
        (u128::from(lhs.to_bits().unsigned_abs()) * u128::from(rhs.to_bits().unsigned_abs())) >> 32;
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
    let numerator = u128::from(lhs.to_bits().unsigned_abs()) << 32;
    let magnitude = numerator / u128::from(rhs.to_bits().unsigned_abs());
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
mod tests;
