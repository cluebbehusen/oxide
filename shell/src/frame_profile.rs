//! Opt-in native-frame timing for the live GPU shell.

use oxide_protocol::{FrameProfileView, FrameProfileWindowView, SlowFrameView, TimingSummary};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_SAMPLES: usize = 65_536;

#[derive(Debug, Clone)]
struct FrameSample {
    mode: String,
    tick_start: u64,
    tick_end: u64,
    started_at: Instant,
    work_ms: f64,
    units: usize,
    buildings: usize,
}

/// Input captured at the native frame boundary.
#[derive(Clone, Copy)]
pub(crate) struct FrameObservation<'a> {
    pub(crate) mode: &'a str,
    pub(crate) active_playing: bool,
    pub(crate) tick_start: u64,
    pub(crate) tick_end: u64,
    pub(crate) work_ms: f64,
    pub(crate) units: usize,
    pub(crate) buildings: usize,
}

/// Bounded collector enabled only by `--profile-frames`.
pub(crate) struct FrameProfiler {
    enabled: bool,
    samples: VecDeque<FrameSample>,
    window: Option<ProfileWindow>,
}

struct ProfileWindow {
    from_tick: u64,
    to_tick: u64,
    next_tick: u64,
    started_at: Option<Instant>,
    elapsed_ms: f64,
    complete: bool,
    truncated: bool,
    start_barrier: bool,
}

impl FrameProfiler {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            samples: VecDeque::new(),
            window: None,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn arm(&mut self, from_tick: u64, to_tick: u64) -> Result<(), String> {
        if !self.enabled {
            return Err(
                "native frame profiling is disabled; launch the shell with --profile-frames"
                    .to_string(),
            );
        }
        if to_tick <= from_tick {
            return Err("profile window end must be greater than its start".to_string());
        }
        self.samples.clear();
        self.window = Some(ProfileWindow {
            from_tick,
            to_tick,
            next_tick: from_tick,
            started_at: None,
            elapsed_ms: 0.0,
            complete: false,
            truncated: false,
            start_barrier: true,
        });
        Ok(())
    }

    /// Consumes the one unmeasured live frame after Resume. Keeping the match
    /// from advancing on that frame prevents request dispatch and reply work
    /// from contaminating the first sample.
    pub(crate) fn take_start_barrier(&mut self) -> bool {
        let Some(window) = &mut self.window else {
            return false;
        };
        std::mem::take(&mut window.start_barrier)
    }

    pub(crate) fn stop_tick(&self) -> Option<u64> {
        self.window
            .as_ref()
            .filter(|window| !window.complete)
            .map(|window| window.to_tick)
    }

    pub(crate) fn record(&mut self, observation: FrameObservation<'_>) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let frame_started = now
            .checked_sub(Duration::from_secs_f64(
                observation.work_ms.max(0.0) / 1000.0,
            ))
            .unwrap_or(now);
        if let Some(window) = &self.window
            && (window.complete
                || window.start_barrier
                || observation.mode != "playing"
                || !observation.active_playing
                || observation.tick_start != window.next_tick
                || observation.tick_end > window.to_tick)
        {
            return;
        }
        if self.samples.len() == MAX_SAMPLES {
            self.samples.pop_front();
            if let Some(window) = &mut self.window {
                window.truncated = true;
            }
        }
        self.samples.push_back(FrameSample {
            mode: observation.mode.to_string(),
            tick_start: observation.tick_start,
            tick_end: observation.tick_end,
            started_at: frame_started,
            work_ms: observation.work_ms,
            units: observation.units,
            buildings: observation.buildings,
        });
        if let Some(window) = &mut self.window {
            window.started_at.get_or_insert(frame_started);
            window.next_tick = observation.tick_end;
            if window.next_tick == window.to_tick {
                window.complete = true;
                window.elapsed_ms = now
                    .duration_since(window.started_at.expect("set above"))
                    .as_secs_f64()
                    * 1000.0;
            }
        }
    }

    pub(crate) fn snapshot(&mut self, reset: bool) -> FrameProfileView {
        let frames = self.samples.len();
        let tick_start = self.samples.front().map_or(0, |sample| sample.tick_start);
        let tick_end = self.samples.back().map_or(0, |sample| sample.tick_end);
        let ticks_presented = self.samples.iter().fold(0u64, |total, sample| {
            total.saturating_add(sample.tick_end.saturating_sub(sample.tick_start))
        });
        let work_values: Vec<f64> = self.samples.iter().map(|sample| sample.work_ms).collect();
        let interval_values: Vec<f64> = self
            .samples
            .iter()
            .zip(self.samples.iter().skip(1))
            .map(|(previous, current)| {
                current
                    .started_at
                    .saturating_duration_since(previous.started_at)
                    .as_secs_f64()
                    * 1000.0
            })
            .collect();
        let slowest = self
            .samples
            .iter()
            .max_by(|a, b| a.work_ms.total_cmp(&b.work_ms))
            .map(|sample| SlowFrameView {
                mode: sample.mode.clone(),
                tick_start: sample.tick_start,
                tick_end: sample.tick_end,
                work_ms: sample.work_ms,
                units: sample.units,
                buildings: sample.buildings,
            });
        let view = FrameProfileView {
            renderer: "gpu".to_string(),
            frames,
            tick_start,
            tick_end,
            ticks_presented,
            work: summarize(work_values),
            interval: summarize(interval_values),
            work_over_16_7_ms: self
                .samples
                .iter()
                .filter(|sample| sample.work_ms > 1000.0 / 60.0)
                .count(),
            work_over_33_3_ms: self
                .samples
                .iter()
                .filter(|sample| sample.work_ms > 2000.0 / 60.0)
                .count(),
            slowest,
            window: self.window.as_ref().map(|window| FrameProfileWindowView {
                from_tick: window.from_tick,
                to_tick: window.to_tick,
                complete: window.complete,
                elapsed_ms: window.elapsed_ms,
                truncated: window.truncated,
            }),
        };
        if reset {
            self.samples.clear();
            self.window = None;
        }
        view
    }
}

fn summarize(mut values: Vec<f64>) -> TimingSummary {
    if values.is_empty() {
        return TimingSummary {
            mean_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            max_ms: 0.0,
        };
    }
    values.sort_by(f64::total_cmp);
    let mean_ms = values.iter().sum::<f64>() / values.len() as f64;
    let percentile = |percent: usize| {
        let index = (values.len() * percent).div_ceil(100).saturating_sub(1);
        values[index]
    };
    TimingSummary {
        mean_ms,
        p50_ms: percentile(50),
        p95_ms: percentile(95),
        p99_ms: percentile(99),
        max_ms: *values.last().expect("nonempty values"),
    }
}

#[cfg(test)]
mod tests;
