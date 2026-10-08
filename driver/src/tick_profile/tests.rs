use super::*;

const REPORT: &str = "\
Analysis of sampling oxide-driver (pid 1) every 1 millisecond
Call graph:
    100 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)
      100 main  (in oxide-driver) + 52  [0x1]
      + 20 _RNvMs1_NtCs1_9oxide_sim5stateNtB5_5State5clone  (in oxide-driver) + 4  [0x2]
      + 80 _RNvMs_NtCss5gFej5ujG_9oxide_sim4tickNtNtB6_5state5State4tick  (in oxide-driver) + 8  [0x3]
      +   50 _RNvNtNtCss5gFej5ujG_9oxide_sim4tick8movement18resolve_collisions  (in oxide-driver) + 1  [0x4]
      +   ! 30 core::slice::sort::quicksort::<(usize, usize)>  (in oxide-driver) + 1  [0x5]
      +   ! 5 _platform_memmove  (in libsystem_platform.dylib) + 1  [0x6]
      +   20 _RNvNtNtCss5gFej5ujG_9oxide_sim4tick5brain3run  (in oxide-driver) + 1  [0x7]
      +   : 12 _RNvNtNtCss5gFej5ujG_9oxide_sim4tick5brain3run  (in oxide-driver) + 1  [0x8]
Total number in stack (recursive counted multiple, when >=5):
        5 _platform_memmove  (in libsystem_platform.dylib) + 0  [0x6]
";

fn find<'a>(shares: &'a [Share], function: &str) -> &'a Share {
    shares
        .iter()
        .find(|share| share.function == function)
        .unwrap_or_else(|| panic!("{function} missing from {shares:?}"))
}

#[test]
fn shares_count_only_samples_inside_the_tick() {
    let profile = analyze(REPORT, None).unwrap();
    assert_eq!((profile.samples, profile.tick_samples), (100, 80));
    let collisions = "oxide_sim::tick::movement::resolve_collisions";
    assert_eq!(find(&profile.phases, collisions).samples, 50);
    assert_eq!(
        find(&profile.phases, "oxide_sim::tick::brain::run").samples,
        20
    );
    assert_eq!(
        find(&profile.phases, &format!("{TICK} (own work)")).samples,
        10
    );
    assert_eq!(find(&profile.own, collisions).samples, 15);
    assert_eq!(
        find(&profile.own, "core::slice::sort::quicksort").percent,
        37.5
    );
    assert!(
        profile
            .own
            .iter()
            .all(|share| !share.function.contains("clone"))
    );
}

#[test]
fn frames_above_the_tick_never_shadow_a_focus_match() {
    let wrapped = REPORT.replace(
        "      + 80 _RNvMs_NtCss5gFej5ujG_9oxide_sim4tickNtNtB6_5state5State4tick",
        "      + 80 _RNvMs_NtCs1_12oxide_driver12tick_profileNtB4_6Window3run  (in oxide-driver) + 1  [0x9]\n      +   80 _RNvMs_NtCss5gFej5ujG_9oxide_sim4tickNtNtB6_5state5State4tick",
    );
    let wrapped = wrapped
        .lines()
        .map(|line| {
            let deep = [
                "resolve_collisions",
                "quicksort",
                "_platform_memmove",
                "brain3run",
            ]
            .iter()
            .any(|frame| line.contains(frame));
            if deep {
                line.replacen("      +   ", "      +     ", 1)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let profile = analyze(&wrapped, Some("run")).unwrap();
    assert_eq!(profile.tick_samples, 80);
    let focus = profile.focus.unwrap();
    assert_eq!(
        focus.samples, 20,
        "brain::run is the outermost match inside the tick"
    );
}

#[test]
fn a_window_must_repeat_enough_to_be_sampled_whole() {
    assert!(ensure_repetitions(Duration::from_millis(500), 5).is_ok());
    assert!(ensure_repetitions(Duration::from_millis(501), 5).is_err());
}

#[test]
fn recursion_counts_once_in_inclusive_time() {
    let profile = analyze(REPORT, None).unwrap();
    assert_eq!(
        find(&profile.inclusive, "oxide_sim::tick::brain::run").samples,
        20
    );
    assert_eq!(
        find(&profile.own, "oxide_sim::tick::brain::run").samples,
        20
    );
}

#[test]
fn focus_breaks_down_the_outermost_match() {
    let profile = analyze(REPORT, Some("resolve_collisions")).unwrap();
    let focus = profile.focus.unwrap();
    assert_eq!(focus.samples, 50);
    assert_eq!(
        find(&focus.breakdown, "core::slice::sort::quicksort").samples,
        30
    );
    assert_eq!(find(&focus.breakdown, "_platform_memmove").samples, 5);
    assert_eq!(
        find(
            &focus.breakdown,
            "oxide_sim::tick::movement::resolve_collisions (own work)"
        )
        .samples,
        15
    );
}

#[test]
fn a_report_without_tick_samples_is_refused() {
    let idle = REPORT.replace("5State4tick", "5State4idle");
    assert!(analyze(&idle, None).is_err());
    assert!(analyze("no graph here", None).is_err());
}

#[test]
fn generic_arguments_leave_names_but_impl_paths_stay() {
    assert_eq!(
        readable("core::slice::sort::quicksort::<((usize, usize), u8), <u8 as Ord>::lt>"),
        "core::slice::sort::quicksort"
    );
    assert_eq!(
        readable("_RNvMs_NtCss5gFej5ujG_9oxide_sim4tickNtNtB6_5state5State4tick"),
        TICK
    );
    assert_eq!(
        readable("<alloc::vec::Vec<(u8, u16)> as core::iter::FromIterator<u8>>::from_iter"),
        "<alloc::vec::Vec as core::iter::FromIterator>::from_iter"
    );
}

#[test]
fn windows_rank_by_average_tick_and_keep_their_starting_units() {
    let times = [1, 1, 9, 9, 4, 4, 9];
    let windows = costliest(&times, 10, 2, &[5, 6, 7, 8], 3);
    let summary: Vec<_> = windows
        .iter()
        .map(|window| (window.from, window.ticks, window.avg_ns, window.units))
        .collect();
    assert_eq!(summary, [(12, 2, 9, 6), (16, 1, 9, 8), (14, 2, 4, 7)]);
}

fn scan_of(replay: &GameReplay) -> Result<Scan> {
    let dir = std::env::temp_dir().join(format!(
        "oxide-tick-scan-{}-{}",
        std::process::id(),
        replay.meta.ticks.unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.json");
    replay.save(&path).unwrap();
    let scan = scan(path.to_str().unwrap(), 5, 10);
    std::fs::remove_dir_all(&dir).unwrap();
    scan
}

#[test]
fn a_scan_times_every_recorded_tick() {
    let scan = scan_of(&crate::support_tests::replay_fixture()).unwrap();
    assert_eq!((scan.start, scan.end, scan.per_tick.count), (0, 12, 12));
    assert_eq!(
        scan.windows.iter().map(|window| window.ticks).sum::<u64>(),
        12
    );
    assert!(scan.slowest_tick < 12);
}

#[test]
fn a_scan_refuses_empty_and_overlong_recordings() {
    let mut empty = crate::support_tests::replay_fixture();
    empty.commands.clear();
    empty.meta.ticks = Some(0);
    assert!(scan_of(&empty).is_err());
    let mut overlong = crate::support_tests::replay_fixture();
    overlong.meta.ticks = Some(oxide_kit::MAX_REPLAY_TICKS + 1);
    assert!(scan_of(&overlong).is_err());
}

#[test]
fn a_window_replays_the_recorded_history() {
    let replay = crate::support_tests::replay_fixture();
    let mut straight = Playback::load(replay.clone()).unwrap();
    straight.seek(9);
    let window = Window::from_replay(replay.clone(), 4, 5).unwrap();
    assert_eq!(window.start.current_tick(), 4);
    assert_eq!(
        window.commands.iter().map(Vec::len).collect::<Vec<_>>(),
        [0, 1, 0, 0, 1]
    );
    let ran = window.run();
    assert_eq!(ran.hash(), straight.state.hash());
    assert_eq!(window.run().hash(), ran.hash(), "repetitions are identical");
    assert!(Window::from_replay(replay.clone(), 10, 5).is_err());
    assert!(Window::from_replay(replay, 4, 0).is_err());
}
