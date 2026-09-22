//! Shared simulation fixtures and bot construction.
#![allow(dead_code)]
use oxide_sim::{PlayerId, Scenario};
#[path = "../../../sim/tests/common/mod.rs"]
pub mod simulation;

/// The ordinary Standard profile for focused bot integration checks.
pub fn standard_brain(scenario: &Scenario, player: PlayerId) -> oxide_bot::SeatBot {
    oxide_bot::SeatBot::balanced(
        player,
        std::sync::Arc::new(oxide_bot::PublicMapBriefing::from_scenario(scenario).unwrap()),
    )
}
