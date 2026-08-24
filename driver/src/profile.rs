//! Reproducible profiling of a resumed match in the actual GPU-backed shell.

use crate::auto::{ShellGuard, build_shell_executable_for};
use crate::client::Client;
use anyhow::{Context, Result, bail, ensure};
use oxide_kit::runner::GameReplay;
use oxide_protocol::{FrameProfileView, Reply, Request, StatusView, UiView};
use oxide_sim::TICKS_PER_SECOND;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Inputs for one native-shell profiling run.
pub struct ProfileOptions<'a> {
    /// Replay or save whose prefix becomes a live resumed match.
    pub replay: &'a Path,
    /// First replay tick included in the timed window.
    pub from: u64,
    /// Tick at which the harness pauses and reports.
    pub to: u64,
    /// Live wall-clock speed multiplier.
    pub speed: f64,
    /// Debug-server port used by the temporary shell.
    pub port: u16,
    /// Build the unoptimized development profile instead of release.
    pub dev: bool,
}

/// Machine-readable result from the native GPU shell.
#[derive(Debug, Serialize)]
pub struct ProfileReport {
    /// Absolute source record path.
    pub replay: PathBuf,
    /// What the shell actually measured. The source suffix is deliberately
    /// not replayed: bots continue live from the reconstructed prefix.
    pub mode: &'static str,
    /// Human-readable data-source semantics for consumers comparing runs.
    pub continuation: &'static str,
    /// Requested first tick.
    pub requested_from: u64,
    /// Requested stopping tick.
    pub requested_to: u64,
    /// Tick at which measurement actually began.
    pub actual_from: u64,
    /// Tick observed after pausing.
    pub actual_to: u64,
    /// Requested wall-clock replay speed.
    pub speed: f64,
    /// Optimized release shell unless the caller explicitly requested dev.
    pub build_profile: &'static str,
    /// Wall time measured inside the shell from the first active Playing
    /// frame through the auto-paused final frame.
    pub elapsed_ms: f64,
    /// Simulation throughput achieved across that wall interval.
    pub achieved_ticks_per_second: f64,
    /// Ideal throughput at the requested speed.
    pub target_ticks_per_second: f64,
    /// Achieved throughput as a percentage of the requested rate.
    pub target_achievement_percent: f64,
    /// Native per-frame timings captured inside the shell.
    pub frames: FrameProfileView,
}

/// Build and profile a temporary real-window shell.
pub fn run(options: &ProfileOptions<'_>) -> Result<ProfileReport> {
    ensure!(
        options.to > options.from,
        "--to must be greater than --from"
    );
    oxide_protocol::check_speed(options.speed).map_err(anyhow::Error::msg)?;

    let replay = options
        .replay
        .canonicalize()
        .with_context(|| format!("resolving replay {}", options.replay.display()))?;
    // Validate before paying for a shell build or opening a window. The shell
    // remains the reproducer; this merely turns malformed input into a prompt
    // CLI error.
    let record = oxide_kit::load_replay(&replay)
        .with_context(|| format!("loading replay {}", replay.display()))?;
    let total = record.meta.ticks.unwrap_or_else(|| {
        record
            .commands
            .last()
            .map_or(0, |command| command.tick.saturating_add(1))
    });
    validate_resume_tick(total, options.from)?;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let executable = build_shell_executable_for(!options.dev)?;
    let prefix = TemporaryReplay::create(&root, options.port, prefix_at(record, options.from))?;
    let mut command = std::process::Command::new(executable);
    command
        .args([
            "--replay",
            prefix.path().to_string_lossy().as_ref(),
            "--debug-server",
            "--paused",
            "--profile-frames",
            "--window",
            "1280x800",
            "--port",
            &options.port.to_string(),
        ])
        .env("OXIDE_RESOURCE_ROOT", &root)
        .current_dir(&root);
    let child = command.spawn().context("spawning profiled Oxide shell")?;
    let _guard = ShellGuard::new(child);
    let mut client = connect(options.port)?;

    let actual_from = status(&mut client)?.tick;
    ensure!(
        actual_from == options.from,
        "shell stopped at tick {actual_from} while seeking to {}",
        options.from
    );
    let ui = ui(&mut client)?;
    ensure!(
        ui.mode == "playing",
        "profiled shell opened in {:?}, not the live Playing screen",
        ui.mode
    );
    expect_ok(
        client.call(Request::SetSpeed {
            multiplier: options.speed,
        })?,
        "set live speed",
    )?;
    expect_performance(
        client.call(Request::QueryPerformance { reset: true })?,
        "reset frame profile",
    )?;
    expect_ok(
        client.call(Request::BeginPerformanceWindow {
            from_tick: options.from,
            to_tick: options.to,
        })?,
        "arm exact frame profile",
    )?;

    expect_ok(client.call(Request::Resume)?, "resume live match")?;
    let started = Instant::now();
    let target_rate = f64::from(TICKS_PER_SECOND) * options.speed;
    let expected = Duration::from_secs_f64((options.to - options.from) as f64 / target_rate);
    let deadline = started + expected.saturating_mul(8) + Duration::from_secs(30);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let current = status(&mut client)?;
        if current.result.is_some() && current.tick <= options.to {
            bail!(
                "live continuation ended at tick {} at or before profile end {}; choose an earlier gameplay-only window",
                current.tick,
                options.to
            );
        }
        if current.tick == options.to && current.paused {
            break;
        }
        ensure!(
            current.tick <= options.to,
            "profiled shell overshot exact end tick {} to {}",
            options.to,
            current.tick
        );
        if Instant::now() >= deadline {
            bail!(
                "profiled shell reached only tick {} before the {:.1}s deadline",
                current.tick,
                deadline.duration_since(started).as_secs_f64()
            );
        }
    }
    let actual_to = status(&mut client)?.tick;
    let frames = expect_performance(
        client.call(Request::QueryPerformance { reset: false })?,
        "read frame profile",
    )?;
    let elapsed_secs = validate_profile_result(&frames, options.from, options.to)?;

    let achieved = actual_to.saturating_sub(actual_from) as f64 / elapsed_secs;
    Ok(ProfileReport {
        replay,
        mode: "live_resume",
        continuation: "source commands before requested_from are reconstructed; after that tick bots and the live shell continue normally, so the source replay suffix is not replayed",
        requested_from: options.from,
        requested_to: options.to,
        actual_from,
        actual_to,
        speed: options.speed,
        build_profile: if options.dev { "dev" } else { "release" },
        elapsed_ms: elapsed_secs * 1000.0,
        achieved_ticks_per_second: achieved,
        target_ticks_per_second: target_rate,
        target_achievement_percent: achieved / target_rate * 100.0,
        frames,
    })
}

fn validate_profile_result(frames: &FrameProfileView, from: u64, to: u64) -> Result<f64> {
    ensure!(
        frames.renderer == "gpu",
        "shell reported a non-GPU renderer"
    );
    ensure!(frames.frames > 0, "shell returned an empty frame profile");
    let window = frames
        .window
        .as_ref()
        .context("shell omitted exact-window metadata")?;
    ensure!(
        window.complete,
        "shell did not complete its exact profile window"
    );
    ensure!(
        !window.truncated,
        "profile exceeded the shell's bounded sample retention"
    );
    ensure!(
        window.from_tick == from && window.to_tick == to,
        "shell reported the wrong exact profile window"
    );
    ensure!(
        frames.tick_start == from && frames.tick_end == to && frames.ticks_presented == to - from,
        "profile samples did not cover exactly {}..{} (got {}..{}, {} ticks)",
        from,
        to,
        frames.tick_start,
        frames.tick_end,
        frames.ticks_presented
    );
    ensure!(
        frames
            .slowest
            .as_ref()
            .is_none_or(|frame| frame.mode == "playing"),
        "profile included a non-Playing frame"
    );

    let elapsed_secs = window.elapsed_ms / 1000.0;
    ensure!(elapsed_secs > 0.0, "shell reported a zero-duration profile");
    Ok(elapsed_secs)
}

fn prefix_at(mut replay: GameReplay, tick: u64) -> GameReplay {
    replay.commands.retain(|command| command.tick < tick);
    replay.meta.ticks = Some(tick);
    replay.meta.kind = Some("save".to_string());
    replay
}

fn validate_resume_tick(total: u64, from: u64) -> Result<()> {
    ensure!(
        from <= total,
        "requested resume tick {from} exceeds source record length {total}"
    );
    Ok(())
}

struct TemporaryReplay {
    path: PathBuf,
}

impl TemporaryReplay {
    fn create(root: &Path, port: u16, replay: GameReplay) -> Result<Self> {
        let directory = root.join("target/oxide-profile");
        std::fs::create_dir_all(&directory).context("creating profile scratch directory")?;
        let path = directory.join(format!(
            "live-prefix-{}-{port}-{}.json",
            std::process::id(),
            replay.meta.ticks.unwrap_or(0)
        ));
        replay
            .save(&path)
            .with_context(|| format!("writing temporary replay prefix {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryReplay {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

fn connect(port: u16) -> Result<Client> {
    let address = format!("127.0.0.1:{port}");
    for _ in 0..240 {
        if let Ok(client) = Client::connect(&address) {
            return Ok(client);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("profiled shell never came up on {address}")
}

fn status(client: &mut Client) -> Result<StatusView> {
    match client.call(Request::Status)? {
        Reply::Status(status) => Ok(status),
        other => bail!("expected status reply, got {other:?}"),
    }
}

fn ui(client: &mut Client) -> Result<UiView> {
    match client.call(Request::QueryUi)? {
        Reply::Ui(ui) => Ok(ui),
        other => bail!("expected ui reply, got {other:?}"),
    }
}

fn expect_ok(reply: Reply, operation: &str) -> Result<()> {
    if matches!(reply, Reply::Ok) {
        Ok(())
    } else {
        bail!("{operation}: expected ok reply, got {reply:?}")
    }
}

fn expect_performance(reply: Reply, operation: &str) -> Result<FrameProfileView> {
    match reply {
        Reply::Performance(profile) => Ok(profile),
        other => bail!("{operation}: expected performance reply, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
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

        let mut replay = GameReplay::new(SIM_VERSION, Scenario::skirmish());
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
        assert!(
            profile_error(&profile).starts_with("profile samples did not cover exactly 10..20")
        );

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
}
