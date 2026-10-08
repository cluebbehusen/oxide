//! Bounded, observational timing for the player-facing HUD.

use crate::config::PerformanceDisplay;
use std::time::{Duration, Instant};

pub(crate) const BUCKETS: usize = 50;

#[derive(Clone, Copy, Default)]
struct Bucket {
    stamp: Option<u128>,
    frames: u32,
    interval_ms: f64,
    work_ms: f64,
    work_frames: u32,
    max_ms: f64,
}

/// Completed measurements only; no renderer or simulation dependencies.
#[derive(Clone)]
pub(crate) struct PerformanceView {
    pub(crate) mode: PerformanceDisplay,
    pub(crate) fps: Option<f64>,
    pub(crate) frame_ms: Option<f64>,
    pub(crate) work_ms: Option<f64>,
    pub(crate) max_ms: Option<f64>,
    /// Oldest to newest; an empty bucket is a gap, not a zero-ms frame.
    pub(crate) graph: [Option<f64>; BUCKETS],
}

impl Default for PerformanceView {
    fn default() -> Self {
        Self {
            mode: PerformanceDisplay::Off,
            fps: None,
            frame_ms: None,
            work_ms: None,
            max_ms: None,
            graph: [None; BUCKETS],
        }
    }
}

/// One collector per shell, never connected to debug capture controls.
pub(crate) struct Performance {
    context: u8,
    epoch: Option<Instant>,
    previous_start: Option<Instant>,
    previous_work: Option<f64>,
    buckets: [Bucket; BUCKETS],
    last_refresh: Option<Duration>,
    view: PerformanceView,
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            context: 0,
            epoch: None,
            previous_start: None,
            previous_work: None,
            buckets: [Bucket::default(); BUCKETS],
            last_refresh: None,
            view: PerformanceView::default(),
        }
    }
}

impl Performance {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn view(&self) -> &PerformanceView {
        &self.view
    }

    /// Called after debug requests, at the same boundary as CPU profiling.
    /// Context zero denotes a hidden screen. Off never reads the clock.
    pub(crate) fn begin(&mut self, mode: PerformanceDisplay, context: u8) -> Option<Instant> {
        self.configure(mode, context);
        if self.view.mode == PerformanceDisplay::Off {
            return None;
        }
        let now = Instant::now();
        let epoch = *self.epoch.get_or_insert(now);
        if let Some(previous) = self.previous_start.replace(now) {
            let previous_work = self.previous_work.take();
            self.record(
                now.duration_since(epoch),
                now.duration_since(previous).as_secs_f64() * 1000.0,
                previous_work,
            );
        }
        Some(now)
    }

    pub(crate) fn finish(&mut self, started: Option<Instant>) {
        if self.view.mode == PerformanceDisplay::Detailed {
            self.previous_work = started.map(|start| start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn configure(&mut self, mode: PerformanceDisplay, context: u8) {
        let mode = if context == 0 {
            PerformanceDisplay::Off
        } else {
            mode
        };
        if mode != self.view.mode || context != self.context {
            self.reset();
            self.context = context;
            self.view.mode = mode;
        }
    }

    fn record(&mut self, elapsed: Duration, interval_ms: f64, work_ms: Option<f64>) {
        if self.view.mode == PerformanceDisplay::Off
            || !interval_ms.is_finite()
            || interval_ms <= 0.0
        {
            return;
        }
        let stamp = elapsed.as_millis() / 100;
        let bucket = &mut self.buckets[(stamp % BUCKETS as u128) as usize];
        if bucket.stamp != Some(stamp) {
            *bucket = Bucket {
                stamp: Some(stamp),
                ..Bucket::default()
            };
        }
        bucket.frames += 1;
        bucket.interval_ms += interval_ms;
        bucket.max_ms = bucket.max_ms.max(interval_ms);
        if self.view.mode == PerformanceDisplay::Detailed
            && let Some(work) = work_ms.filter(|work| work.is_finite() && *work >= 0.0)
        {
            bucket.work_ms += work;
            bucket.work_frames += 1;
        }

        self.view.graph = std::array::from_fn(|index| {
            let ago = (BUCKETS - 1 - index) as u128;
            stamp.checked_sub(ago).and_then(|wanted| {
                let bucket = self.buckets[(wanted % BUCKETS as u128) as usize];
                (bucket.stamp == Some(wanted)).then_some(bucket.max_ms)
            })
        });
        if self
            .last_refresh
            .is_some_and(|last| elapsed.saturating_sub(last) < Duration::from_millis(250))
        {
            return;
        }
        self.last_refresh = Some(elapsed);
        let mut recent = Bucket::default();
        for bucket in &self.buckets {
            if bucket
                .stamp
                .is_some_and(|then| then <= stamp && stamp - then < 10)
            {
                recent.frames += bucket.frames;
                recent.interval_ms += bucket.interval_ms;
                recent.work_ms += bucket.work_ms;
                recent.work_frames += bucket.work_frames;
            }
        }
        self.view.fps =
            (recent.frames > 0).then(|| f64::from(recent.frames) * 1000.0 / recent.interval_ms);
        self.view.frame_ms =
            (recent.frames > 0).then(|| recent.interval_ms / f64::from(recent.frames));
        self.view.work_ms =
            (recent.work_frames > 0).then(|| recent.work_ms / f64::from(recent.work_frames));
        self.view.max_ms = self.view.graph.iter().flatten().copied().reduce(f64::max);
    }
}

#[cfg(test)]
mod tests;
