//! Always-on crash and freeze evidence for the native shell.
//!
//! A process installs one [`Monitor`]. The main thread and each bot seat
//! publish their current stage, tick and progress through atomics. A watchdog
//! thread records a stall when one stops progressing, and a panic hook records
//! the panic with a backtrace. Recent frame timing stays in memory and is
//! written only with an incident, so nothing reaches the disk while play is
//! healthy. Diagnostics are observational and never become replay input.

use crate::recovery::{BuildIdentity, RecoveryWriter};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, TryLockError,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread::ThreadId,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Incident log name, inside a recording directory or at the recovery root.
pub const INCIDENTS: &str = "incidents.json";
const STALL: Duration = Duration::from_secs(5);
const RECENT_SECONDS: u64 = 30;
const RECENT_FRAMES: usize = 60;
const MAX_INCIDENTS: usize = 16;
const MAX_LOG_BYTES: usize = 1024 * 1024;
const MAX_BACKTRACE_BYTES: usize = 32 * 1024;
const SEATS: usize = oxide_sim::scenario::MAX_PLAYERS;
const STAGES: usize = Stage::ALL.len();

/// What the main thread is doing. Time outside a named stage counts as `Frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// Main-thread work outside a named stage.
    Frame,
    /// Input polling, debug requests and screen logic.
    Input,
    /// Collecting a tick's bot commands, including waiting on bot workers.
    Bots,
    /// The authoritative state transition.
    Simulation,
    /// Presentation and statistics updates after a tick.
    Presentation,
    /// Screen updates, drawing and audio.
    Draw,
    /// Waiting for the next frame, including presentation and OS delay.
    Present,
    /// Rebuilding a live session from a replay.
    ReplayLoad,
}

impl Stage {
    const ALL: [Self; 8] = [
        Self::Frame,
        Self::Input,
        Self::Bots,
        Self::Simulation,
        Self::Presentation,
        Self::Draw,
        Self::Present,
        Self::ReplayLoad,
    ];

    fn from_index(index: u8) -> Self {
        Self::ALL
            .get(usize::from(index))
            .copied()
            .unwrap_or(Self::Frame)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::Input => "input",
            Self::Bots => "bots",
            Self::Simulation => "simulation",
            Self::Presentation => "presentation",
            Self::Draw => "draw",
            Self::Present => "present",
            Self::ReplayLoad => "replay_load",
        }
    }
}

/// Display context sampled once per frame and attached to incidents.
pub struct FrameContext<'a> {
    /// Visible screen's stable name.
    pub screen: &'static str,
    /// Visible session's current tick.
    pub tick: u64,
    /// Visible session's unit count.
    pub units: usize,
    /// Visible session's building count.
    pub buildings: usize,
    /// Requested simulation speed.
    pub speed: f64,
    /// Logical window width.
    pub width: u32,
    /// Logical window height.
    pub height: u32,
    /// Native display scale.
    pub dpi: f64,
    /// Whether the visible session is paused.
    pub paused: bool,
    /// Last reported native minimize state; `None` means unknown.
    pub minimized: Option<bool>,
    /// The visible live match's recording. Incidents go to its directory while
    /// it is open, and to the recovery root otherwise.
    pub recording: Option<&'a Arc<RecoveryWriter>>,
}

/// How a recording's session ended, from its journal and incident log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    /// The journal closed cleanly.
    Clean,
    /// A live writer still holds the recording.
    InProgress,
    /// The last incident was a panic.
    Panic,
    /// A recorded stall never resumed.
    Froze,
    /// Only the main thread's wait for its next frame never resumed: the OS
    /// suspended or hid the app, or presentation hung.
    AwaitingFrame,
    /// No clean close, panic or unresolved stall: a native crash, a kill or
    /// power loss.
    Abnormal,
}

/// Classify a recording from its journal state and incident log.
pub fn ending(directory: &Path, clean: bool, active: bool) -> Ending {
    if clean {
        return Ending::Clean;
    }
    if active {
        return Ending::InProgress;
    }
    let incidents = read_log(&directory.join(INCIDENTS))
        .map(|log| log.incidents)
        .unwrap_or_default();
    concluding(&incidents)
}

fn concluding(incidents: &[Value]) -> Ending {
    if incidents.last().is_some_and(|last| last["kind"] == "panic") {
        return Ending::Panic;
    }
    let mut open = BTreeMap::new();
    for incident in incidents {
        let thread = incident["thread"].as_str().unwrap_or_default();
        match incident["kind"].as_str() {
            Some("stall") => {
                open.insert(thread, incident["stage"].as_str());
            }
            Some("resumed") => {
                open.remove(thread);
            }
            _ => {}
        }
    }
    if open.is_empty() {
        Ending::Abnormal
    } else if open
        .iter()
        .all(|(thread, stage)| *thread == "main" && *stage == Some(Stage::Present.name()))
    {
        Ending::AwaitingFrame
    } else {
        Ending::Froze
    }
}

#[derive(Default, Serialize, Deserialize)]
struct IncidentLog {
    format: u32,
    incidents: Vec<Value>,
    dropped: u64,
}

fn read_log(path: &Path) -> Option<IncidentLog> {
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() <= MAX_LOG_BYTES * 2)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
}

/// Append to a bounded incident log, dropping the oldest entries first.
fn append(directory: &Path, incident: Value) -> std::io::Result<()> {
    let path = directory.join(INCIDENTS);
    let mut log = read_log(&path).unwrap_or_default();
    log.format = 1;
    log.incidents.push(incident);
    loop {
        while log.incidents.len() > MAX_INCIDENTS {
            log.incidents.remove(0);
            log.dropped += 1;
        }
        let bytes = serde_json::to_vec_pretty(&log).map_err(std::io::Error::other)?;
        if bytes.len() <= MAX_LOG_BYTES || log.incidents.len() <= 1 {
            return chassis::fsx::write_atomic(path, |writer| {
                std::io::Write::write_all(writer, &bytes)
            });
        }
        log.incidents.remove(0);
        log.dropped += 1;
    }
}

#[derive(Default)]
struct Main {
    stage: AtomicU8,
    tick: AtomicU64,
    /// Microseconds since start at the last stage change or progress report.
    progress: AtomicU64,
    /// Start of the current stage's running segment.
    segment: AtomicU64,
    /// Exclusive time per stage since the last frame boundary.
    spent: [AtomicU64; STAGES],
    /// Set by the first frame, so startup work is never reported as a stall.
    armed: AtomicBool,
}

#[derive(Default)]
struct Seat {
    /// Decision start in microseconds plus one; zero while idle.
    started: AtomicU64,
    tick: AtomicU64,
}

struct Second {
    second: u64,
    frames: u32,
    ticks: u64,
    longest_us: u64,
    spent: [u64; STAGES],
}

struct FrameRecord {
    at_us: u64,
    interval_us: Option<u64>,
    tick: u64,
    spent: [u64; STAGES],
}

#[derive(Default)]
struct Recent {
    context: Option<Value>,
    last_frame: Option<u64>,
    last_tick: u64,
    seconds: VecDeque<Second>,
    frames: VecDeque<FrameRecord>,
}

impl Recent {
    fn push(&mut self, now: u64, tick: u64, spent: [u64; STAGES]) {
        let interval_us = self.last_frame.map(|last| now.saturating_sub(last));
        let ticks = if self.last_frame.is_some() {
            tick.saturating_sub(self.last_tick)
        } else {
            0
        };
        self.last_frame = Some(now);
        self.last_tick = tick;
        let second = now / 1_000_000;
        if self
            .seconds
            .back()
            .is_none_or(|current| current.second != second)
        {
            self.seconds.push_back(Second {
                second,
                frames: 0,
                ticks: 0,
                longest_us: 0,
                spent: [0; STAGES],
            });
        }
        if let Some(current) = self.seconds.back_mut() {
            current.frames += 1;
            current.ticks += ticks;
            current.longest_us = current.longest_us.max(interval_us.unwrap_or(0));
            for (total, value) in current.spent.iter_mut().zip(spent) {
                *total += value;
            }
        }
        while self
            .seconds
            .front()
            .is_some_and(|oldest| oldest.second + RECENT_SECONDS <= second)
        {
            self.seconds.pop_front();
        }
        self.frames.push_back(FrameRecord {
            at_us: now,
            interval_us,
            tick,
            spent,
        });
        if self.frames.len() > RECENT_FRAMES {
            self.frames.pop_front();
        }
    }

    fn json(&self) -> Value {
        json!({
            "seconds": self.seconds.iter().map(|second| json!({
                "uptime_s": second.second,
                "frames": second.frames,
                "ticks": second.ticks,
                "longest_frame_us": second.longest_us,
                "stages_us": stage_times(&second.spent),
            })).collect::<Vec<_>>(),
            "frames": self.frames.iter().map(|frame| json!({
                "uptime_us": frame.at_us,
                "interval_us": frame.interval_us,
                "tick": frame.tick,
                "stages_us": stage_times(&frame.spent),
            })).collect::<Vec<_>>(),
        })
    }
}

fn stage_times(spent: &[u64; STAGES]) -> Value {
    Value::Object(
        Stage::ALL
            .iter()
            .zip(spent)
            .filter(|(_, value)| **value > 0)
            .map(|(stage, value)| (stage.name().to_owned(), json!(value)))
            .collect::<Map<_, _>>(),
    )
}

struct Inner {
    start: Instant,
    root: Option<PathBuf>,
    build: BuildIdentity,
    stall: Duration,
    main_thread: ThreadId,
    main: Main,
    seats: [Seat; SEATS],
    minimized: AtomicBool,
    recent: Mutex<Recent>,
    target: Mutex<Option<Arc<RecoveryWriter>>>,
    writing: Mutex<()>,
    stop: AtomicBool,
}

impl Inner {
    fn micros(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn on_main(&self) -> bool {
        std::thread::current().id() == self.main_thread
    }

    /// Credit the running segment to the current stage and start a new one.
    fn close_segment(&self, now: u64) {
        let current = usize::from(self.main.stage.load(Ordering::Relaxed));
        let segment = self.main.segment.swap(now, Ordering::Relaxed);
        if let Some(spent) = self.main.spent.get(current) {
            spent.fetch_add(now.saturating_sub(segment), Ordering::Relaxed);
        }
    }

    /// Enter `stage`, returning the stage it replaces.
    fn switch(&self, stage: Stage, tick: Option<u64>) -> Stage {
        let now = self.micros();
        self.close_segment(now);
        let previous = Stage::from_index(self.main.stage.swap(stage as u8, Ordering::Relaxed));
        if let Some(tick) = tick {
            self.main.tick.store(tick, Ordering::Relaxed);
        }
        self.main.progress.store(now, Ordering::Release);
        previous
    }

    fn frame(&self, context: FrameContext<'_>) {
        let now = self.micros();
        self.close_segment(now);
        let spent = std::array::from_fn(|index| self.main.spent[index].swap(0, Ordering::Relaxed));
        self.main.tick.store(context.tick, Ordering::Relaxed);
        self.main.progress.store(now, Ordering::Release);
        self.main.armed.store(true, Ordering::Release);
        self.minimized
            .store(context.minimized == Some(true), Ordering::Relaxed);
        if let Ok(mut target) = self.target.lock() {
            let unchanged = match (target.as_ref(), context.recording) {
                (Some(current), Some(next)) => Arc::ptr_eq(current, next),
                (current, next) => current.is_none() && next.is_none(),
            };
            if !unchanged {
                *target = context.recording.cloned();
            }
        }
        if let Ok(mut recent) = self.recent.lock() {
            recent.context = Some(json!({
                "screen": context.screen,
                "tick": context.tick,
                "units": context.units,
                "buildings": context.buildings,
                "speed": context.speed,
                "window": [context.width, context.height],
                "dpi": context.dpi,
                "paused": context.paused,
                "minimized": context.minimized,
            }));
            recent.push(now, context.tick, spent);
        }
    }

    fn heartbeats(&self, now: u64) -> (Value, Value) {
        let main = json!({
            "stage": Stage::from_index(self.main.stage.load(Ordering::Relaxed)).name(),
            "tick": self.main.tick.load(Ordering::Relaxed),
            "idle_ms": now.saturating_sub(self.main.progress.load(Ordering::Acquire)) / 1000,
        });
        let seats = self
            .seats
            .iter()
            .enumerate()
            .filter_map(|(seat, slot)| {
                let started = slot.started.load(Ordering::Acquire);
                (started > 0).then(|| {
                    json!({
                        "seat": seat,
                        "tick": slot.tick.load(Ordering::Relaxed),
                        "busy_ms": now.saturating_sub(started - 1) / 1000,
                    })
                })
            })
            .collect();
        (main, Value::Array(seats))
    }

    fn incident(&self, kind: &str, thread: &str, details: Map<String, Value>) -> Value {
        let now = self.micros();
        let (main, seats) = self.heartbeats(now);
        let mut incident = Map::new();
        incident.insert("kind".into(), json!(kind));
        incident.insert(
            "at_unix_ms".into(),
            json!(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |time| time.as_millis())
            ),
        );
        incident.insert("uptime_ms".into(), json!(now / 1000));
        incident.insert("process".into(), json!(std::process::id()));
        incident.insert("build".into(), json!(self.build));
        incident.insert("thread".into(), json!(thread));
        incident.extend(details);
        incident.insert("main".into(), main);
        incident.insert("seats".into(), seats);
        if kind != "resumed"
            && let Some(recent) = lock_briefly(&self.recent)
        {
            incident.insert(
                "context".into(),
                recent.context.clone().unwrap_or(Value::Null),
            );
            incident.insert("recent".into(), recent.json());
        }
        Value::Object(incident)
    }

    /// Write to the open recording's directory, falling back to the recovery root.
    fn record(&self, incident: Value) {
        let _writing = lock_briefly(&self.writing);
        let session = lock_briefly(&self.target)
            .and_then(|target| target.clone())
            .filter(|writer| {
                let status = writer.status();
                status.ready && !status.clean
            })
            .map(|writer| writer.directory().to_owned());
        for directory in session.into_iter().chain(self.root.clone()) {
            if std::fs::create_dir_all(&directory).is_ok()
                && append(&directory, incident.clone()).is_ok()
            {
                return;
            }
        }
    }

    fn record_panic(&self, message: &str, location: Option<String>, backtrace: String) {
        let thread = std::thread::current();
        let mut details = Map::new();
        details.insert(
            "panic".into(),
            json!({
                "message": message,
                "location": location,
                "backtrace": truncate(backtrace, MAX_BACKTRACE_BYTES),
            }),
        );
        self.record(self.incident("panic", thread.name().unwrap_or("unnamed"), details));
    }

    fn check_main(&self, now: u64, open: &mut Option<OpenStall>) {
        if !self.main.armed.load(Ordering::Acquire) {
            return;
        }
        let progress = self.main.progress.load(Ordering::Acquire);
        let stage = Stage::from_index(self.main.stage.load(Ordering::Relaxed));
        match open {
            Some(stall) if stall.progress != progress => {
                self.report(
                    "resumed",
                    "main",
                    stall,
                    progress.saturating_sub(stall.progress),
                );
                *open = None;
            }
            None if now.saturating_sub(progress) >= self.stall_us()
                // A minimized window can legitimately withhold frames.
                && !(stage == Stage::Present && self.minimized.load(Ordering::Relaxed)) =>
            {
                let stall = OpenStall {
                    progress,
                    stage: Some(stage),
                    tick: self.main.tick.load(Ordering::Relaxed),
                };
                self.report("stall", "main", &stall, now.saturating_sub(progress));
                *open = Some(stall);
            }
            _ => {}
        }
    }

    fn check_seat(&self, seat: usize, now: u64, open: &mut Option<OpenStall>) {
        let slot = &self.seats[seat];
        let started = slot.started.load(Ordering::Acquire);
        let thread = format!("bot seat {seat}");
        match open {
            Some(stall) if stall.progress != started => {
                self.report(
                    "resumed",
                    &thread,
                    stall,
                    now.saturating_sub(stall.progress - 1),
                );
                *open = None;
            }
            None if started > 0 && now.saturating_sub(started - 1) >= self.stall_us() => {
                let stall = OpenStall {
                    progress: started,
                    stage: None,
                    tick: slot.tick.load(Ordering::Relaxed),
                };
                self.report("stall", &thread, &stall, now.saturating_sub(started - 1));
                *open = Some(stall);
            }
            _ => {}
        }
    }

    fn report(&self, kind: &str, thread: &str, stall: &OpenStall, elapsed_us: u64) {
        let mut details = Map::new();
        if let Some(stage) = stall.stage {
            details.insert("stage".into(), json!(stage.name()));
        }
        details.insert("tick".into(), json!(stall.tick));
        details.insert("stalled_ms".into(), json!(elapsed_us / 1000));
        self.record(self.incident(kind, thread, details));
    }

    fn stall_us(&self) -> u64 {
        u64::try_from(self.stall.as_micros()).unwrap_or(u64::MAX)
    }
}

struct OpenStall {
    /// The progress value that stopped changing.
    progress: u64,
    stage: Option<Stage>,
    tick: u64,
}

fn watch(inner: Arc<Inner>) {
    let poll = (inner.stall / 4).min(Duration::from_millis(250));
    let mut main = None;
    let mut seats: [Option<OpenStall>; SEATS] = std::array::from_fn(|_| None);
    while !inner.stop.load(Ordering::Acquire) {
        std::thread::sleep(poll);
        let now = inner.micros();
        inner.check_main(now, &mut main);
        for (seat, open) in seats.iter_mut().enumerate() {
            inner.check_seat(seat, now, open);
        }
    }
}

/// Never block indefinitely: a panic can arrive while its own thread holds the lock.
fn lock_briefly<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(TryLockError::WouldBlock) => return None,
        }
    }
}

fn truncate(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Main-thread stage tracking, bot seat heartbeats, a stall watchdog and panic
/// capture for one process. Its watchdog never takes gameplay locks.
pub struct Monitor {
    inner: Arc<Inner>,
}

impl Monitor {
    /// Start monitoring with the calling thread as the main thread. Incidents
    /// outside an open recording go to `root`; without one they are not kept.
    pub fn start(root: Option<PathBuf>, build: BuildIdentity) -> std::io::Result<Self> {
        Self::start_with(root, build, STALL)
    }

    fn start_with(
        root: Option<PathBuf>,
        build: BuildIdentity,
        stall: Duration,
    ) -> std::io::Result<Self> {
        let inner = Arc::new(Inner {
            start: Instant::now(),
            root,
            build,
            stall,
            main_thread: std::thread::current().id(),
            main: Main::default(),
            seats: std::array::from_fn(|_| Seat::default()),
            minimized: AtomicBool::new(false),
            recent: Mutex::default(),
            target: Mutex::default(),
            writing: Mutex::default(),
            stop: AtomicBool::new(false),
        });
        let watchdog = inner.clone();
        std::thread::Builder::new()
            .name("oxide-watchdog".into())
            .spawn(move || watch(watchdog))?;
        Ok(Self { inner })
    }

    /// Make this the process's monitor and record panics through it. Only the
    /// first call succeeds; a rejected monitor is returned.
    pub fn install(self) -> Result<&'static Self, Self> {
        static HOOK: std::sync::Once = std::sync::Once::new();
        PROCESS.set(self)?;
        HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if let Some(monitor) = PROCESS.get() {
                    let message = info
                        .payload()
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("non-text panic payload");
                    let location = info.location().map(|location| {
                        format!(
                            "{}:{}:{}",
                            location.file(),
                            location.line(),
                            location.column()
                        )
                    });
                    monitor.inner.record_panic(
                        message,
                        location,
                        std::backtrace::Backtrace::force_capture().to_string(),
                    );
                }
                previous(info);
            }));
        });
        Ok(PROCESS.get().expect("installed monitor"))
    }

    /// Enter a main-thread stage until the guard drops. Other threads get `None`.
    pub fn stage(&self, stage: Stage, tick: u64) -> Option<StageGuard<'_>> {
        self.inner.on_main().then(|| StageGuard {
            inner: &self.inner,
            previous: self.inner.switch(stage, Some(tick)),
        })
    }

    /// Report progress within a long main-thread stage without leaving it.
    pub fn progress(&self, tick: u64) {
        if self.inner.on_main() {
            self.inner.main.tick.store(tick, Ordering::Relaxed);
            self.inner
                .main
                .progress
                .store(self.inner.micros(), Ordering::Release);
        }
    }

    /// Mark a bot seat's decision as running until the guard drops.
    pub fn seat(&self, seat: u8, tick: u64) -> Option<SeatGuard<'_>> {
        let slot = self.inner.seats.get(usize::from(seat))?;
        slot.tick.store(tick, Ordering::Relaxed);
        slot.started
            .store(self.inner.micros().saturating_add(1), Ordering::Release);
        Some(SeatGuard { slot })
    }

    /// Close a main-thread frame: fold its stage times into recent timing and
    /// sample display context. Other threads are ignored.
    pub fn frame(&self, context: FrameContext<'_>) {
        if self.inner.on_main() {
            self.inner.frame(context);
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
    }
}

static PROCESS: OnceLock<Monitor> = OnceLock::new();

/// Enter a stage on the installed monitor; `None` without one or off the main thread.
pub fn stage(stage: Stage, tick: u64) -> Option<StageGuard<'static>> {
    PROCESS.get()?.stage(stage, tick)
}

/// Report long-stage progress to the installed monitor.
pub fn progress(tick: u64) {
    if let Some(monitor) = PROCESS.get() {
        monitor.progress(tick);
    }
}

/// Mark a bot seat's decision on the installed monitor.
pub fn seat(seat: u8, tick: u64) -> Option<SeatGuard<'static>> {
    PROCESS.get()?.seat(seat, tick)
}

/// Close a frame on the installed monitor.
pub fn frame(context: FrameContext<'_>) {
    if let Some(monitor) = PROCESS.get() {
        monitor.frame(context);
    }
}

/// Restores the previous main-thread stage when dropped.
pub struct StageGuard<'a> {
    inner: &'a Inner,
    previous: Stage,
}

impl Drop for StageGuard<'_> {
    fn drop(&mut self) {
        self.inner.switch(self.previous, None);
    }
}

/// Marks a bot seat idle when dropped.
pub struct SeatGuard<'a> {
    slot: &'a Seat,
}

impl Drop for SeatGuard<'_> {
    fn drop(&mut self) {
        self.slot.started.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
