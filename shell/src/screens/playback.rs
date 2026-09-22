//! Read-only replay viewing. The engine owns simulation state; presentation
//! borrows it for rendering, interpolation, effects, and audio.

use crate::action::{Action, ActionEvent, ActionResolver, BindingMap, Context as InputContext};
use crate::game::{self, GameReplay, Presentation, Scene};
use crate::render;
use crate::render::prim::{fill_rect, stroke_rect};
use anyhow::{Context, Result};
use macroquad::prelude::*;
#[cfg(test)]
use oxide_protocol::Key;
use oxide_protocol::{MouseButton, RawEvent};
use oxide_sim::SIM_VERSION;

/// Where closing a playback viewer returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnTo {
    /// A cold launch or replay-shelf viewer returns to Home.
    Home,
    /// A viewer opened over a paused live match returns to Pause.
    Pause,
    /// A completed match's viewer returns to its report.
    Results,
}

/// Replay engine, presentation, and viewer transport controls.
pub struct PlaybackSession {
    pub engine: oxide_kit::playback::Playback,
    pub diagnostics: Option<oxide_kit::diagnostics::Recorder>,
    pub recording: Option<std::sync::Arc<oxide_kit::recovery::RecoveryWriter>>,
    diagnostics_warned: bool,
    pub presentation: Presentation,
    pub speed: f32,
    pub paused: bool,
    pub accum: f32,
    pub bindings: BindingMap,
    resolver: ActionResolver,
    middle_anchor: Option<Vec2>,
    /// A held minimap press steers the camera, like live play.
    pub minimap_drag: bool,
    /// Explicit return destination. A tick-count heuristic resurrected
    /// matches Main Menu had already discarded, while a boolean origin
    /// could not distinguish the replay shelf from a match report.
    pub return_to: ReturnTo,
    /// A seek in flight: the target tick, chipped away a budget per
    /// frame so the render thread never freezes on a long jump.
    pub seeking: Option<u64>,
    /// A held press is scrubbing the timeline.
    pub scrubbing: bool,
    /// The composition timeline overlay is showing.
    pub show_stats: bool,
    /// Match statistics, computed from the record on first toggle —
    /// one deterministic re-execution, paid only when asked for.
    pub stats: Option<oxide_kit::stats::MatchStats>,
    /// The pristine record, kept for that deferred computation.
    replay: GameReplay,
}

impl PlaybackSession {
    pub(crate) fn view(&self) -> Scene<'_> {
        Scene {
            state: &self.engine.state,
            scenario: &self.replay.setup,
            pending: &[],
            presentation: &self.presentation,
        }
    }

    pub(crate) fn configure_diagnostics(&mut self, enabled: bool, root: Option<&std::path::Path>) {
        if !enabled {
            self.diagnostics_warned = false;
        }
        if enabled && self.diagnostics.is_none() && !self.diagnostics_warned {
            let result = (|| -> Result<oxide_kit::diagnostics::Recorder> {
                if self.recording.is_none() {
                    let root = root.context("diagnostics folder unavailable")?;
                    self.recording = Some(std::sync::Arc::new(
                        oxide_kit::recovery::RecoveryWriter::start_playback(
                            root.to_owned(),
                            self.replay.clone(),
                            self.engine.total(),
                            crate::build_identity(),
                        )?,
                    ));
                }
                Ok(oxide_kit::diagnostics::Recorder::start(
                    self.recording.as_ref().unwrap().clone(),
                )?)
            })();
            match result {
                Ok(recorder) => {
                    recorder.install_panic_hook();
                    self.diagnostics = Some(recorder);
                }
                Err(error) => {
                    self.diagnostics_warned = true;
                    self.presentation
                        .toast(format!("Playback diagnostics unavailable: {error}"));
                }
            }
        }
        if let Some(recorder) = &self.diagnostics {
            recorder.set_enabled(enabled);
        }
        if !self.diagnostics_warned
            && let Some(error) = self
                .recording
                .as_ref()
                .and_then(|writer| writer.status().error)
        {
            self.diagnostics_warned = true;
            self.presentation
                .toast(format!("Playback diagnostics stopped: {error}"));
        }
    }

    pub(crate) fn finish_diagnostics(&self) {
        if let Some(writer) = &self.recording {
            crate::game::finish_recording(writer, self.engine.total());
        }
    }

    pub fn open(path: &str) -> Result<Self> {
        let replay =
            oxide_kit::load_replay(path).with_context(|| format!("loading replay {path}"))?;
        Self::from_replay(replay)
    }

    pub fn from_replay(replay: GameReplay) -> Result<Self> {
        let record = replay.clone();
        let engine = oxide_kit::playback::Playback::load(replay)?;
        // The spectator door: an all-bot record (driver benchmark,
        // bot-vs-bot spectacle) is a perfectly watchable replay.
        let vantage = record
            .setup
            .players
            .iter()
            .position(|player| !player.bot)
            .unwrap_or(0);
        let mut presentation = Presentation::new(
            &engine.state,
            oxide_sim::PlayerId(vantage as u8),
            render::viewport(),
        );
        // Spectator truth: fog-free, but NOT the developer overlay —
        // playback must look like the game, not the debugger.
        presentation.spectate = true;
        Ok(Self {
            engine,
            diagnostics: None,
            recording: None,
            diagnostics_warned: false,
            presentation,
            speed: 1.0,
            paused: false,
            accum: 0.0,
            bindings: BindingMap::classic(),
            resolver: ActionResolver::default(),
            middle_anchor: None,
            minimap_drag: false,
            return_to: ReturnTo::Home,
            seeking: None,
            scrubbing: false,
            show_stats: false,
            stats: None,
            replay: record,
        })
    }

    /// Toggles the composition overlay, computing the sampled series
    /// from the record on first use. Sampling stride targets ~240
    /// columns so the band chart stays legible at any match length.
    fn toggle_stats(&mut self) {
        self.show_stats = !self.show_stats;
        if self.show_stats && self.stats.is_none() {
            let every = (self.engine.total() / 240).max(50);
            self.stats = oxide_kit::stats::compute(&self.replay, every).ok();
        }
    }

    fn sync_render_clock(&mut self) {
        self.presentation
            .sync_external_tick_fraction(self.accum / game::TICK_DT);
    }

    fn reset_clock_debt(&mut self) {
        self.accum = 0.0;
        self.sync_render_clock();
    }
}

/// Where the scrub bar lives: a strip above the transport line,
/// stopping short of the minimap's corner. One geometry source for
/// hit-testing and drawing, like all chrome.
pub fn scrub_rect(game: &Scene<'_>, viewport: Vec2) -> macroquad::prelude::Rect {
    let s = render::ui_scale();
    let mini = render::minimap_rect(game);
    let right = (mini.x - 8.0 * s).min(viewport.x - 12.0 * s);
    let y = viewport.y - 46.0 * s;
    macroquad::prelude::Rect::new(12.0 * s, y, (right - 12.0 * s).max(60.0 * s), 10.0 * s)
}

pub fn playback_hud(pb: &PlaybackSession, viewport: Vec2) {
    let s = render::ui_scale();
    let size = 18.0 * s;
    // The timeline: played track, live position, and the ghost of a
    // seek in flight.
    let bar = scrub_rect(&pb.view(), viewport);
    fill_rect(bar, Color::from_rgba(15, 15, 18, 235));
    let total = pb.engine.total().max(1) as f32;
    let frac = pb.engine.position() as f32 / total;
    draw_rectangle(
        bar.x,
        bar.y,
        bar.w * frac,
        bar.h,
        Color::new(0.55, 0.55, 0.62, 0.9),
    );
    if let Some(target) = pb.seeking {
        let tfrac = target as f32 / total;
        draw_rectangle(
            bar.x + bar.w * tfrac - 1.5 * s,
            bar.y - 2.0 * s,
            3.0 * s,
            bar.h + 4.0 * s,
            Color::new(0.92, 0.5, 0.45, 1.0),
        );
    }
    stroke_rect(bar, 1.2 * s, Color::new(0.45, 0.45, 0.52, 0.8));
    if pb.show_stats
        && let Some(stats) = &pb.stats
    {
        composition_band(pb, stats, bar, s);
    }
    if let Some(target) = pb.seeking {
        // Mid-seek the transport numbers would lie (the state is
        // sprinting through the record); show honest progress instead.
        let line = format!("SEEKING  {} / {target}", pb.engine.position());
        let width = measure_text(&line, None, size as u16, 1.0).width;
        let x = (screen_width() - width) * 0.5;
        let y = screen_height() - 14.0 * s;
        draw_rectangle(
            x - 10.0 * s,
            y - size,
            width + 20.0 * s,
            size + 10.0 * s,
            macroquad::prelude::Color::from_rgba(15, 15, 18, 235),
        );
        draw_text(&line, x, y, size, crate::theme::TEXT_PRIMARY);
        return;
    }
    let full = format!(
        "PLAYBACK {} / {} | {}x{} | {} pause | {}/{} seek | {} stats | {} leave",
        pb.engine.position(),
        pb.engine.total(),
        pb.speed,
        if pb.paused { " PAUSED" } else { "" },
        pb.bindings.label(Action::ReplayPause),
        pb.bindings.label(Action::ReplayBack),
        pb.bindings.label(Action::ReplayForward),
        pb.bindings.label(Action::ReplayStats),
        pb.bindings.label(Action::Back),
    );
    // A 640px window cannot seat the controls hint; the transport
    // numbers alone must never run off both edges.
    let line = if measure_text(&full, None, size as u16, 1.0).width > screen_width() - 16.0 * s {
        format!(
            "PLAYBACK  {} / {}  |  {}x{}",
            pb.engine.position(),
            pb.engine.total(),
            pb.speed,
            if pb.paused { "  |  PAUSED" } else { "" },
        )
    } else {
        full
    };
    let width = measure_text(&line, None, size as u16, 1.0).width;
    let x = (screen_width() - width) * 0.5;
    let y = screen_height() - 14.0 * s;
    draw_rectangle(
        x - 10.0 * s,
        y - size,
        width + 20.0 * s,
        size + 10.0 * s,
        Color::from_rgba(15, 15, 19, 220),
    );
    draw_text(&line, x, y, size, crate::theme::TEXT_PRIMARY);
}

/// Distinct band colors; the ninth is the gray "other" fold.
const BAND_COLORS: [Color; 9] = [
    Color::new(0.85, 0.45, 0.25, 0.95),
    Color::new(0.35, 0.65, 0.75, 0.95),
    Color::new(0.75, 0.70, 0.30, 0.95),
    Color::new(0.50, 0.75, 0.40, 0.95),
    Color::new(0.70, 0.45, 0.75, 0.95),
    Color::new(0.40, 0.50, 0.85, 0.95),
    Color::new(0.85, 0.60, 0.55, 0.95),
    Color::new(0.45, 0.75, 0.65, 0.95),
    Color::new(0.55, 0.55, 0.55, 0.95),
];

/// The composition timeline: a stacked share-of-army band per sample
/// column, all seats pooled, top kinds named and the tail folded into
/// gray. Rides above the scrub bar and carries a cursor tied to the
/// transport position.
fn composition_band(
    pb: &PlaybackSession,
    stats: &oxide_kit::stats::MatchStats,
    bar: macroquad::prelude::Rect,
    s: f32,
) {
    use std::collections::BTreeMap;
    let columns = stats.sample_ticks.len();
    if columns == 0 {
        return;
    }
    // Pool seats per column, and rank kinds by their peak pooled count.
    let mut pooled: Vec<BTreeMap<&'static str, u32>> = vec![BTreeMap::new(); columns];
    let mut peak: BTreeMap<&'static str, u32> = BTreeMap::new();
    for player in &stats.players {
        for (column, counts) in player.kinds.iter().enumerate() {
            for (kind, count) in counts {
                let entry = pooled[column].entry(kind).or_default();
                *entry += u32::from(*count);
                let best = peak.entry(kind).or_default();
                *best = (*best).max(*entry);
            }
        }
    }
    let mut ranked: Vec<(&'static str, u32)> = peak.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let named: Vec<&'static str> = ranked.iter().take(8).map(|(kind, _)| *kind).collect();

    let band = macroquad::prelude::Rect::new(bar.x, bar.y - 96.0 * s, bar.w, 72.0 * s);
    fill_rect(band, Color::from_rgba(15, 15, 18, 220));
    let column_w = band.w / columns as f32;
    for (column, counts) in pooled.iter().enumerate() {
        let total: u32 = counts.values().sum();
        if total == 0 {
            continue;
        }
        let x = band.x + column_w * column as f32;
        let mut y = band.y + band.h;
        let mut share = |count: u32, color: Color| {
            let h = band.h * count as f32 / total as f32;
            y -= h;
            draw_rectangle(x, y, column_w + 0.5, h, color);
        };
        let mut other = 0u32;
        let mut named_counts: Vec<(usize, u32)> = Vec::new();
        for (kind, count) in counts {
            match named.iter().position(|n| n == kind) {
                Some(index) => named_counts.push((index, *count)),
                None => other += count,
            }
        }
        named_counts.sort_by_key(|(index, _)| *index);
        for (index, count) in named_counts {
            share(count, BAND_COLORS[index]);
        }
        if other > 0 {
            share(other, BAND_COLORS[8]);
        }
    }
    // Transport cursor.
    let frac = pb.engine.position() as f32 / pb.engine.total().max(1) as f32;
    draw_rectangle(
        band.x + band.w * frac - 1.0 * s,
        band.y,
        2.0 * s,
        band.h,
        Color::new(0.95, 0.95, 0.95, 0.9),
    );
    // Legend across the top edge.
    let mut x = band.x + 4.0 * s;
    let size = 13.0 * s;
    for (index, kind) in named.iter().enumerate() {
        let label = format!("{kind} ");
        draw_text(&label, x, band.y - 4.0 * s, size, BAND_COLORS[index]);
        x += measure_text(&label, None, size as u16, 1.0).width + 6.0 * s;
    }
    stroke_rect(band, 1.2 * s, Color::new(0.45, 0.45, 0.52, 0.8));
}

impl PlaybackSession {
    /// The tick a scrub-bar x position means.
    fn tick_at(&self, bar: macroquad::prelude::Rect, x: f32) -> u64 {
        let frac = ((x - bar.x) / bar.w).clamp(0.0, 1.0);
        (frac * self.engine.total() as f32).round() as u64
    }

    /// Applies transport input without advancing replay time.
    /// Returns true when the viewer should close.
    pub fn apply_input(
        &mut self,
        events: &[RawEvent],
        dt: f32,
        viewport: Vec2,
        zoom_inverted: bool,
        pan_speed: f32,
        mouse: &mut Vec2,
    ) -> bool {
        let mut seek_to: Option<u64> = None;
        let mut leave = false;
        for e in events {
            match e {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(*x, *y);
                    if let Some(anchor) = self.middle_anchor {
                        self.presentation
                            .camera
                            .pan((anchor - *mouse) / self.presentation.camera.zoom);
                        self.middle_anchor = Some(*mouse);
                    }
                    if self.scrubbing {
                        let bar = scrub_rect(&self.view(), viewport);
                        seek_to = Some(self.tick_at(bar, mouse.x));
                    }
                    // A held minimap press keeps steering, clamped so
                    // sliding off the edge doesn't stall the pan — same
                    // feel as live play.
                    if self.minimap_drag {
                        let rect = render::minimap_rect(&self.view());
                        let clamped = vec2(
                            x.clamp(rect.x, rect.x + rect.w - 1.0),
                            y.clamp(rect.y, rect.y + rect.h - 1.0),
                        );
                        if let Some(world) = render::minimap_world_at(&self.view(), clamped) {
                            self.presentation.camera.center = world;
                            self.presentation.camera.pan(vec2(0.0, 0.0));
                        }
                    }
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    *mouse = vec2(*x, *y);
                    let bar = scrub_rect(&self.view(), viewport);
                    if bar.contains(*mouse) {
                        self.scrubbing = true;
                        seek_to = Some(self.tick_at(bar, mouse.x));
                    } else if let Some(world) = render::minimap_world_at(&self.view(), *mouse) {
                        self.presentation.camera.center = world;
                        self.presentation.camera.pan(vec2(0.0, 0.0));
                        self.minimap_drag = true;
                    }
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    ..
                } => {
                    self.minimap_drag = false;
                    self.scrubbing = false;
                }
                RawEvent::Wheel { delta } => {
                    let delta = if zoom_inverted { -*delta } else { *delta };
                    self.presentation.camera.zoom_at(*mouse, delta);
                }
                RawEvent::KeyDown { key } => {
                    if let Some(ActionEvent::Pressed(action)) = self.resolver.key_edge_in(
                        &self.bindings,
                        *key,
                        true,
                        InputContext::Playback,
                    ) {
                        match action {
                            Action::Back => leave = true,
                            Action::ReplayPause => self.paused = !self.paused,
                            Action::ReplayBack => {
                                seek_to = Some(self.engine.position().saturating_sub(500))
                            }
                            Action::ReplayForward => seek_to = Some(self.engine.position() + 500),
                            Action::ReplayStart => seek_to = Some(0),
                            Action::ReplayEnd => seek_to = Some(self.engine.total()),
                            Action::ReplaySpeed(n) => self.speed = 0.5 * 2_f32.powi(i32::from(n)),
                            Action::ReplayStats => self.toggle_stats(),
                            _ => {}
                        }
                    }
                }
                RawEvent::KeyUp { key } => {
                    self.resolver
                        .key_edge_in(&self.bindings, *key, false, InputContext::Playback);
                }
                RawEvent::MouseDown {
                    button: MouseButton::Middle,
                    x,
                    y,
                } => self.middle_anchor = Some(vec2(*x, *y)),
                RawEvent::MouseUp {
                    button: MouseButton::Middle,
                    ..
                } => self.middle_anchor = None,
                _ => {}
            }
        }
        if leave {
            return true;
        }
        let mut dir = vec2(0.0, 0.0);
        if self.resolver.is_held(Action::PanUp) {
            dir.y -= 1.0;
        }
        if self.resolver.is_held(Action::PanDown) {
            dir.y += 1.0;
        }
        if self.resolver.is_held(Action::PanLeft) {
            dir.x -= 1.0;
        }
        if self.resolver.is_held(Action::PanRight) {
            dir.x += 1.0;
        }
        if dir != vec2(0.0, 0.0) {
            let world_per_sec = 240.0 * pan_speed / self.presentation.camera.zoom;
            self.presentation
                .camera
                .pan(dir.normalize() * world_per_sec * dt);
        }
        if let Some(target) = seek_to {
            // A fresh transport command replaces any seek in flight.
            self.seeking = Some(target);
            self.accum = 0.0;
        }
        false
    }

    /// Advances replay time and presentation after input has been handled.
    pub fn advance_frame(&mut self, dt: f32, viewport: Vec2) {
        if let Some(target) = self.seeking {
            // Budgeted: a slice per frame keeps a long first jump from
            // hitching the render thread; sim ticks run thousands per
            // second, so 2000 is comfortably under a frame.
            if self.engine.seek_step(target, 2_000) {
                self.seeking = None;
            }
            // Every budgeted chunk is a bulk jump. Establish the new
            // destination as both interpolation endpoints instead of
            // drawing motion from the prior timeline while seeking.
            self.presentation.reset_after_jump(&self.engine.state);
        } else if !self.paused && !self.engine.at_end() {
            self.accum += dt * self.speed;
            let ticks = (self.accum / game::TICK_DT) as u64;
            if ticks > 0 {
                self.accum -= ticks as f32 * game::TICK_DT;
                // One tick per present: fog is per-tick truth, and
                // batching sight checks against the final state judged
                // sounds by the wrong tick's sight. Ticks past the cap
                // are dropped debt, exactly like the live clock after a
                // hitch.
                for _ in 0..ticks.min(24) {
                    self.presentation.remember_previous_tick(&self.engine.state);
                    let events = self.engine.advance(1);
                    self.presentation.observe_tick(
                        &self.engine.state,
                        &events,
                        &self.engine.last_motion,
                    );
                    if self.engine.at_end() {
                        break;
                    }
                }
            }
        }
        self.sync_render_clock();
        self.presentation.paused = self.paused;
        self.presentation
            .update_wall_clock_fx(&self.engine.state, dt);
        self.presentation.camera.set_viewport(viewport);
        self.presentation.camera.update(dt);
    }

    #[cfg(test)]
    fn update(
        &mut self,
        events: &[RawEvent],
        dt: f32,
        viewport: Vec2,
        zoom_inverted: bool,
        pan_speed: f32,
        mouse: &mut Vec2,
    ) -> bool {
        let leave = self.apply_input(events, dt, viewport, zoom_inverted, pan_speed, mouse);
        if !leave {
            self.advance_frame(dt, viewport);
        }
        leave
    }
}

/// The viewer's half of the debug protocol's shared surface. The engine
/// owns truth, so every state-shaped answer reads `engine.state`
/// directly. The driven clock advances or seeks through recorded commands.
impl oxide_protocol::DebugSession for PlaybackSession {
    fn status(&self) -> oxide_protocol::StatusView {
        oxide_protocol::StatusView {
            tick: self.engine.state.current_tick(),
            paused: self.paused,
            speed: f64::from(self.speed),
            scenario: self.replay.setup.name.clone(),
            sim_version: SIM_VERSION.to_string(),
            result: self.engine.state.result(),
            recorded_commands: 0,
        }
    }

    fn state(&self) -> &oxide_sim::State {
        &self.engine.state
    }

    fn advance(&mut self, ticks: u64) -> oxide_protocol::AdvancedView {
        // Seek, don't advance: advance collects the interval's events
        // for presentation, and a million-tick battle's worth of them is
        // memory nobody will hear. The reply reports what actually ran —
        // a replay near its end advances less than asked.
        //
        // An external transport op replaces any UI seek in flight: left
        // pending, the stale target resumes next frame and rewinds the
        // replay this reply just reported as advanced.
        self.seeking = None;
        self.reset_clock_debt();
        let before = self.engine.position();
        self.engine.seek(before.saturating_add(ticks));
        self.presentation.reset_after_jump(&self.engine.state);
        oxide_protocol::AdvancedView {
            ticks: self.engine.position() - before,
            tick: self.engine.state.current_tick(),
            hash: oxide_protocol::hash_hex(self.engine.state.hash()),
        }
    }

    fn present(&mut self, ticks: u64) -> oxide_protocol::PresentedView {
        self.seeking = None;
        self.reset_clock_debt();
        let before = self.engine.position();
        let mut events = Vec::new();
        for _ in 0..ticks {
            if self.engine.at_end() {
                break;
            }
            // Mirror Game::present_ticks: the previous tick's transients
            // age by one sim interval, while effects emitted by the
            // newest tick stay fresh.
            self.presentation
                .update_fx(&self.engine.state, game::TICK_DT);
            self.presentation.remember_previous_tick(&self.engine.state);
            let tick_events = self.engine.advance(1);
            self.presentation.observe_tick(
                &self.engine.state,
                &tick_events,
                &self.engine.last_motion,
            );
            events.extend(tick_events);
        }
        oxide_protocol::PresentedView {
            ticks: self.engine.position() - before,
            tick: self.engine.state.current_tick(),
            hash: oxide_protocol::hash_hex(self.engine.state.hash()),
            events,
        }
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        self.paused = paused;
        self.presentation.paused = paused;
        Ok(())
    }

    fn set_speed(&mut self, multiplier: f64) -> Result<(), String> {
        oxide_protocol::check_speed(multiplier)?;
        self.speed = multiplier as f32;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;
    use macroquad::prelude::vec2;

    fn replay() -> GameReplay {
        // A short real replay: run the embedded skirmish headless and
        // record it, exactly what a save file contains.
        let scenario = oxide_sim::Scenario::skirmish();
        let outcome = oxide_kit::runner::run_scenario(&scenario, 60, true, true).expect("run");
        let mut replay = outcome.replay.expect("recorded");
        replay.meta.ticks = Some(60);
        replay
    }

    fn session() -> PlaybackSession {
        PlaybackSession::from_replay(replay()).expect("session opens")
    }

    fn long_session(ticks: u64) -> PlaybackSession {
        let mut replay = GameReplay::new(SIM_VERSION, oxide_sim::Scenario::skirmish());
        replay.meta.ticks = Some(ticks);
        PlaybackSession::from_replay(replay).expect("long session opens")
    }

    fn key(session: &mut PlaybackSession, key: Key) -> bool {
        let mut mouse = vec2(0.0, 0.0);
        let leave = session.update(
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
            0.0,
            vec2(1280.0, 800.0),
            false,
            1.0,
            &mut mouse,
        );
        // Seeks are budgeted across frames; drain any pending one so
        // asserts see the settled position.
        let mut frames = 0;
        while session.seeking.is_some() {
            session.update(&[], 0.0, vec2(1280.0, 800.0), false, 1.0, &mut mouse);
            frames += 1;
            assert!(frames < 1_000, "a pending seek must finish");
        }
        leave
    }

    #[test]
    fn an_all_bot_record_opens_for_watching() {
        // Driver benchmarks and bot-vs-bot spectacles record replays
        // with no human seat; the viewer must not demand one.
        let mut scenario = oxide_sim::Scenario::skirmish();
        for p in &mut scenario.players {
            p.bot = true;
            p.bot_config = Some(oxide_sim::scenario::BotConfig::default());
        }
        let outcome = oxide_kit::runner::run_scenario(&scenario, 60, true, true).expect("run");
        let mut replay = outcome.replay.expect("recorded");
        replay.meta.ticks = Some(60);
        let pb = PlaybackSession::from_replay(replay).expect("a spectator needs no command seat");
        assert!(pb.presentation.spectate, "the viewer stays fog-free");
    }

    #[test]
    fn transport_input_defers_replay_work_until_the_frame_advance() {
        let mut pb = session();
        let viewport = vec2(1280.0, 800.0);
        let mut mouse = Vec2::ZERO;
        assert!(!pb.apply_input(
            &[RawEvent::KeyDown { key: Key::End }],
            1.0,
            viewport,
            false,
            1.0,
            &mut mouse,
        ));
        assert_eq!(pb.engine.position(), 0);
        assert_eq!(pb.seeking, Some(60));
        pb.advance_frame(1.0, viewport);
        assert_eq!(pb.engine.position(), 60);
        assert_eq!(pb.engine.state.current_tick(), 60);

        assert!(!pb.apply_input(
            &[RawEvent::KeyDown { key: Key::Home }],
            0.0,
            viewport,
            false,
            1.0,
            &mut mouse,
        ));
        pb.advance_frame(0.0, viewport);
        assert_eq!(pb.engine.position(), 0);
        assert!(!pb.apply_input(&[], 0.1, viewport, false, 1.0, &mut mouse));
        assert_eq!(pb.engine.position(), 0);
        pb.advance_frame(0.1, viewport);
        assert_eq!(pb.engine.position(), 2);
    }

    #[test]
    fn the_transport_answers_its_keys() {
        let mut pb = session();
        assert!(!pb.paused);
        key(&mut pb, Key::Space);
        assert!(pb.paused, "space pauses");
        for (key_code, speed) in [
            (Key::Num1, 0.5),
            (Key::Num2, 1.0),
            (Key::Num3, 2.0),
            (Key::Num4, 4.0),
            (Key::Num5, 8.0),
            (Key::Num6, 16.0),
            (Key::Num7, 32.0),
            (Key::Num8, 64.0),
        ] {
            key(&mut pb, key_code);
            assert!(
                (pb.speed - speed).abs() < f32::EPSILON,
                "{key_code:?} selects {speed}x"
            );
        }
        key(&mut pb, Key::End);
        assert_eq!(pb.engine.position(), 60, "End seeks to the tail");
        key(&mut pb, Key::Home);
        assert_eq!(pb.engine.position(), 0, "Home rewinds");
        key(&mut pb, Key::PageDown);
        assert_eq!(pb.engine.position(), 60, "seeks clamp to the total");
        assert!(key(&mut pb, Key::Escape), "Escape closes the viewer");
    }

    #[test]
    fn a_scrub_press_seeks_to_the_bar_fraction_and_a_drag_retargets() {
        let mut pb = session();
        let viewport = vec2(1280.0, 800.0);
        let bar = scrub_rect(&pb.view(), viewport);
        let mut mouse = vec2(0.0, 0.0);
        pb.update(
            &[RawEvent::MouseDown {
                button: MouseButton::Left,
                x: bar.x + bar.w * 0.75,
                y: bar.y + bar.h * 0.5,
            }],
            0.0,
            viewport,
            false,
            1.0,
            &mut mouse,
        );
        assert!(pb.scrubbing, "the press grabs the timeline");
        // A 60-tick record fits one frame's budget, so the seek has
        // already landed; the position is the proof.
        let landed = pb.engine.position();
        assert!(
            (40..=50).contains(&landed),
            "three quarters of a 60-tick record is ~45, got {landed}"
        );
        // Dragging retargets before release.
        pb.update(
            &[RawEvent::MouseMove { x: bar.x, y: bar.y }],
            0.0,
            viewport,
            false,
            1.0,
            &mut mouse,
        );
        assert_eq!(pb.engine.position(), 0, "the drag walked the target home");
        pb.update(
            &[RawEvent::MouseUp {
                button: MouseButton::Left,
                x: bar.x,
                y: bar.y,
            }],
            0.0,
            viewport,
            false,
            1.0,
            &mut mouse,
        );
        assert!(!pb.scrubbing, "release lets go");
    }

    #[test]
    fn the_viewer_answers_the_shared_surface_exactly_like_a_resumed_live_session() {
        // The invariant the save-is-a-replay design rests on: a replayed
        // world and a live world resumed from the same record answer the
        // protocol identically. Both go through the one shared
        // dispatcher, so agreement here is agreement on the wire.
        use oxide_protocol::{DebugSession, Reply, Request, StateFilter, dispatch_shared};
        let scenario = oxide_sim::Scenario::skirmish();
        let outcome = oxide_kit::runner::run_scenario(&scenario, 120, true, true).expect("run");
        let mut replay = outcome.replay.expect("recorded");
        replay.meta.ticks = Some(120);
        let mut live = Game::from_replay(replay.clone()).expect("the record resumes live");
        let mut pb = PlaybackSession::from_replay(replay).expect("the record opens for watching");
        // The driven clock: the viewer seeks to the tick the resumed
        // session re-simulated to, reporting what actually ran.
        let Some(Ok(Reply::Advanced(advanced))) =
            dispatch_shared(&mut pb, &Request::AdvanceTicks { ticks: 500 })
        else {
            panic!("advance is a shared request");
        };
        assert_eq!(
            advanced.ticks, 120,
            "a replay near its end advances less than asked"
        );
        assert_eq!(advanced.tick, 120);
        for request in [
            Request::StateHash,
            Request::QueryState {
                filter: StateFilter {
                    map: true,
                    ..StateFilter::default()
                },
            },
            Request::QueryFogView {
                player: oxide_sim::PlayerId(0),
            },
        ] {
            let a = dispatch_shared(&mut live, &request).expect("shared");
            let b = dispatch_shared(&mut pb, &request).expect("shared");
            assert_eq!(a, b, "replies diverged on {request:?}");
        }
        // Status: one world, two transports. The world's fields agree;
        // pause stance, speed, and the recorder are each transport's own
        // (the viewer records nothing — it is read-only by construction).
        let live_status = DebugSession::status(&live);
        let viewer_status = DebugSession::status(&pb);
        assert_eq!(live_status.tick, viewer_status.tick);
        assert_eq!(live_status.scenario, viewer_status.scenario);
        assert_eq!(live_status.sim_version, viewer_status.sim_version);
        assert_eq!(live_status.result, viewer_status.result);
        assert_eq!(viewer_status.recorded_commands, 0);
        // The clock family answers on both, and refuses the same speeds
        // in the same words.
        for session in [&mut live as &mut dyn DebugSession, &mut pb] {
            assert_eq!(
                dispatch_shared(session, &Request::Pause),
                Some(Ok(Reply::Ok))
            );
            let refusal = dispatch_shared(session, &Request::SetSpeed { multiplier: 1000.0 })
                .expect("speed is shared")
                .expect_err("1000x is out of range");
            assert!(refusal.contains("outside 0.05..=64"));
        }
    }

    #[test]
    fn paused_time_does_not_advance_the_reproduction() {
        let mut pb = session();
        key(&mut pb, Key::Space);
        let before = pb.engine.position();
        let fx_before = pb.presentation.fx_time();
        let mut mouse = vec2(0.0, 0.0);
        pb.update(&[], 1.0, vec2(1280.0, 800.0), false, 1.0, &mut mouse);
        assert_eq!(pb.engine.position(), before, "a paused viewer holds still");
        assert_eq!(
            pb.presentation.fx_time(),
            fx_before,
            "a paused viewer holds decorative animation too"
        );
    }

    #[test]
    fn replay_fraction_drives_interpolation_and_authored_cycles() {
        let mut pb = session();
        let mut mouse = vec2(0.0, 0.0);
        let start = pb.engine.position();

        pb.update(
            &[],
            game::TICK_DT * 0.25,
            vec2(1280.0, 800.0),
            false,
            1.0,
            &mut mouse,
        );
        assert_eq!(pb.engine.position(), start);
        assert!((pb.presentation.tick_fraction() - 0.25).abs() < 1e-6);
        assert!((pb.presentation.render_alpha() - 0.25).abs() < 1e-6);

        pb.update(
            &[],
            game::TICK_DT,
            vec2(1280.0, 800.0),
            false,
            1.0,
            &mut mouse,
        );
        assert_eq!(pb.engine.position(), start + 1);
        assert!((pb.presentation.tick_fraction() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn driven_steps_discard_partial_wall_clock_debt() {
        use oxide_protocol::DebugSession;

        for present in [false, true] {
            let mut pb = session();
            let mut mouse = vec2(0.0, 0.0);
            pb.update(
                &[],
                game::TICK_DT * 0.75,
                vec2(1280.0, 800.0),
                false,
                1.0,
                &mut mouse,
            );
            assert!((pb.presentation.tick_fraction() - 0.75).abs() < 1e-6);

            if present {
                DebugSession::present(&mut pb, 1);
            } else {
                DebugSession::advance(&mut pb, 1);
            }

            assert_eq!(
                pb.accum, 0.0,
                "a driven step must reset the viewer clock like a live session"
            );
            assert_eq!(pb.presentation.tick_fraction(), 0.0);
            let after_step = pb.engine.position();
            pb.update(
                &[],
                game::TICK_DT * 0.3,
                vec2(1280.0, 800.0),
                false,
                1.0,
                &mut mouse,
            );
            assert_eq!(
                pb.engine.position(),
                after_step,
                "a sub-tick frame after a driven step must not consume old debt"
            );
        }
    }

    #[test]
    fn seek_replacement_resets_interpolation_and_old_timeline_facing() {
        use oxide_protocol::DebugSession;

        let mut pb = session();
        pb.presentation
            .prev_pos
            .values_mut()
            .for_each(|position| *position += vec2(99.0, 99.0));
        pb.presentation.facing.insert(0, 1.25);

        DebugSession::advance(&mut pb, 40);

        for (&id, &angle) in &pb.presentation.facing {
            let unit = pb.engine.state.unit(oxide_sim::UnitId(id)).unwrap();
            let expected = f32::from(unit.heading) * std::f32::consts::TAU / 256.0
                + std::f32::consts::FRAC_PI_2;
            assert_eq!(angle, expected);
        }
        for unit in pb.engine.state.units() {
            let expected = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
            assert_eq!(pb.presentation.prev_pos.get(&unit.id.0), Some(&expected));
        }
    }

    #[test]
    fn long_seeks_are_budgeted_and_a_new_transport_command_replaces_them() {
        let mut pb = long_session(5_000);
        let viewport = vec2(1280.0, 800.0);
        let mut mouse = Vec2::ZERO;

        pb.update(
            &[RawEvent::KeyDown { key: Key::End }],
            0.0,
            viewport,
            false,
            1.0,
            &mut mouse,
        );
        assert_eq!(
            pb.engine.position(),
            2_000,
            "one frame consumes one seek budget"
        );
        assert_eq!(pb.seeking, Some(5_000));

        pb.update(
            &[RawEvent::KeyDown { key: Key::PageUp }],
            0.0,
            viewport,
            false,
            1.0,
            &mut mouse,
        );
        assert_eq!(pb.engine.position(), 1_500);
        assert_eq!(pb.seeking, None, "the replacement target settled");

        use oxide_protocol::DebugSession;
        pb.seeking = Some(5_000);
        pb.accum = game::TICK_DT * 0.75;
        let advanced = DebugSession::advance(&mut pb, 10);
        assert_eq!(advanced.ticks, 10);
        assert_eq!(pb.engine.position(), 1_510);
        assert_eq!(pb.seeking, None, "driven input cancels stale UI seeks");
        assert_eq!(pb.accum, 0.0, "driven input drops wall-clock debt");
    }

    #[test]
    fn playback_hitches_run_only_one_frames_tick_budget() {
        let mut pb = long_session(5_000);
        pb.speed = 64.0;
        let mut mouse = Vec2::ZERO;
        pb.update(&[], 1.0, vec2(1280.0, 800.0), false, 1.0, &mut mouse);
        assert_eq!(pb.engine.position(), 24);
        assert!(pb.accum < game::TICK_DT, "excess frame debt is dropped");
    }

    #[test]
    fn statistics_are_created_lazily_and_retained_while_hidden() {
        let mut pb = session();
        assert!(!pb.show_stats);
        assert!(pb.stats.is_none());

        key(&mut pb, Key::Tab);
        assert!(pb.show_stats);
        let final_tick = pb.stats.as_ref().expect("statistics computed").final_tick;
        assert_eq!(final_tick, pb.engine.total());

        key(&mut pb, Key::Tab);
        assert!(!pb.show_stats);
        assert_eq!(
            pb.stats.as_ref().map(|stats| stats.final_tick),
            Some(final_tick)
        );
    }
    #[test]
    fn playback_uses_rebound_camera_and_transport_keys_without_advancing_the_record() {
        use crate::action::Chord;
        let mut pb = session();
        pb.paused = true;
        pb.presentation.camera.zoom_at(vec2(640.0, 400.0), 4.0);
        pb.presentation.camera.update(1.0);
        assert!(pb.bindings.rebind(Action::PanRight, Chord::bare(Key::L)));
        assert!(pb.bindings.rebind(Action::ReplayStats, Chord::bare(Key::O)));
        let mut mouse = vec2(640.0, 400.0);
        let tick = pb.engine.position();
        let before = pb.presentation.camera.center.x;
        pb.update(
            &[
                RawEvent::KeyDown { key: Key::L },
                RawEvent::KeyDown { key: Key::Right },
            ],
            0.1,
            vec2(1280.0, 800.0),
            false,
            1.0,
            &mut mouse,
        );
        let halfway = pb.presentation.camera.center.x;
        pb.update(
            &[RawEvent::KeyUp { key: Key::L }],
            0.1,
            vec2(1280.0, 800.0),
            false,
            1.0,
            &mut mouse,
        );
        let after = pb.presentation.camera.center.x;
        assert!(before < halfway && halfway < after);
        pb.update(
            &[RawEvent::KeyUp { key: Key::Right }],
            0.1,
            vec2(1280.0, 800.0),
            false,
            1.0,
            &mut mouse,
        );
        assert_eq!(pb.presentation.camera.center.x, after);
        assert_eq!(pb.engine.position(), tick);
        key(&mut pb, Key::Tab);
        assert!(!pb.show_stats);
        key(&mut pb, Key::O);
        assert!(pb.show_stats);
    }
}
