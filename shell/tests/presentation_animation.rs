//! Runs the presentation animation module's tests outside the shell binary.

#![allow(clippy::disallowed_types)]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

#[path = "../src/presentation_animation.rs"]
mod presentation_animation;
