//! Simulation integration suites, compiled as one test binary.
//!
//! Each suite is a module so the workspace builds and links one executable
//! here instead of one per file.

#![deny(clippy::float_arithmetic, clippy::disallowed_types)]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

mod aircraft_turning;
mod bastion_acquisition;
mod behavior_combat;
mod behavior_construction;
mod behavior_economy;
mod behavior_leash;
mod behavior_movement;
mod behavior_rules;
mod behavior_settle;
mod blind_targeting;
mod bombers;
mod building_contact;
mod cancel_order;
mod clicked_tile_goals;
mod combat_edges;
mod command_canonicalization;
mod common;
mod defense_focus;
mod defense_roles;
mod determinism;
mod domains;
mod extractors;
mod field_kit;
mod foundries;
mod fuzz;
mod ground_turning;
mod harvest_zones;
mod landing;
mod movement_lab;
mod peaks;
mod pits;
mod provisional_construction;
mod repair;
mod repair_bay;
mod repair_unit;
mod return_cargo;
mod roster;
mod salvage;
mod sandbox;
mod scuttler_contact;
mod scuttler_crowd;
mod shells;
mod smelter;
mod state_integrity;
mod surrender;
mod teams;
mod transports;
mod turret_acquisition;
mod unreachable_goals;
mod upgrades;
mod worker_contact;

#[test]
fn every_suite_file_is_declared() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/integration");
    let declared = include_str!("main.rs");
    for entry in std::fs::read_dir(dir).expect("suite directory") {
        let path = entry.expect("suite entry").path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if path.extension().is_some_and(|ext| ext == "rs") && stem != "main" {
            assert!(
                declared.contains(&format!("\nmod {stem};")),
                "tests/integration/{stem}.rs is not a module of the suite binary"
            );
        }
    }
}
