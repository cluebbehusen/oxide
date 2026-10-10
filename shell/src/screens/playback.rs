//! Read-only replay viewing. The engine owns simulation state; presentation
//! borrows it for rendering, interpolation, effects, and audio.

use crate::action::{Action, ActionEvent, ActionResolver, BindingMap, Context as InputContext};
use crate::camera::controls::{MinimapPoint, ViewerHands, event_point, held_pan, pan_toward};
use crate::frame_time::FrameTime;
use crate::game::{self, GameReplay, Presentation, Scene};
use crate::numeric;
use crate::numeric::Fit;
use crate::press::{Fed, Press};
use crate::render::prim::{fill_rect, stroke_rect};
use crate::{render, theme};
use anyhow::{Context, Result};
use macroquad::prelude::*;
#[cfg(test)]
use oxide_protocol::Key;
use oxide_protocol::{MouseButton, RawEvent};
use oxide_sim::SIM_VERSION;

/// Replay engine, presentation, and viewer transport controls.
pub struct PlaybackSession {
    pub engine: oxide_kit::playback::Playback,
    pub presentation: Presentation,
    /// Whether replay time runs, how fast, and its tick debt.
    pub clock: game::Clock,
    resolver: ActionResolver,
    /// A held left press is dragging the timeline, seeking as it moves.
    pub scrubbing: bool,
    /// A seek in flight: the target tick, chipped away a budget per
    /// frame so the render thread never freezes on a long jump.
    pub seeking: Option<u64>,
    /// The corner transport buttons' press.
    buttons: Press<Transport>,
    /// The finger scrubbing the timeline, if one landed on the bar.
    scrub_finger: Option<u64>,
    /// Middle-drag, wheel, minimap and finger camera control.
    hands: ViewerHands,
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
        Scene::new(
            &self.engine.state,
            &self.replay.setup,
            &[],
            &self.presentation,
            &self.clock,
        )
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
            oxide_sim::PlayerId(vantage.fit::<u8>()),
            render::viewport(),
        );
        // Spectator view: fog-free, but without the developer overlay, so
        // playback looks like the game.
        presentation.spectate = true;
        Ok(Self {
            engine,
            presentation,
            clock: game::Clock::default(),
            resolver: ActionResolver::default(),
            scrubbing: false,
            seeking: None,
            buttons: Press::default(),
            scrub_finger: None,
            hands: ViewerHands::default(),
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
            let every = ((self.engine.total() - self.engine.start()) / 240).max(50);
            self.stats = oxide_kit::stats::compute(&self.replay, every).ok();
        }
    }

    fn sync_render_clock(&mut self) {
        self.presentation
            .set_tick_fraction(self.clock.tick_fraction());
    }

    fn reset_clock_debt(&mut self) {
        self.clock.accum = 0.0;
        self.sync_render_clock();
    }
}

/// The viewer's corner buttons: the only way out, and the only
/// transport, for a player without keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Back,
    PlayPause,
}

fn transport_buttons(s: f32) -> [(macroquad::prelude::Rect, Transport); 2] {
    [
        (crate::button::corner_slot(0, s), Transport::Back),
        (crate::button::corner_slot(1, s), Transport::PlayPause),
    ]
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

pub fn playback_hud(pb: &PlaybackSession, bindings: &BindingMap, viewport: Vec2, mouse: Vec2) {
    let s = render::ui_scale();
    let size = crate::theme::Type::Label.at(s);
    for (rect, button) in transport_buttons(s) {
        let label = match button {
            Transport::Back => "BACK",
            Transport::PlayPause if pb.clock.paused => "PLAY",
            Transport::PlayPause => "PAUSE",
        };
        crate::button::draw(rect, label, rect.contains(mouse), s);
    }
    // The timeline: played track, live position, and the ghost of a
    // seek in flight.
    let bar = scrub_rect(&pb.view(), viewport);
    fill_rect(bar, theme::SURFACE_CAPTION);
    let total = (pb.engine.total() - pb.engine.start()).max(1) as f32;
    let frac = (pb.engine.position() - pb.engine.start()) as f32 / total;
    draw_rectangle(
        bar.x,
        bar.y,
        bar.w * frac,
        bar.h,
        Color::new(0.55, 0.55, 0.62, 0.9),
    );
    if let Some(target) = pb.seeking {
        let tfrac = target.saturating_sub(pb.engine.start()) as f32 / total;
        draw_rectangle(
            bar.x + bar.w * tfrac - 1.5 * s,
            bar.y - 2.0 * s,
            3.0 * s,
            bar.h + 4.0 * s,
            theme::TEXT_ALERT,
        );
    }
    stroke_rect(bar, theme::Stroke::Edge.at(s), theme::EDGE_CHIP);
    if pb.show_stats
        && let Some(stats) = &pb.stats
    {
        composition_band(pb, stats, bar, s);
    }
    if let Some(target) = pb.seeking {
        // Mid-seek the transport numbers would show intermediate state;
        // show seek progress instead.
        let line = format!("SEEKING  {} / {target}", pb.engine.position());
        let width = measure_text(&line, None, numeric::font_size(size), 1.0).width;
        let x = (screen_width() - width) * 0.5;
        let y = screen_height() - 14.0 * s;
        draw_rectangle(
            x - 10.0 * s,
            y - size,
            width + 20.0 * s,
            size + 10.0 * s,
            theme::SURFACE_CAPTION,
        );
        draw_text(&line, x, y, size, crate::theme::TEXT_PRIMARY);
        return;
    }
    let full = format!(
        "PLAYBACK {} / {} | {}x{} | {} pause | {}/{} seek | {} stats | {} leave",
        pb.engine.position(),
        pb.engine.total(),
        pb.clock.speed,
        if pb.clock.paused { " PAUSED" } else { "" },
        bindings.label(Action::ReplayPause),
        bindings.label(Action::ReplayBack),
        bindings.label(Action::ReplayForward),
        bindings.label(Action::ReplayStats),
        bindings.label(Action::Back),
    );
    // A 640px window cannot seat the controls hint; the transport
    // numbers alone must never run off both edges. A touch-only build
    // has no keys to hint at.
    let line = if crate::platform::TOUCH_ONLY
        || !crate::hints::showing()
        || measure_text(&full, None, numeric::font_size(size), 1.0).width
            > screen_width() - 16.0 * s
    {
        format!(
            "PLAYBACK  {} / {}  |  {}x{}",
            pb.engine.position(),
            pb.engine.total(),
            pb.clock.speed,
            if pb.clock.paused { "  |  PAUSED" } else { "" },
        )
    } else {
        full
    };
    let width = measure_text(&line, None, numeric::font_size(size), 1.0).width;
    let x = (screen_width() - width) * 0.5;
    let y = screen_height() - 14.0 * s;
    draw_rectangle(
        x - 10.0 * s,
        y - size,
        width + 20.0 * s,
        size + 10.0 * s,
        theme::SURFACE_CAPTION,
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

fn composition_interval(ticks: &[u64], column: usize) -> (f32, f32) {
    let start = ticks[0];
    let end = ticks[ticks.len() - 1];
    if start == end {
        return (0.0, 1.0);
    }
    let next = ticks.get(column + 1).copied().unwrap_or(end);
    let duration = (end - start) as f32;
    (
        (ticks[column] - start) as f32 / duration,
        (next - start) as f32 / duration,
    )
}

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
    fill_rect(band, theme::SURFACE_CAPTION);
    for (column, counts) in pooled.iter().enumerate() {
        let (left, right) = composition_interval(&stats.sample_ticks, column);
        let column_w = band.w * (right - left);
        let total: u32 = counts.values().sum();
        if total == 0 || column_w <= 0.0 {
            continue;
        }
        let x = band.x + band.w * left;
        let mut y = band.y + band.h;
        let mut share = |count: u32, color: Color| {
            let h = band.h * count as f32 / total as f32;
            y -= h;
            draw_rectangle(x, y, column_w, h, color);
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
    let frac = (pb.engine.position() - pb.engine.start()) as f32
        / (pb.engine.total() - pb.engine.start()).max(1) as f32;
    draw_rectangle(
        band.x + band.w * frac - 1.0 * s,
        band.y,
        2.0 * s,
        band.h,
        theme::EDGE_FOCUS,
    );
    // Legend across the top edge.
    let mut x = band.x + 4.0 * s;
    let size = crate::theme::Type::Small.at(s);
    for (index, kind) in named.iter().enumerate() {
        let label = format!("{kind} ");
        draw_text(&label, x, band.y - 4.0 * s, size, BAND_COLORS[index]);
        x += measure_text(&label, None, numeric::font_size(size), 1.0).width + 6.0 * s;
    }
    stroke_rect(band, theme::Stroke::Edge.at(s), theme::EDGE_CHIP);
}

impl PlaybackSession {
    /// The tick a scrub-bar x position means.
    fn tick_at(&self, bar: macroquad::prelude::Rect, x: f32) -> u64 {
        let frac = ((x - bar.x) / bar.w).clamp(0.0, 1.0);
        self.engine.start()
            + numeric::to_u64((frac * (self.engine.total() - self.engine.start()) as f32).round())
    }

    /// Applies transport input without advancing replay time.
    /// Returns true when the viewer should close.
    pub fn apply_input(
        &mut self,
        bindings: &BindingMap,
        events: &[RawEvent],
        dt: f32,
        viewport: Vec2,
        prefs: crate::config::CameraPrefs,
        mouse: &mut Vec2,
    ) -> bool {
        let mut seek_to: Option<u64> = None;
        let mut leave = false;
        let ui = render::ui_scale();
        let buttons = transport_buttons(ui);
        for e in events {
            match e {
                RawEvent::KeyDown { key } => {
                    if let Some(ActionEvent::Pressed(action)) =
                        self.resolver
                            .key_edge_in(bindings, *key, true, InputContext::Playback)
                    {
                        match action {
                            Action::Back => leave = true,
                            Action::ReplayPause => self.clock.paused = !self.clock.paused,
                            Action::ReplayBack => {
                                seek_to = Some(self.engine.position().saturating_sub(500));
                            }
                            Action::ReplayForward => seek_to = Some(self.engine.position() + 500),
                            Action::ReplayStart => seek_to = Some(self.engine.start()),
                            Action::ReplayEnd => seek_to = Some(self.engine.total()),
                            Action::ReplaySpeed(n) => {
                                self.clock.speed = 0.5 * 2_f64.powi(i32::from(n));
                            }
                            Action::ReplayStats => self.toggle_stats(),
                            _ => {}
                        }
                    }
                }
                RawEvent::KeyUp { key } => {
                    self.resolver
                        .key_edge_in(bindings, *key, false, InputContext::Playback);
                }
                _ => {
                    // The corner buttons see the pointer first; the
                    // timeline, minimap, and battlefield get the rest.
                    let zone_at = |p: Vec2, _| {
                        buttons
                            .iter()
                            .find(|(rect, _)| rect.contains(p))
                            .map(|(_, button)| *button)
                    };
                    match self.buttons.feed(e, zone_at) {
                        Fed::Activated(Transport::Back) => leave = true,
                        Fed::Activated(Transport::PlayPause) => {
                            self.clock.paused = !self.clock.paused;
                        }
                        Fed::Held => {}
                        Fed::Ignored => {
                            self.apply_pointer(e, viewport, ui, prefs, mouse, &mut seek_to);
                        }
                    }
                }
            }
        }
        if leave {
            return true;
        }
        pan_toward(
            &mut self.presentation.camera,
            held_pan(&self.resolver),
            prefs,
            dt,
        );
        if let Some(target) = seek_to {
            // A fresh transport command replaces any seek in flight.
            self.seeking = Some(target.clamp(self.engine.start(), self.engine.total()));
            self.clock.accum = 0.0;
        }
        false
    }

    /// One pointer event on the timeline, minimap, or battlefield. A
    /// finger or press that lands on the timeline scrubs; everything else
    /// is the camera's.
    fn apply_pointer(
        &mut self,
        e: &RawEvent,
        viewport: Vec2,
        ui: f32,
        prefs: crate::config::CameraPrefs,
        mouse: &mut Vec2,
        seek_to: &mut Option<u64>,
    ) {
        let bar = scrub_rect(&self.view(), viewport);
        match *e {
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            } if bar.contains(vec2(x, y)) => {
                *mouse = vec2(x, y);
                self.scrubbing = true;
                *seek_to = Some(self.tick_at(bar, x));
                return;
            }
            RawEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.scrubbing = false,
            RawEvent::TouchDown { id, .. } if self.scrub_finger == Some(id) => {
                // A platform's repeat report of the scrubbing finger.
                return;
            }
            RawEvent::TouchDown { id, x, y }
                if self.scrub_finger.is_none()
                    && crate::layout::touch_pad(bar, ui).contains(vec2(x, y)) =>
            {
                self.scrub_finger = Some(id);
                *seek_to = Some(self.tick_at(bar, x));
                return;
            }
            RawEvent::TouchMove { id, x, .. } if self.scrub_finger == Some(id) => {
                *seek_to = Some(self.tick_at(bar, x));
                return;
            }
            RawEvent::TouchUp { id, .. } | RawEvent::TouchCancel { id }
                if self.scrub_finger == Some(id) =>
            {
                self.scrub_finger = None;
                return;
            }
            _ => {}
        }
        let minimap = event_point(e).map_or_else(MinimapPoint::default, |p| MinimapPoint {
            under: render::minimap_world_at(&self.view(), p),
            clamped: render::minimap_world_clamped(&self.view(), p),
        });
        self.hands
            .pointer(e, minimap, &mut self.presentation.camera, mouse, prefs, ui);
        if self.scrubbing && matches!(e, RawEvent::MouseMove { .. }) {
            *seek_to = Some(self.tick_at(bar, mouse.x));
        }
    }

    /// Advances replay time and presentation after input has been handled.
    /// Replay time follows the unclamped frame time, capped by its own
    /// catch-up; effects and the camera follow the clamped time, like
    /// live play.
    pub fn advance_frame(&mut self, time: FrameTime, viewport: Vec2) {
        if let Some(target) = self.seeking {
            // Budgeted: a slice per frame keeps a long jump from hitching
            // the render thread.
            if self.engine.seek_step(target, 2_000) {
                self.seeking = None;
            }
            // Every budgeted chunk is a bulk jump. Establish the new
            // destination as both interpolation endpoints instead of
            // drawing motion from the prior timeline while seeking.
            self.presentation.reset_after_jump(&self.engine.state);
        } else if !self.clock.paused && !self.engine.at_end() {
            // One tick at a time: fog is per-tick truth, so each tick's
            // sounds are judged by that tick's sight.
            for _ in 0..self.clock.due_ticks(time.raw) {
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
        self.sync_render_clock();
        if !self.clock.paused {
            self.presentation
                .update_fx(&self.engine.state, time.presentation);
        }
        self.presentation.camera.set_viewport(viewport);
        self.presentation.camera.update(time.presentation);
    }

    #[cfg(test)]
    fn update(
        &mut self,
        bindings: &BindingMap,
        events: &[RawEvent],
        dt: f32,
        viewport: Vec2,
        prefs: crate::config::CameraPrefs,
        mouse: &mut Vec2,
    ) -> bool {
        let time = FrameTime::measure(dt);
        let leave = self.apply_input(bindings, events, time.presentation, viewport, prefs, mouse);
        if !leave {
            self.advance_frame(time, viewport);
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
            paused: self.clock.paused,
            speed: self.clock.speed,
            scenario: self.replay.setup.name.clone(),
            sim_version: SIM_VERSION,
            result: self.engine.state.result(),
            recorded_commands: 0,
        }
    }

    fn state(&self) -> &oxide_sim::State {
        &self.engine.state
    }

    fn advance(&mut self, ticks: u64) -> oxide_protocol::AdvancedView {
        // Seek, don't advance: advance collects the interval's events for
        // presentation, which over a long interval is wasted memory. The
        // reply reports what actually ran; a replay near its end advances
        // less than asked.
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
        self.clock.paused = paused;
        Ok(())
    }

    fn set_speed(&mut self, multiplier: f64) -> Result<(), String> {
        oxide_protocol::check_speed(multiplier)?;
        self.clock.speed = multiplier;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
