//! Where simulation time goes inside a window of recorded ticks.
//!
//! The replay is rebuilt to the window's first tick, then the window is
//! re-simulated from a clone of that world again and again while macOS's
//! `sample` records the thread's stack once a millisecond. Every repetition
//! does identical work, so even a single tick gathers thousands of samples.
//! Shares count only samples inside `State::tick`; cloning the world and the
//! loop around it are harness overhead, reported apart.

use crate::bot_cost::Summary;
use anyhow::{Context, Result, ensure};
use oxide_kit::GameReplay;
use oxide_kit::playback::Playback;
use oxide_sim::{PlayerCommand, State};
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The demangled name every sample inside the simulation passes through.
const TICK: &str = "<oxide_sim::state::State>::tick";

/// The world before a window's first tick and the commands each of its ticks
/// applies.
pub struct Window {
    /// The world before commands stamped with the window's first tick run.
    pub start: State,
    /// Recorded commands, one entry per tick of the window.
    pub commands: Vec<Vec<PlayerCommand>>,
}

impl Window {
    /// Rebuilds `replay` to tick `from` and gathers the commands of the
    /// `ticks` ticks that follow.
    pub fn from_replay(replay: GameReplay, from: u64, ticks: u64) -> Result<Self> {
        ensure!(ticks > 0, "a window spans at least one tick");
        let end = from.checked_add(ticks).context("window end overflows")?;
        let recorded = replay.commands.clone();
        let mut playback = Playback::load(replay)?;
        ensure!(
            from >= playback.start() && end <= playback.total(),
            "ticks {from}..{end} fall outside the recording's {}..{}",
            playback.start(),
            playback.total()
        );
        playback.seek(from);
        let first = recorded.partition_point(|command| command.tick < from);
        let mut commands = vec![Vec::new(); ticks as usize];
        for command in recorded[first..]
            .iter()
            .take_while(|command| command.tick < end)
        {
            commands[(command.tick - from) as usize].push(command.command.clone());
        }
        Ok(Self {
            start: playback.state,
            commands,
        })
    }

    /// Simulates the window once from its starting world.
    pub fn run(&self) -> State {
        let mut state = self.start.clone();
        for commands in &self.commands {
            state.tick(commands);
        }
        state
    }

    /// Wall time of the window's ticks, excluding the clone, repeated for at
    /// least `budget` and three repetitions.
    pub fn time(&self, budget: Duration) -> Summary {
        let began = Instant::now();
        let mut samples = Vec::new();
        while samples.len() < 3 || began.elapsed() < budget {
            let mut state = self.start.clone();
            let start = Instant::now();
            for commands in &self.commands {
                state.tick(commands);
            }
            samples.push(u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));
            std::hint::black_box(&state);
        }
        Summary::of(&samples)
    }

    /// Repeats the window while `sample` records this process for `seconds`,
    /// returning the sampler's report.
    pub fn sample(&self, seconds: u64) -> Result<String> {
        ensure_sampler()?;
        let path =
            std::env::temp_dir().join(format!("oxide-tick-profile-{}.txt", std::process::id()));
        let mut sampler = std::process::Command::new("/usr/bin/sample")
            .arg(std::process::id().to_string())
            .arg(seconds.to_string())
            .arg("1")
            .arg("-mayDie")
            .arg("-file")
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("starting /usr/bin/sample")?;
        while sampler.try_wait()?.is_none() {
            std::hint::black_box(self.run());
        }
        let output = sampler.wait_with_output()?;
        ensure!(
            output.status.success(),
            "sample failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let report =
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()));
        let _ = std::fs::remove_file(&path);
        report
    }
}

/// A profiled window: what it held, how long it took and where the time went.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// The replay the window came from.
    pub replay: String,
    /// The window's first tick.
    pub from: u64,
    /// Ticks in the window.
    pub ticks: u64,
    /// Units alive when the window starts.
    pub units: usize,
    /// Buildings standing when the window starts.
    pub buildings: usize,
    /// Wall time per repetition of the window, measured before sampling.
    pub window: Summary,
    /// Shares of the sampled time.
    pub profile: Profile,
}

/// Repetitions the sampler must see so a repetition cut off at the end of
/// sampling skews the shares toward the window's early ticks only slightly.
const MIN_REPETITIONS: u32 = 10;

/// Refuses platforms without macOS's `sample`.
fn ensure_sampler() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "tick-profile records with macOS's `sample`, which this platform lacks"
    );
    Ok(())
}

/// Refuses a window whose repetition takes `once` when `seconds` of sampling
/// would not repeat it [`MIN_REPETITIONS`] times.
fn ensure_repetitions(once: Duration, seconds: u64) -> Result<()> {
    let needed = once * MIN_REPETITIONS;
    ensure!(
        needed <= Duration::from_secs(seconds),
        "one repetition of the window takes {:.2} s, so {seconds} s of sampling covers it \
         fewer than {MIN_REPETITIONS} times; raise --seconds to at least {} or shorten --ticks",
        once.as_secs_f64(),
        needed.as_secs_f64().ceil()
    );
    Ok(())
}

/// Profiles ticks `from..from + ticks` of the replay at `path`, sampling for
/// `seconds`.
pub fn profile(
    path: &str,
    from: u64,
    ticks: u64,
    seconds: u64,
    focus: Option<&str>,
) -> Result<Report> {
    ensure_sampler()?;
    let replay = oxide_kit::load_replay(path).with_context(|| format!("loading {path}"))?;
    let window = Window::from_replay(replay, from, ticks)?;
    let began = Instant::now();
    std::hint::black_box(window.run());
    ensure_repetitions(began.elapsed(), seconds)?;
    let timing = window.time(Duration::from_secs(1));
    let profile = analyze(&window.sample(seconds)?, focus)?;
    Ok(Report {
        replay: path.to_owned(),
        from,
        ticks,
        units: window.start.units().len(),
        buildings: window.start.buildings().len(),
        window: timing,
        profile,
    })
}

impl Report {
    /// The window's timing above the profile's tables.
    pub fn table(&self, top: usize) -> String {
        let per_tick = |ns: u64| ns as f64 / self.ticks as f64 / 1000.0;
        format!(
            "{}: ticks {}..{} ({} units, {} buildings)\n\
             wall time per tick: avg {:.1} µs, p99 {:.1} µs over {} unsampled repetitions\n{}",
            self.replay,
            self.from,
            self.from + self.ticks,
            self.units,
            self.buildings,
            per_tick(self.window.avg_ns),
            per_tick(self.window.p99_ns),
            self.window.count,
            self.profile.table(top)
        )
    }
}

/// One function's samples, as a share of every sample inside the tick.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Share {
    /// Demangled name without generic arguments.
    pub function: String,
    /// Samples attributed to it.
    pub samples: u64,
    /// Percent of the samples inside `State::tick`.
    pub percent: f64,
}

/// Where one function's time goes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Focus {
    /// The name fragment that selected it.
    pub pattern: String,
    /// Samples inside the outermost matching frames.
    pub samples: u64,
    /// Its callees and its own work, as shares of the tick.
    pub breakdown: Vec<Share>,
}

/// A sampler report narrowed to the simulation tick.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    /// Samples of every recorded thread.
    pub samples: u64,
    /// Samples inside `State::tick`.
    pub tick_samples: u64,
    /// The tick's direct callees and its own work.
    pub phases: Vec<Share>,
    /// Work done in each function's own body.
    pub own: Vec<Share>,
    /// Work done in each function and everything it calls.
    pub inclusive: Vec<Share>,
    /// Breakdown of the function chosen with `--focus`.
    pub focus: Option<Focus>,
}

struct Frame {
    depth: usize,
    count: u64,
    name: String,
}

/// Reads the call graph of a macOS `sample` report.
fn frames(report: &str) -> Result<Vec<Frame>> {
    let graph = report
        .split_once("Call graph:")
        .context("the report has no call graph")?
        .1;
    let graph = graph
        .split("\nTotal number in stack")
        .next()
        .unwrap_or(graph);
    Ok(graph
        .lines()
        .filter_map(|line| {
            let depth = line.find(|c: char| c.is_ascii_digit())?;
            if !line[..depth]
                .chars()
                .all(|c| matches!(c, ' ' | '+' | '!' | ':' | '|'))
            {
                return None;
            }
            let rest = &line[depth..];
            let digits = rest.find(' ')?;
            let count = rest[..digits].parse().ok()?;
            let symbol = rest[digits + 1..]
                .split("  (in ")
                .next()
                .unwrap_or_default()
                .trim();
            Some(Frame {
                depth,
                count,
                name: readable(symbol),
            })
        })
        .collect())
}

/// Demangles a Rust symbol and drops generic arguments, keeping qualified
/// paths such as `<Type as Trait>::method`.
fn readable(symbol: &str) -> String {
    let name = rustc_demangle::try_demangle(symbol)
        .map_or_else(|_| symbol.to_owned(), |name| format!("{name:#}"));
    let mut out = String::with_capacity(name.len());
    let mut skipped = 0usize;
    for c in name.chars() {
        if skipped > 0 {
            match c {
                '<' => skipped += 1,
                '>' => skipped -= 1,
                _ => {}
            }
            continue;
        }
        if c == '<' {
            if out.ends_with("::") {
                out.truncate(out.len() - 2);
                skipped = 1;
                continue;
            }
            if out.ends_with(|last: char| last.is_alphanumeric() || last == '_') {
                skipped = 1;
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Narrows a `sample` report to the samples inside `State::tick`.
pub fn analyze(report: &str, focus: Option<&str>) -> Result<Profile> {
    let frames = frames(report)?;
    let mut parent = vec![None; frames.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&top| frames[top].depth >= frame.depth)
        {
            stack.pop();
        }
        parent[index] = stack.last().copied();
        stack.push(index);
    }
    let mut callees = vec![0u64; frames.len()];
    for (index, frame) in frames.iter().enumerate() {
        if let Some(up) = parent[index] {
            callees[up] += frame.count;
        }
    }
    let parents = &parent;
    let ancestors = move |mut index: usize| {
        std::iter::from_fn(move || {
            index = parents[index]?;
            Some(index)
        })
    };
    let outermost = |index: usize, matches: &dyn Fn(&str) -> bool| {
        matches(&frames[index].name) && !ancestors(index).any(|up| matches(&frames[up].name))
    };
    let is_tick = |name: &str| name == TICK;
    let roots: Vec<usize> = (0..frames.len())
        .filter(|&index| outermost(index, &is_tick))
        .collect();
    let in_tick =
        |index: usize| roots.contains(&index) || ancestors(index).any(|up| roots.contains(&up));
    let samples = frames
        .iter()
        .zip(&parent)
        .filter(|(_, up)| up.is_none())
        .map(|(frame, _)| frame.count)
        .sum();
    let tick_samples: u64 = roots.iter().map(|&root| frames[root].count).sum();
    ensure!(
        tick_samples > 0,
        "no samples landed inside `State::tick`; sample longer or widen the window"
    );
    let share = |totals: BTreeMap<String, u64>| {
        let mut shares: Vec<Share> = totals
            .into_iter()
            .filter(|(_, samples)| *samples > 0)
            .map(|(function, samples)| Share {
                function,
                samples,
                percent: 100.0 * samples as f64 / tick_samples as f64,
            })
            .collect();
        shares.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.function.cmp(&b.function)));
        shares
    };
    let breakdown = |selected: &[usize]| {
        let mut totals = BTreeMap::new();
        for &root in selected {
            *totals
                .entry(format!("{} (own work)", frames[root].name))
                .or_default() += frames[root].count - callees[root];
        }
        for (index, frame) in frames.iter().enumerate() {
            if parent[index].is_some_and(|up| selected.contains(&up)) {
                *totals.entry(frame.name.clone()).or_default() += frame.count;
            }
        }
        share(totals)
    };
    let mut own = BTreeMap::new();
    let mut inclusive = BTreeMap::new();
    for (index, frame) in frames.iter().enumerate() {
        if !in_tick(index) {
            continue;
        }
        *own.entry(frame.name.clone()).or_default() += frame.count - callees[index];
        let recursive = ancestors(index)
            .take_while(|up| in_tick(*up))
            .any(|up| frames[up].name == frame.name);
        if !recursive {
            *inclusive.entry(frame.name.clone()).or_default() += frame.count;
        }
    }
    let focus = focus.map(|pattern| {
        let matches = |name: &str| name.contains(pattern);
        // Frames above the tick, such as the harness's own `run`, never
        // shadow a match inside it.
        let selected: Vec<usize> = (0..frames.len())
            .filter(|&index| {
                in_tick(index)
                    && matches(&frames[index].name)
                    && !ancestors(index)
                        .take_while(|&up| in_tick(up))
                        .any(|up| matches(&frames[up].name))
            })
            .collect();
        Focus {
            pattern: pattern.to_owned(),
            samples: selected.iter().map(|&index| frames[index].count).sum(),
            breakdown: breakdown(&selected),
        }
    });
    Ok(Profile {
        samples,
        tick_samples,
        phases: breakdown(&roots),
        own: share(own),
        inclusive: share(inclusive),
        focus,
    })
}

impl Profile {
    /// Plain-text tables of the `top` rows of each view.
    pub fn table(&self, top: usize) -> String {
        let mut out = format!(
            "samples: {} inside State::tick of {} ({:.1}% harness overhead)\n",
            self.tick_samples,
            self.samples,
            100.0 * (self.samples - self.tick_samples) as f64 / self.samples.max(1) as f64
        );
        let mut section = |title: &str, shares: &[Share]| {
            out.push_str(&format!("\n{title}\n"));
            for share in shares.iter().take(top) {
                out.push_str(&format!(
                    "{:>6.1}%  {:>7}  {}\n",
                    share.percent, share.samples, share.function
                ));
            }
        };
        section("phases (direct callees of State::tick)", &self.phases);
        section("own work", &self.own);
        section("inclusive", &self.inclusive);
        if let Some(focus) = &self.focus {
            section(
                &format!(
                    "focus `{}` ({:.1}% of the tick)",
                    focus.pattern,
                    100.0 * focus.samples as f64 / self.tick_samples as f64
                ),
                &focus.breakdown,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
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
    fn a_window_replays_the_recorded_history() {
        let replay = crate::test_support::replay_fixture();
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
}
