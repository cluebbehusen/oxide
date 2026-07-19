//! The tests this whole architecture answers to: same inputs, same bits.
//!
//! Every test here runs the real skirmish scenario — if determinism breaks
//! anywhere in the sim, a hash comparison in this file goes red.

use chassis::replay::Replay;
use oxide_sim::bot::Bot;
use oxide_sim::{GameResult, PlayerCommand, SIM_VERSION, Scenario, State};

/// Advances one tick with bot commands included, optionally recording them.
fn tick_with_bots(
    state: &mut State,
    bots: &mut [Bot],
    recorder: &mut Option<&mut Replay<Scenario, PlayerCommand>>,
) {
    let mut commands: Vec<PlayerCommand> = Vec::new();
    for bot in bots.iter_mut() {
        commands.extend(bot.act(state));
    }
    if let Some(replay) = recorder {
        for command in &commands {
            replay.record(state.current_tick(), command.clone());
        }
    }
    state.tick(&commands);
}

/// Skirmish with both seats handed to bots.
fn bot_match() -> (Scenario, State, Vec<Bot>) {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
    }
    let state = scenario.build().unwrap();
    let bots = Bot::for_scenario(&scenario);
    assert_eq!(bots.len(), 2);
    (scenario, state, bots)
}

#[test]
fn identical_runs_stay_bit_identical() {
    let (_, mut a, mut bots_a) = bot_match();
    let (_, mut b, mut bots_b) = bot_match();
    for tick in 0..1200u64 {
        tick_with_bots(&mut a, &mut bots_a, &mut None);
        tick_with_bots(&mut b, &mut bots_b, &mut None);
        if tick % 100 == 0 {
            assert_eq!(a.hash(), b.hash(), "diverged by tick {tick}");
        }
    }
    assert_eq!(a.hash(), b.hash());
}

#[test]
fn serde_roundtrip_mid_run_continues_identically() {
    let (_, mut original, mut bots) = bot_match();
    for _ in 0..400 {
        tick_with_bots(&mut original, &mut bots, &mut None);
    }
    // Snapshot through JSON — the same path a debug-socket save would take.
    let json = serde_json::to_string(&original).unwrap();
    let mut restored: State = serde_json::from_str(&json).unwrap();
    assert_eq!(
        original.hash(),
        restored.hash(),
        "roundtrip must be lossless"
    );

    let mut bots_restored = bots.clone();
    for tick in 0..400u64 {
        tick_with_bots(&mut original, &mut bots, &mut None);
        tick_with_bots(&mut restored, &mut bots_restored, &mut None);
        if tick % 50 == 0 {
            assert_eq!(
                original.hash(),
                restored.hash(),
                "diverged {tick} after restore"
            );
        }
    }
}

#[test]
fn replay_reproduces_a_recorded_run() {
    let (scenario, mut live, mut bots) = bot_match();
    let mut replay = Replay::new(SIM_VERSION, scenario);
    {
        let mut rec = Some(&mut replay);
        for _ in 0..1500 {
            tick_with_bots(&mut live, &mut bots, &mut rec);
        }
    }
    let live_hash = live.hash();
    assert!(!replay.commands.is_empty(), "bots must have acted");

    // Replay: no bots anywhere — the command log is the whole game.
    let mut replayed = replay.setup.build().unwrap();
    let mut cursor = replay.cursor();
    for _ in 0..1500 {
        let commands: Vec<PlayerCommand> = cursor
            .take_tick(replayed.current_tick())
            .iter()
            .map(|t| t.command.clone())
            .collect();
        replayed.tick(&commands);
    }
    assert!(cursor.is_finished());
    assert_eq!(replayed.hash(), live_hash);
}

#[test]
fn bot_match_reaches_a_decisive_end() {
    let (_, mut state, mut bots) = bot_match();
    let mut ticks = 0u64;
    while state.result().is_none() {
        tick_with_bots(&mut state, &mut bots, &mut None);
        ticks += 1;
        assert!(
            ticks < 30_000,
            "bot match should conclude within 25 minutes of game time"
        );
    }
    assert!(
        matches!(state.result(), Some(GameResult::Victory { .. })),
        "mirror bots must not draw: {:?}",
        state.result()
    );
    // Sanity: a real game happened — armies were built beyond the 8 starters.
    assert!(state.units().iter().any(|u| u.id.0 >= 8));
}
