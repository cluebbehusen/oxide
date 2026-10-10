#![doc = include_str!("../README.md")]

pub mod audit;
pub mod auto;
pub mod bot_cost;
pub mod bot_eval;
pub mod bot_ladder;
pub mod bot_pressure;
pub mod client;
pub mod evaluation;
pub mod ios;
pub mod ledger;
pub mod pool;
pub mod profile;
pub mod replay_inspect;
pub mod replay_ledger;
pub mod replay_summary;
pub mod rotation;
pub mod seat_summary;
pub mod session;
pub mod shots;
pub mod smoke;
pub mod tick_profile;

/// Provenance of this driver build.
pub fn build_identity() -> oxide_kit::recovery::BuildIdentity {
    oxide_kit::recovery::BuildIdentity::new(
        env!("CARGO_PKG_VERSION"),
        env!("OXIDE_BUILD_REVISION"),
        env!("OXIDE_BUILD_DIRTY"),
    )
}

#[cfg(test)]
mod source_tests;
#[cfg(test)]
mod support_tests;
