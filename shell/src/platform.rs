//! Platform facts the shell's presentation depends on.

/// Whether this build's only pointer is a fingertip: no hardware keys,
/// mouse, hover, or wheel reach the app. Code branches on this constant
/// rather than `#[cfg]` so both sides compile, lint, and test on every
/// target; pure helpers take it as a parameter.
pub(crate) const TOUCH_ONLY: bool = cfg!(target_os = "ios");
