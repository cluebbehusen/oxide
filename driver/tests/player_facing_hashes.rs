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
//! Two rows per map at the common 6,000-tick horizon: the state hash, and a
//! running fold of the full command stream — each tick's commands folded with
//! the tick they were staged for — keyed `<map>#commands`. Separately keyed
//! later-horizon probes may extend a map without replacing that comparable
//! baseline. The command fold moves whenever any decision or its timing
//! changes, even where the worlds later reconverge, so it is the sharper
//! refactoring tripwire; the state hash anchors the world the commands actually
//! built.
//!
//! `tests/goldens/player-facing-hashes.json` obeys the same bless discipline
//! as the Overseer golden. An intentional player-facing behavior change is
//! expected to move rows here while leaving `state-hashes.json` untouched.
//! Inspect that drift and obtain explicit approval from the human user for
//! either a version bump or a same-version bless before changing the workspace
//! version or invoking `BLESS_SAME_VERSION=1`.

mod support;

use oxide_sim::Scenario;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use std::collections::BTreeMap;
use std::path::PathBuf;
use support::check_or_bless;

const DEFAULT_FIXTURE_TICKS: u64 = 6_000;
const SKYHOOK_EXTENDED_TICKS: u64 = 9_000;

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

fn run_ticks(name: &str) -> u64 {
    if name == "skyhook-anchorage" {
        SKYHOOK_EXTENDED_TICKS
    } else {
        DEFAULT_FIXTURE_TICKS
    }
}

struct MapRun {
    rows: Vec<(String, String)>,
    saw_lift_load: bool,
}

fn hash_rows(
    name: &str,
    suffix: &str,
    state_hash: u64,
    command_fold: u64,
) -> [(String, String); 2] {
    let key = format!("{name}{suffix}");
    [
        (key.clone(), oxide_protocol::hash_hex(state_hash)),
        (
            format!("{key}#commands"),
            oxide_protocol::hash_hex(command_fold),
        ),
    ]
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
    let mut rows = Vec::new();
    let final_tick = run_ticks(name);
    for _ in 0..final_tick {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        if !commands.is_empty() {
            // The tick is part of the fold: replay identity is "these
            // commands at this tick", so identical commands sliding to a
            // different tick must move this row even where the world
            // later reconverges.
            command_fold =
                chassis::hash::state_hash(&(command_fold, state.current_tick(), &commands));
            saw_lift_load |= commands
                .iter()
                .any(|c| matches!(c.command, oxide_sim::Command::Load { .. }));
        }
        state.tick(&commands);
        if state.current_tick() == DEFAULT_FIXTURE_TICKS {
            rows.extend(hash_rows(name, "", state.hash(), command_fold));
        }
    }
    if final_tick > DEFAULT_FIXTURE_TICKS {
        rows.extend(hash_rows(
            name,
            &format!("@{final_tick}"),
            state.hash(),
            command_fold,
        ));
    }
    MapRun {
        rows,
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
    let actual: BTreeMap<String, String> = runs.into_iter().flat_map(|run| run.rows).collect();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/player-facing-hashes.json");
    check_or_bless(&fixture, actual);
}
