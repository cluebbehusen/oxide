//! Presentation-side numeric conversions.
//!
//! Screen and world measures are floats. Converting one to an integer
//! truncates toward zero and saturates at the target's bounds, with NaN
//! becoming zero, which is what every caller here wants; callers that need
//! another rounding floor, ceil or round first.

use chassis::grid::TilePos;
use macroquad::math::Vec2;

/// A font size in whole pixels, as macroquad's text API takes it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "presentation floats saturate into integers by design"
)]
pub(crate) fn font_size(px: f32) -> u16 {
    px as u16
}

/// The tile containing a world position.
pub(crate) fn tile_at(world: Vec2) -> TilePos {
    TilePos::new(to_i32(world.x.floor()), to_i32(world.y.floor()))
}

/// `value` truncated toward zero, saturating at `i32`'s bounds.
#[expect(
    clippy::cast_possible_truncation,
    reason = "presentation floats saturate into integers by design"
)]
pub(crate) fn to_i32(value: f32) -> i32 {
    value as i32
}

/// `value` truncated toward zero, saturating at `u32`'s bounds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "presentation floats saturate into integers by design"
)]
pub(crate) fn to_u32(value: f32) -> u32 {
    value as u32
}

/// `value` truncated toward zero, saturating at `u64`'s bounds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "presentation floats saturate into integers by design"
)]
pub(crate) fn to_u64(value: f32) -> u64 {
    value as u64
}

/// `value` truncated toward zero, saturating at `u8`'s bounds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "presentation floats saturate into integers by design"
)]
pub(crate) fn to_u8(value: f32) -> u8 {
    value as u8
}

/// `value` truncated toward zero, saturating at `usize`'s bounds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "presentation floats saturate into integers by design"
)]
pub(crate) fn to_usize(value: f32) -> usize {
    value as usize
}

/// `value` at `f32` precision, which is all presentation needs.
#[expect(
    clippy::cast_possible_truncation,
    reason = "presentation needs no more than f32 precision"
)]
pub(crate) fn to_f32(value: f64) -> f32 {
    value as f32
}

/// Checked conversion for counts, indices and extents that fit their target
/// by construction.
pub(crate) trait Fit: Copy + std::fmt::Display {
    /// `self` as `T`.
    ///
    /// # Panics
    ///
    /// When `self` does not fit, naming the caller.
    #[track_caller]
    fn fit<T: TryFrom<Self>>(self) -> T {
        let Ok(value) = T::try_from(self) else {
            panic!("{self} does not fit in {}", std::any::type_name::<T>());
        };
        value
    }
}

impl<U: Copy + std::fmt::Display> Fit for U {}
