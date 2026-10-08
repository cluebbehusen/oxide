use super::*;

fn detailed() -> Performance {
    let mut perf = Performance::default();
    perf.configure(PerformanceDisplay::Detailed, 1);
    perf
}

#[test]
fn cadence_uses_elapsed_time_not_mean_instantaneous_fps() {
    let mut perf = detailed();
    for frame in 1..=100 {
        perf.record(Duration::from_millis(frame * 10), 10.0, Some(2.0));
    }
    assert_eq!(perf.view.fps, Some(100.0));
    assert_eq!(perf.view.frame_ms, Some(10.0));
    assert_eq!(perf.view.work_ms, Some(2.0));
    let mut perf = detailed();
    perf.record(Duration::from_millis(10), 10.0, Some(1.0));
    perf.record(Duration::from_millis(300), 290.0, Some(3.0));
    assert!((perf.view.fps.unwrap() - 2000.0 / 300.0).abs() < 1e-10);
    assert_eq!(perf.view.frame_ms, Some(150.0));
    assert_eq!(perf.view.work_ms, Some(2.0));
}

#[test]
fn hitches_survive_bucketing_and_history_expires_after_long_gaps() {
    let mut perf = detailed();
    perf.record(Duration::from_millis(100), 80.0, Some(5.0));
    perf.record(Duration::from_millis(110), 10.0, Some(1.0));
    assert_eq!(perf.view.graph[49], Some(80.0));
    perf.record(Duration::from_millis(400), 10.0, None);
    assert_eq!(perf.view.max_ms, Some(80.0));
    perf.record(Duration::from_millis(10_400), 10_000.0, None);
    assert_eq!(perf.view.max_ms, Some(10_000.0));
    assert_eq!(perf.view.graph.iter().flatten().count(), 1);
    for frame in 105..=1000 {
        perf.record(Duration::from_millis(frame * 100), 100.0, None);
    }
    assert_eq!(perf.view.graph.len(), 50);
    assert_eq!(perf.view.max_ms, Some(100.0));
}

#[test]
fn labels_refresh_quarterly_while_graph_keeps_new_hitches() {
    let mut perf = detailed();
    perf.record(Duration::from_millis(10), 10.0, None);
    perf.record(Duration::from_millis(100), 90.0, None);
    assert_eq!(perf.view.fps, Some(100.0));
    assert_eq!(perf.view.graph[49], Some(90.0));
    perf.record(Duration::from_millis(300), 200.0, None);
    assert_eq!(perf.view.fps, Some(10.0));
}

#[test]
fn invalid_values_cannot_poison_the_display() {
    let mut perf = detailed();
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        perf.record(Duration::ZERO, bad, Some(1.0));
    }
    assert_eq!(perf.view.fps, None);
    perf.record(Duration::ZERO, 20.0, Some(f64::NAN));
    assert_eq!(perf.view.fps, Some(50.0));
    assert_eq!(perf.view.work_ms, None);
}

#[test]
fn modes_contexts_and_session_resets_clear_history() {
    let mut perf = detailed();
    for (mode, context) in [
        (PerformanceDisplay::Fps, 1),
        (PerformanceDisplay::Fps, 2),
        (PerformanceDisplay::Detailed, 3),
        (PerformanceDisplay::Off, 1),
        (PerformanceDisplay::Detailed, 0),
    ] {
        perf.record(Duration::ZERO, 20.0, Some(5.0));
        perf.configure(mode, context);
        assert_eq!(perf.view.fps, None);
        assert!(perf.view.graph.iter().all(Option::is_none));
    }
    assert!(perf.begin(PerformanceDisplay::Off, 1).is_none());
    perf.record(Duration::ZERO, 20.0, Some(5.0));
    assert_eq!(perf.view.fps, None);
    perf.configure(PerformanceDisplay::Fps, 1);
    perf.record(Duration::ZERO, 20.0, Some(5.0));
    assert_eq!(perf.view.work_ms, None);
    perf.reset();
    assert_eq!(perf.view.mode, PerformanceDisplay::Off);
}
