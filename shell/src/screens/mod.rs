//! Per-screen state objects, extracted from the main loop's mode match.
//!
//! The 0.9 endgame's lesson: every review-found shell bug lived in a
//! mode arm no headless test could reach. Each screen here owns its
//! menus and its update logic, takes raw events, and returns a
//! transition — windowless by construction, so the whole flow drives
//! in unit tests. The main loop keeps only drawing and session wiring.

pub mod home;
pub mod pause;
pub mod settings;
pub mod shelf;
pub mod wizard;
