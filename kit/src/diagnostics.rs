//! Optional bounded timing and independent progress monitoring. Never replay input.

use crate::recovery::RecoveryWriter;
use oxide_sim::bot::observer::{BotPhase, PhaseObserver};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

const SLOTS: usize = 33;
const DEPTH: usize = 16;
const MAX_EVENTS: usize = 32768;
/// Coarse shell operations, distinct from actual GPU execution time.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Phase {
    /// Hardware or injected input and debug request handling.
    Input = 10,
    /// Ordered collection of all bot-seat command batches.
    Bots,
    /// The authoritative state transition.
    Simulation,
    /// Presentation and statistics updates after a completed tick.
    Presentation,
    /// Screen update, drawing and audio preparation.
    Screen,
    /// Waiting for the next frame; may include OS or presentation delay.
    FrameWait,
    /// Replay loading and controller reconstruction.
    ReplayLoad,
    /// Ordinary explicit save/leave persistence.
    Save,
    /// Entire native frame CPU work before presentation handoff.
    Frame,
}
#[derive(Default)]
struct Slot {
    phases: [AtomicU8; DEPTH],
    children: [AtomicU64; DEPTH],
    depth: AtomicUsize,
    tick: AtomicU64,
    progress: AtomicU64,
}
#[derive(Serialize, Clone)]
struct Timing {
    end_us: u64,
    duration_us: u64,
    exclusive_us: u64,
    slot: usize,
    phase: u8,
    tick: u64,
}
#[derive(Clone, Serialize)]
struct Progress {
    slot: usize,
    tick: u64,
    phases: Vec<u8>,
    idle_ms: u64,
}

struct PanicRecord {
    file: [u8; 256],
    message: [u8; 512],
    line: u32,
    column: u32,
}
impl PanicRecord {
    fn capture(info: &std::panic::PanicHookInfo<'_>) -> Self {
        fn copy<const N: usize>(value: &str) -> [u8; N] {
            let mut bytes = [0; N];
            let length = value.len().min(N);
            bytes[..length].copy_from_slice(&value.as_bytes()[..length]);
            bytes
        }
        let location = info.location();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-text panic payload");
        Self {
            file: copy(location.map_or("unknown", |value| value.file())),
            message: copy(message),
            line: location.map_or(0, |value| value.line()),
            column: location.map_or(0, |value| value.column()),
        }
    }
    fn json(&self) -> serde_json::Value {
        fn text(bytes: &[u8]) -> String {
            String::from_utf8_lossy(
                &bytes[..bytes
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(bytes.len())],
            )
            .into_owned()
        }
        serde_json::json!({"file":text(&self.file),"message":text(&self.message),"line":self.line,"column":self.column})
    }
}

struct Inner {
    start: Instant,
    slots: [Slot; SLOTS],
    enabled: AtomicBool,
    stop: AtomicBool,
    dropped: AtomicU64,
    frame_tick: AtomicU64,
    last_frame: AtomicU64,
    capture_started: AtomicU64,
    units: AtomicUsize,
    buildings: AtomicUsize,
    mode: AtomicU8,
    paused: AtomicBool,
    minimized: AtomicU8,
    speed_bits: AtomicU64,
    width: AtomicU64,
    height: AtomicU64,
    dpi: AtomicU64,
    recording: Arc<RecoveryWriter>,
    sender: SyncSender<Timing>,
    panic: SyncSender<PanicRecord>,
}
impl Inner {
    fn micros(&self) -> u64 {
        self.start.elapsed().as_micros().min(u64::MAX as u128) as u64
    }
    fn begin(&self, slot: usize, phase: u8, tick: u64) {
        let Some(slot) = self.slots.get(slot) else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let depth = slot.depth.load(Ordering::Relaxed);
        if depth < DEPTH {
            slot.phases[depth].store(phase, Ordering::Relaxed);
            slot.children[depth].store(0, Ordering::Relaxed);
        }
        slot.tick.store(tick, Ordering::Relaxed);
        slot.progress.store(self.micros(), Ordering::Relaxed);
        slot.depth.store(depth + 1, Ordering::Release);
    }
    fn end(&self, slot: usize, phase: u8, tick: u64, start: u64) {
        let Some(state) = self.slots.get(slot) else {
            return;
        };
        let end = self.micros();
        let depth = state.depth.fetch_sub(1, Ordering::AcqRel).saturating_sub(1);
        state.progress.store(end, Ordering::Release);
        let duration_us = end.saturating_sub(start);
        let children = state
            .children
            .get(depth)
            .map_or(0, |value| value.load(Ordering::Relaxed));
        if depth > 0
            && let Some(parent) = state.children.get(depth - 1)
        {
            parent.fetch_add(duration_us, Ordering::Relaxed);
        }
        if self
            .sender
            .try_send(Timing {
                end_us: end,
                duration_us,
                exclusive_us: duration_us.saturating_sub(children),
                slot,
                phase,
                tick,
            })
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn progress(&self, now: u64) -> Vec<Progress> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let depth = slot.depth.load(Ordering::Acquire).min(DEPTH);
                if depth == 0 {
                    return None;
                }
                Some(Progress {
                    slot: index,
                    tick: slot.tick.load(Ordering::Relaxed),
                    phases: slot.phases[..depth]
                        .iter()
                        .map(|phase| phase.load(Ordering::Relaxed))
                        .collect(),
                    idle_ms: now.saturating_sub(slot.progress.load(Ordering::Acquire)) / 1000,
                })
            })
            .collect()
    }
    fn context(&self) -> serde_json::Value {
        serde_json::json!({"format":1,"capture_started_us":self.capture_started.load(Ordering::Relaxed),"build":crate::recovery::BuildIdentity::default(),"diagnostics":self.enabled.load(Ordering::Acquire),"live_tick":self.frame_tick.load(Ordering::Relaxed),"units":self.units.load(Ordering::Relaxed),"buildings":self.buildings.load(Ordering::Relaxed),"screen":self.mode.load(Ordering::Relaxed),"paused":self.paused.load(Ordering::Relaxed),"reported_minimized":match self.minimized.load(Ordering::Relaxed) { 1 => Some(false), 2 => Some(true), _ => None },"speed":f64::from_bits(self.speed_bits.load(Ordering::Relaxed)),"window":[self.width.load(Ordering::Relaxed),self.height.load(Ordering::Relaxed)],"dpi":f64::from_bits(self.dpi.load(Ordering::Relaxed)),"writer":self.recording.status(),"dropped_timing_events":self.dropped.load(Ordering::Relaxed),"phase_names":{"1":"bot observation","2":"bot maintenance","3":"bot strategy","4":"bot allocation","5":"bot defense","6":"bot economy","7":"bot executive","10":"input/debug requests","11":"bot collection wall time","12":"simulation","13":"presentation/statistics","14":"screen/draw/audio","15":"presentation/OS wait","16":"replay reconstruction","17":"save","18":"frame CPU work","19":"frame start interval (not exclusive)","20":"bot seat total"},"screen_names":{"0":"other","1":"playing","2":"paused","3":"playback","4":"home","5":"settings"}})
    }
}

/// Observational display context attached to diagnostic records.
pub struct FrameContext<'a> {
    /// Current screen's stable name.
    pub mode: &'a str,
    /// Current completed tick.
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
    /// Last reported native minimize state; None means unknown.
    pub minimized: Option<bool>,
}

/// Per-match optional observer. Its writer and watchdog hold no gameplay locks.
pub struct Recorder {
    inner: Arc<Inner>,
}
impl Recorder {
    /// Start independent timing persistence and watchdog workers.
    pub fn start(recording: Arc<RecoveryWriter>) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(4096);
        let (panic, panics) = mpsc::sync_channel(4);
        let inner = Arc::new(Inner {
            start: Instant::now(),
            slots: std::array::from_fn(|_| Slot::default()),
            enabled: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
            frame_tick: AtomicU64::new(0),
            last_frame: AtomicU64::new(0),
            capture_started: AtomicU64::new(0),
            units: AtomicUsize::new(0),
            buildings: AtomicUsize::new(0),
            mode: AtomicU8::new(0),
            paused: AtomicBool::new(false),
            minimized: AtomicU8::new(0),
            speed_bits: AtomicU64::new(1f64.to_bits()),
            width: AtomicU64::new(0),
            height: AtomicU64::new(0),
            dpi: AtomicU64::new(1f64.to_bits()),
            recording,
            sender,
            panic,
        });
        let writer = inner.clone();
        std::thread::Builder::new()
            .name("oxide-diagnostic-writer".into())
            .spawn(move || write_timings(writer, receiver))?;
        let watchdog = inner.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("oxide-watchdog".into())
            .spawn(move || watch(watchdog, panics))
        {
            inner.stop.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(Self { inner })
    }
    /// Turn detailed capture on or off at a frame boundary.
    pub fn set_enabled(&self, enabled: bool) {
        let previous = self.inner.enabled.swap(enabled, Ordering::AcqRel);
        if previous != enabled {
            self.inner.last_frame.store(0, Ordering::Relaxed);
            if enabled {
                self.inner
                    .capture_started
                    .store(self.inner.micros(), Ordering::Relaxed);
            }
        }
    }
    /// Whether new operations should be observed.
    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Acquire)
    }
    /// Update immutable display context once per frame.
    pub fn frame(&self, context: FrameContext<'_>) {
        let FrameContext {
            mode,
            tick,
            units,
            buildings,
            speed,
            width,
            height,
            dpi,
            paused,
            minimized,
        } = context;
        if self.enabled() {
            let end = self.inner.micros();
            let previous = self.inner.last_frame.swap(end, Ordering::AcqRel);
            if previous > 0
                && self
                    .inner
                    .sender
                    .try_send(Timing {
                        end_us: end,
                        duration_us: end.saturating_sub(previous),
                        exclusive_us: 0,
                        slot: 0,
                        phase: 19,
                        tick,
                    })
                    .is_err()
            {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.inner.frame_tick.store(tick, Ordering::Relaxed);
        self.inner.units.store(units, Ordering::Relaxed);
        self.inner.buildings.store(buildings, Ordering::Relaxed);
        self.inner.mode.store(
            match mode {
                "playing" => 1,
                "pause_menu" | "pause" => 2,
                "playback" => 3,
                "home" => 4,
                "settings" => 5,
                _ => 0,
            },
            Ordering::Relaxed,
        );
        self.inner
            .speed_bits
            .store(speed.to_bits(), Ordering::Relaxed);
        self.inner.width.store(width as u64, Ordering::Relaxed);
        self.inner.height.store(height as u64, Ordering::Relaxed);
        self.inner.dpi.store(dpi.to_bits(), Ordering::Relaxed);
        self.inner.paused.store(paused, Ordering::Relaxed);
        self.inner.minimized.store(
            minimized.map_or(0, |value| if value { 2 } else { 1 }),
            Ordering::Relaxed,
        );
    }
    /// Publish progress within a long reconstruction without ending its phase.
    pub fn replay_progress(&self, tick: u64) {
        if self.enabled() {
            self.inner.slots[0].tick.store(tick, Ordering::Relaxed);
            self.inner.slots[0]
                .progress
                .store(self.inner.micros(), Ordering::Release);
            self.inner.frame_tick.store(tick, Ordering::Relaxed);
        }
    }
    /// Begin a main-thread operation. Dropping its guard records completion.
    pub fn span(&self, phase: Phase, tick: u64) -> Option<Span> {
        self.enabled().then(|| {
            self.inner.begin(0, phase as u8, tick);
            Span {
                inner: self.inner.clone(),
                slot: 0,
                phase: phase as u8,
                tick,
                start: self.inner.micros(),
            }
        })
    }
    /// Observe one seat locally on its existing worker, preserving ordinary scheduling.
    pub fn bot_commands(
        &self,
        state: &oxide_sim::State,
        bot: &mut oxide_sim::bot::SeatBot,
    ) -> Vec<oxide_sim::PlayerCommand> {
        if !self.enabled() {
            return bot.act(state);
        }
        let observer = BotObserver {
            inner: &self.inner,
            slot: usize::from(bot.player().0) + 1,
            tick: state.current_tick(),
            stack: RefCell::new(Vec::with_capacity(8)),
        };
        self.inner.begin(observer.slot, 20, observer.tick);
        let _scope = Span {
            inner: self.inner.clone(),
            slot: observer.slot,
            phase: 20,
            tick: observer.tick,
            start: self.inner.micros(),
        };
        bot.act_observed(state, &observer)
    }
    /// Register this recorder with a single chained, nonblocking panic hook.
    pub fn install_panic_hook(&self) {
        static TARGET: std::sync::Mutex<std::sync::Weak<Inner>> =
            std::sync::Mutex::new(std::sync::Weak::new());
        static INSTALL: std::sync::Once = std::sync::Once::new();
        if let Ok(mut target) = TARGET.lock() {
            *target = Arc::downgrade(&self.inner);
        }
        INSTALL.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if let Ok(target) = TARGET.try_lock()
                    && let Some(inner) = target.upgrade()
                    && inner.enabled.load(Ordering::Acquire)
                {
                    let _ = inner.panic.try_send(PanicRecord::capture(info));
                }
                previous(info);
            }));
        });
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
    }
}
/// A main-thread diagnostic span. It never owns gameplay data or locks.
pub struct Span {
    inner: Arc<Inner>,
    slot: usize,
    phase: u8,
    tick: u64,
    start: u64,
}
impl Drop for Span {
    fn drop(&mut self) {
        self.inner.end(self.slot, self.phase, self.tick, self.start);
    }
}
struct BotObserver<'a> {
    inner: &'a Inner,
    slot: usize,
    tick: u64,
    stack: RefCell<Vec<u64>>,
}
impl PhaseObserver for BotObserver<'_> {
    fn enter(&self, phase: BotPhase) {
        self.inner.begin(self.slot, phase as u8, self.tick);
        self.stack.borrow_mut().push(self.inner.micros());
    }
    fn exit(&self, phase: BotPhase) {
        if let Some(start) = self.stack.borrow_mut().pop() {
            self.inner.end(self.slot, phase as u8, self.tick, start);
        }
    }
}
fn write_timings(writer: Arc<Inner>, receiver: mpsc::Receiver<Timing>) {
    let mut events = VecDeque::new();
    let mut slow = VecDeque::new();
    let mut last = Instant::now();
    let mut was_enabled = true;
    let mut evicted = 0u64;
    loop {
        let first = receiver.recv_timeout(Duration::from_millis(250)).ok();
        for event in first.into_iter().chain(receiver.try_iter().take(4096)) {
            if event.duration_us >= 100_000 {
                slow.push_back(event.clone());
                if slow.len() > 256 {
                    slow.pop_front();
                }
            }
            events.push_back(event);
        }
        let now = writer.micros();
        while events.len() > MAX_EVENTS
            || events
                .front()
                .is_some_and(|event: &Timing| now.saturating_sub(event.end_us) > 60_000_000)
        {
            events.pop_front();
            evicted += 1;
        }
        let stop = writer.stop.load(Ordering::Acquire);
        let enabled = writer.enabled.load(Ordering::Acquire);
        if stop
            || (enabled && last.elapsed() >= Duration::from_secs(1))
            || (was_enabled && !enabled)
        {
            if writer.recording.status().ready {
                let data = serde_json::json!({"format":1,"at_us":now,"capture_stopped":stop,"active_spans":writer.progress(now),"events":events,"slow_operations":slow,"evicted":evicted,"dropped":writer.dropped.load(Ordering::Relaxed)});
                persist(&writer, "timings.json", &data);
                persist(&writer, "context.json", &writer.context());
            }
            last = Instant::now();
        }
        was_enabled = enabled;
        if stop {
            break;
        }
    }
}
fn watch(watchdog: Arc<Inner>, panics: mpsc::Receiver<PanicRecord>) {
    let mut incidents = VecDeque::new();
    let mut stalled = Vec::new();
    while !watchdog.stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(250));
        let progress = watchdog.progress(watchdog.micros());
        let enabled = watchdog.enabled.load(Ordering::Acquire);
        let now_stalled: Vec<_> = progress
            .iter()
            .filter(|slot| enabled && slot.idle_ms >= 5000)
            .map(|slot| slot.slot)
            .collect();
        let panic = panics.try_recv().ok();
        if now_stalled != stalled || panic.is_some() {
            let kind = if panic.is_some() {
                "panic observed"
            } else if !now_stalled.is_empty() {
                "suspected stall"
            } else if !enabled {
                "capture disabled"
            } else {
                "progress resumed"
            };
            incidents.push_back(serde_json::json!({"at_us":watchdog.micros(),"kind":kind,"panic":panic.as_ref().map(PanicRecord::json),"progress":progress,"context":watchdog.context()}));
            if incidents.len() > 32 {
                incidents.pop_front();
            }
            // This path deliberately does not take the journal's budget/writer lock.
            persist(&watchdog, "watchdog.json", &incidents);
            stalled = now_stalled;
        }
    }
}

fn persist(inner: &Inner, name: &str, value: &impl Serialize) {
    if !inner.recording.directory().join("lease").exists() {
        inner.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(value)
        && bytes.len()
            <= if name == "timings.json" {
                8 * 1024 * 1024
            } else {
                256 * 1024
            }
    {
        if chassis::fsx::write_atomic(inner.recording.directory().join(name), |writer| {
            std::io::Write::write_all(writer, &bytes)
        })
        .is_err()
        {
            inner.dropped.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        inner.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests;
