//! Per-screen state objects.
//!
//! Each screen owns its menus and update logic, takes raw events, and
//! returns a transition. Screens need no window, so unit tests can drive
//! the whole flow; the main loop keeps drawing and session wiring.

pub mod browser;
pub mod codex;
pub mod final_map;
pub mod home;
pub mod lobby;
pub mod pause;
pub mod playback;
pub mod results;
pub mod settings;
pub mod shelf;
pub mod wizard;
