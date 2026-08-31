//! State-hash fixtures for the shipped player-facing controller.
//!
//! The Overseer fixtures deliberately pin the frozen QA controller in every
//! seat, which leaves the configurable scripted opponent — the controller
//! players actually meet — with no whole-behavior tripwire at all. These
//! fixtures close that gap: representative shipped maps run with every seat
//! driven through the shipped seating path (`oxide_sim::bot::seat_bots`,
//! exactly as the shell and driver construct real matches) at Prime
//! difficulty, Balanced stance, and fixed per-seat personality seeds, to a
//! combat-phase horizon.
//!
//! Two rows per map: the final state hash, and a running fold of the full
//! command stream keyed `<map>#commands`. The command fold moves whenever
//! any decision changes, even where the worlds later reconverge, so it is
//! the sharper refactoring tripwire; the state hash anchors the world the
//! commands actually built.
//!
//! `tests/goldens/player-facing-hashes.json` obeys the same bless
//! discipline as the Overseer golden: `BLESS=1 cargo test -p oxide-driver`,
//! gated on `SIM_VERSION`, with `BLESS_SAME_VERSION=1` as the deliberate
//! exception. An intentional player-facing behavior change is expected to
//! move rows here while leaving `state-hashes.json` untouched; the commit
//! that re-blesses explains the movement.

mod support;

use oxide_sim::Scenario;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use std::collections::BTreeMap;
use std::path::PathBuf;
use support::check_or_bless;

const FIXTURE_TICKS: u64 = 6_000;

/// Representative repertoire spread: a 1v1, small and large team maps, the
/// transport-island economy, and the two largest shipped bot loads.
const MAPS: [&str; 6] = [
    "skirmish",
    "twin-forges",
    "three-shifts",
    "terminal-basin",
    "skyhook-anchorage",
    "gatework-array",
];

/// Fixed per-seat personality seed. Any stable spread works; it is part of
/// the fixture identity and must not change without a re-bless.
const fn personality_seed(seat: usize) -> u64 {
    9_000 + seat as u64
}

struct MapRun {
    state_row: (String, String),
    commands_row: (String, String),
    saw_lift_load: bool,
}

fn run_map(name: &str) -> MapRun {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../scenarios/{name}.json"));
    let mut scenario =
        Scenario::load(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    for (seat, player) in scenario.players.iter_mut().enumerate() {
        player.bot = true;
        player.bot_config = Some(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            personality_seed(seat),
        ));
    }
    let mut state = scenario
        .build()
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let mut bots = oxide_sim::bot::seat_bots(&scenario)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert_eq!(
        bots.len(),
        scenario.players.len(),
        "{name}: every seat must receive the shipped scripted controller"
    );

    let mut command_fold: u64 = 0;
    let mut saw_lift_load = false;
    for _ in 0..FIXTURE_TICKS {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        if !commands.is_empty() {
            command_fold = chassis::hash::state_hash(&(command_fold, &commands));
            saw_lift_load |= commands
                .iter()
                .any(|c| matches!(c.command, oxide_sim::Command::Load { .. }));
        }
        state.tick(&commands);
    }
    MapRun {
        state_row: (name.to_string(), oxide_protocol::hash_hex(state.hash())),
        commands_row: (
            format!("{name}#commands"),
            oxide_protocol::hash_hex(command_fold),
        ),
        saw_lift_load,
    }
}

#[test]
fn scripted_controller_matches_hash_fixtures() {
    let runs: Vec<MapRun> = std::thread::scope(|scope| {
        let handles: Vec<_> = MAPS
            .iter()
            .map(|name| scope.spawn(|| run_map(name)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a fixture run panicked"))
            .collect()
    });
    assert!(
        runs.iter().any(|run| run.saw_lift_load),
        "no fixture map exercised a transport Load — the lift path is uncovered; \
         adjust the map set or seeds so the fixture demonstrably reaches it"
    );
    let actual: BTreeMap<String, String> = runs
        .into_iter()
        .flat_map(|run| [run.state_row, run.commands_row])
        .collect();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/player-facing-hashes.json");
    check_or_bless(&fixture, actual);
}
