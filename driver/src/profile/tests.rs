use super::*;
use oxide_protocol::{FrameProfileWindowView, SlowFrameView, TimingSummary};

fn valid_profile() -> FrameProfileView {
    let timing = TimingSummary {
        mean_ms: 4.0,
        p50_ms: 3.0,
        p95_ms: 7.0,
        p99_ms: 8.0,
        max_ms: 9.0,
    };
    FrameProfileView {
        renderer: "gpu".to_string(),
        frames: 4,
        tick_start: 10,
        tick_end: 20,
        ticks_presented: 10,
        work: timing.clone(),
        interval: timing,
        work_over_16_7_ms: 0,
        work_over_33_3_ms: 0,
        slowest: Some(SlowFrameView {
            mode: "playing".to_string(),
            tick_start: 14,
            tick_end: 15,
            work_ms: 9.0,
            units: 8,
            buildings: 3,
        }),
        window: Some(FrameProfileWindowView {
            from_tick: 10,
            to_tick: 20,
            complete: true,
            elapsed_ms: 500.0,
            truncated: false,
        }),
    }
}

fn profile_error(profile: &FrameProfileView) -> String {
    validate_profile_result(profile, 10, 20)
        .unwrap_err()
        .to_string()
}

#[test]
fn options_reject_empty_or_reversed_tick_windows_before_launching() {
    let options = ProfileOptions {
        replay: Path::new("does-not-matter.json"),
        from: 50,
        to: 50,
        speed: 8.0,
        port: 4198,
        dev: false,
    };
    assert_eq!(
        run(&options).unwrap_err().to_string(),
        "--to must be greater than --from"
    );
}

#[test]
fn live_prefix_keeps_only_commands_before_the_resume_tick() {
    use oxide_sim::{Command, PlayerCommand, PlayerId, SIM_VERSION, Scenario};

    let mut replay = GameReplay::new(SIM_VERSION, "test", Scenario::skirmish());
    for tick in [3, 9, 10, 11] {
        replay.record(
            tick,
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Stop { units: Vec::new() },
            },
        );
    }
    replay.meta.ticks = Some(20);
    let prefix = prefix_at(replay, 10);
    assert_eq!(
        prefix
            .commands
            .iter()
            .map(|command| command.tick)
            .collect::<Vec<_>>(),
        vec![3, 9]
    );
    assert_eq!(prefix.meta.ticks, Some(10));
    assert_eq!(prefix.meta.kind.as_deref(), Some("save"));
    prefix.validate(Some(SIM_VERSION)).unwrap();
}

#[test]
fn only_the_resume_tick_is_bounded_by_the_source_record() {
    validate_resume_tick(100, 100).unwrap();
    assert_eq!(
        validate_resume_tick(100, 101).unwrap_err().to_string(),
        "requested resume tick 101 exceeds source record length 100"
    );
    // The measured `to` tick is intentionally absent here: a live
    // continuation may run arbitrarily beyond the source save's end.
}

#[test]
fn exact_profile_validation_returns_the_measured_wall_time() {
    assert_eq!(
        validate_profile_result(&valid_profile(), 10, 20).unwrap(),
        0.5
    );
}

#[test]
fn exact_profile_validation_requires_gpu_frames() {
    let mut profile = valid_profile();
    profile.renderer = "cpu".to_string();
    assert_eq!(profile_error(&profile), "shell reported a non-GPU renderer");

    let mut profile = valid_profile();
    profile.frames = 0;
    assert_eq!(
        profile_error(&profile),
        "shell returned an empty frame profile"
    );
}

#[test]
fn exact_profile_validation_requires_complete_untruncated_window_metadata() {
    let mut profile = valid_profile();
    profile.window = None;
    assert_eq!(
        profile_error(&profile),
        "shell omitted exact-window metadata"
    );

    let mut profile = valid_profile();
    profile.window.as_mut().unwrap().complete = false;
    assert_eq!(
        profile_error(&profile),
        "shell did not complete its exact profile window"
    );

    let mut profile = valid_profile();
    profile.window.as_mut().unwrap().truncated = true;
    assert_eq!(
        profile_error(&profile),
        "profile exceeded the shell's bounded sample retention"
    );

    let mut profile = valid_profile();
    profile.window.as_mut().unwrap().to_tick = 21;
    assert_eq!(
        profile_error(&profile),
        "shell reported the wrong exact profile window"
    );
}

#[test]
fn exact_profile_validation_rejects_mixed_or_empty_measurement_windows() {
    let mut profile = valid_profile();
    profile.ticks_presented = 9;
    assert!(profile_error(&profile).starts_with("profile samples did not cover exactly 10..20"));

    let mut profile = valid_profile();
    profile.slowest.as_mut().unwrap().mode = "results".to_string();
    assert_eq!(
        profile_error(&profile),
        "profile included a non-Playing frame"
    );

    let mut profile = valid_profile();
    profile.window.as_mut().unwrap().elapsed_ms = 0.0;
    assert_eq!(
        profile_error(&profile),
        "shell reported a zero-duration profile"
    );
}
