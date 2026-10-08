#[test]
fn a_budgeted_seek_lands_bit_identical_to_a_straight_one() {
    // The slices must be invisible: however many frames a seek is
    // spread across, the state it lands on is the state one big
    // seek produces.
    let replay = recorded_match();
    let mut straight = Playback::load(replay.clone()).unwrap();
    straight.seek(333);
    let expected = straight.state.hash();

    let mut sliced = Playback::load(replay).unwrap();
    let mut slices = 0;
    while !sliced.seek_step(333, 50) {
        slices += 1;
        assert!(slices < 100, "the budget must make progress");
    }
    assert_eq!(sliced.position(), 333);
    assert_eq!(sliced.state.hash(), expected, "slices changed the state");
    assert!(slices >= 5, "a 50-tick budget takes several frames");
}

use super::*;
use crate::runner;
use oxide_sim::Scenario;

fn recorded_match() -> GameReplay {
    let mut scenario = Scenario::skirmish();
    crate::bench::all_bots(&mut scenario);
    runner::run_scenario(&scenario, 900, true, true)
        .unwrap()
        .replay
        .unwrap()
}

#[test]
fn a_seeked_position_matches_the_straight_run() {
    let replay = recorded_match();
    let mut straight = Playback::load(replay.clone()).unwrap();
    straight.advance(700);
    let truth = straight.state.hash();

    let mut seeker = Playback::load(replay).unwrap();
    seeker.advance(900);
    assert!(seeker.at_end());
    seeker.seek(700); // backward across two checkpoints
    assert_eq!(seeker.position(), 700);
    assert_eq!(
        seeker.state.hash(),
        truth,
        "a seek is a re-simulation, not an approximation"
    );
    seeker.seek(123); // backward into the first checkpoint span
    seeker.seek(700); // and forward again
    assert_eq!(seeker.state.hash(), truth);
}

#[test]
fn checkpoint_memory_is_bounded_at_every_length() {
    for total in [0, 900, 40_000, 500_000, 2_000_000] {
        let cadence = checkpoint_cadence(total);
        assert!(cadence >= CHECKPOINT_EVERY);
        assert!(cadence.is_power_of_two());
        assert!(
            total / cadence <= MAX_CHECKPOINTS,
            "{total} ticks at cadence {cadence} keeps too many clones"
        );
    }
}

#[test]
fn mixed_seek_budgets_restore_real_checkpoints_without_replaying_commands_twice() {
    let mut replay = recorded_match();
    replay.meta.ticks = Some(5_000);
    let mut sliced = Playback::load(replay.clone()).unwrap();
    sliced.seek(5_000);
    assert!(sliced.checkpoints.len() >= 4);
    for target in [0, 2_048, 4_100, 1_024, 4_096, 5_000, u64::MAX] {
        let mut straight = Playback::load(replay.clone()).unwrap();
        straight.advance(target.min(5_000));
        sliced.seek_step(target, 0);
        let mut slices = 0;
        while !sliced.seek_step(target, 17) {
            slices += 1;
            assert!(slices < 300);
        }
        assert_eq!(sliced.state.hash(), straight.state.hash());
        assert_eq!(sliced.next_cmd, straight.next_cmd);
        let position = sliced.position();
        let hash = sliced.state.hash();
        assert!(sliced.seek_step(position, 0));
        assert_eq!(sliced.state.hash(), hash);
    }
}

#[test]
fn an_absurd_claimed_length_is_refused_interactively() {
    let mut replay = recorded_match();
    replay.meta.ticks = Some(1_000_000_000);
    assert!(
        Playback::load(replay).is_err(),
        "a billion-tick claim must not hang the first End press"
    );
}

#[test]
fn end_home_end_replays_a_suffix_not_the_record() {
    let replay = recorded_match();
    let mut pb = Playback::load(replay).unwrap();
    pb.advance(900);
    let full = pb.state.hash();
    pb.seek(0);
    assert_eq!(pb.position(), 0);
    pb.seek(900);
    assert_eq!(
        pb.state.hash(),
        full,
        "the round trip lands on the same bytes"
    );
    assert!(
        !pb.checkpoints.is_empty(),
        "forward checkpoints survive backward seeks"
    );
}

#[test]
fn playback_never_outruns_the_record() {
    let replay = recorded_match();
    let mut pb = Playback::load(replay).unwrap();
    pb.advance(5_000);
    assert_eq!(pb.position(), 900, "the record ends where it ends");
    pb.seek(2_000);
    assert_eq!(pb.position(), 900);
}

#[test]
fn missing_duration_uses_the_tick_after_the_last_command() {
    let mut inferred = recorded_match();
    inferred.meta.ticks = None;
    let total = inferred
        .commands
        .last()
        .expect("the bot fixture issues commands")
        .tick
        + 1;

    let mut explicit = inferred.clone();
    explicit.meta.ticks = Some(total);
    let mut inferred_playback = Playback::load(inferred).unwrap();
    let mut explicit_playback = Playback::load(explicit).unwrap();
    assert_eq!(inferred_playback.total(), total);

    inferred_playback.seek(u64::MAX);
    explicit_playback.seek(total);
    assert_eq!(inferred_playback.position(), total);
    assert_eq!(
        inferred_playback.state.hash(),
        explicit_playback.state.hash(),
        "legacy records without duration reproduce the same inferred span"
    );
}
