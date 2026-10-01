//! Scripted-bot integration suites, compiled as one test binary.
//!
//! Each suite is a module so the workspace builds and links one executable
//! here instead of one per file.

mod battlefield_adaptation;
mod behavior_construction;
mod bot_brain;
mod bot_frames;
mod common;
mod determinism;
mod ground_symmetry;
mod mine_construction;
mod recon_support;
mod scripted_bot;
mod teams;

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
