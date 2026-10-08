use super::*;

#[test]
fn exact_window_retains_high_rate_frames_between_simulation_ticks() {
    let mut profiler = FrameProfiler::new(true);
    profiler.arm(0, 480).unwrap();
    assert!(profiler.take_start_barrier());
    for tick in 0..480 {
        for frame in 0..64 {
            profiler.record(FrameObservation {
                mode: "playing",
                active_playing: true,
                tick_start: tick,
                tick_end: tick + u64::from(frame == 63),
                work_ms: 0.2,
                units: 1,
                buildings: 1,
            });
        }
    }
    let view = profiler.snapshot(false);
    assert_eq!(view.frames, 480 * 64);
    assert_eq!(view.tick_start, 0);
    assert_eq!(view.tick_end, 480);
    assert_eq!(view.ticks_presented, 480);
    let window = view.window.unwrap();
    assert!(window.complete);
    assert!(!window.truncated);
}

#[test]
fn snapshot_reports_native_work_and_can_reset_the_window() {
    let mut profiler = FrameProfiler::new(true);
    for (index, work_ms) in [1.0, 2.0, 40.0].into_iter().enumerate() {
        profiler.record(FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 100 + index as u64,
            tick_end: 101 + index as u64,
            work_ms,
            units: 10 + index,
            buildings: 4,
        });
    }

    let view = profiler.snapshot(true);
    assert_eq!(view.renderer, "gpu");
    assert_eq!(view.frames, 3);
    assert_eq!(view.tick_start, 100);
    assert_eq!(view.tick_end, 103);
    assert_eq!(view.ticks_presented, 3);
    assert_eq!(view.work.p50_ms, 2.0);
    assert_eq!(view.work.p95_ms, 40.0);
    assert_eq!(view.work_over_16_7_ms, 1);
    assert_eq!(view.work_over_33_3_ms, 1);
    assert_eq!(view.slowest.expect("slow frame").tick_end, 103);
    assert_eq!(profiler.snapshot(false).frames, 0);
}

#[test]
fn disabled_profiling_has_no_samples() {
    let mut profiler = FrameProfiler::new(false);
    profiler.record(FrameObservation {
        mode: "playing",
        active_playing: true,
        tick_start: 1,
        tick_end: 2,
        work_ms: 99.0,
        units: 1,
        buildings: 1,
    });
    assert_eq!(profiler.snapshot(false).frames, 0);
}

#[test]
fn profiling_windows_require_an_enabled_forward_tick_range() {
    let mut disabled = FrameProfiler::new(false);
    assert!(disabled.arm(10, 20).unwrap_err().contains("disabled"));

    let mut profiler = FrameProfiler::new(true);
    assert!(profiler.arm(10, 10).unwrap_err().contains("greater"));
    assert!(profiler.arm(20, 10).unwrap_err().contains("greater"));
    profiler.arm(10, 20).expect("valid window");
    assert_eq!(profiler.stop_tick(), Some(20));
}

#[test]
fn exact_window_keeps_contiguous_active_playing_frames_including_tick_waits() {
    let mut profiler = FrameProfiler::new(true);
    profiler.arm(10, 14).unwrap();
    assert!(profiler.take_start_barrier());
    assert!(!profiler.take_start_barrier());
    for observation in [
        FrameObservation {
            mode: "pause_menu",
            active_playing: false,
            tick_start: 10,
            tick_end: 10,
            work_ms: 50.0,
            units: 1,
            buildings: 1,
        },
        FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 10,
            tick_end: 10,
            work_ms: 1.0,
            units: 10,
            buildings: 4,
        },
        FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 10,
            tick_end: 12,
            work_ms: 2.0,
            units: 10,
            buildings: 4,
        },
        FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 12,
            tick_end: 14,
            work_ms: 3.0,
            units: 11,
            buildings: 4,
        },
        FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 14,
            tick_end: 15,
            work_ms: 99.0,
            units: 11,
            buildings: 4,
        },
    ] {
        profiler.record(observation);
    }
    let view = profiler.snapshot(false);
    assert_eq!(view.frames, 3);
    assert_eq!(view.tick_start, 10);
    assert_eq!(view.tick_end, 14);
    assert_eq!(view.ticks_presented, 4);
    assert!(view.slowest.iter().all(|frame| frame.mode == "playing"));
    let window = view.window.unwrap();
    assert!(window.complete);
    assert!(window.elapsed_ms > 0.0);
    assert!(!window.truncated);
    assert_eq!(profiler.stop_tick(), None);
}

#[test]
fn retained_samples_are_bounded() {
    let mut profiler = FrameProfiler::new(true);
    profiler
        .arm(0, MAX_SAMPLES as u64 + 10)
        .expect("long window");
    assert!(profiler.take_start_barrier());
    for tick in 0..(MAX_SAMPLES as u64 + 5) {
        profiler.record(FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: tick,
            tick_end: tick + 1,
            work_ms: 1.0,
            units: 1,
            buildings: 1,
        });
    }
    let view = profiler.snapshot(false);
    assert_eq!(view.frames, MAX_SAMPLES);
    assert_eq!(view.tick_start, 5);
    assert_eq!(view.tick_end, MAX_SAMPLES as u64 + 5);
    let window = view.window.expect("armed window");
    assert!(!window.complete);
    assert!(window.truncated, "dropping an in-window sample is reported");
}

#[test]
fn an_exact_window_ignores_discontinuous_or_overshooting_frames() {
    let mut profiler = FrameProfiler::new(true);
    profiler.arm(10, 12).expect("window");
    assert!(profiler.take_start_barrier());
    for observation in [
        FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 9,
            tick_end: 10,
            work_ms: 90.0,
            units: 99,
            buildings: 99,
        },
        FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 10,
            tick_end: 13,
            work_ms: 80.0,
            units: 88,
            buildings: 88,
        },
        FrameObservation {
            mode: "playing",
            active_playing: true,
            tick_start: 10,
            tick_end: 12,
            work_ms: 2.0,
            units: 8,
            buildings: 4,
        },
    ] {
        profiler.record(observation);
    }
    let view = profiler.snapshot(false);
    assert_eq!(view.frames, 1);
    assert_eq!(view.tick_start, 10);
    assert_eq!(view.tick_end, 12);
    assert_eq!(view.slowest.expect("one sample").units, 8);
    assert!(view.window.expect("window").complete);
}
