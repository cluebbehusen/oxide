//! The decided-match report: team-grouped scoreboard, match curve, and
//! touchable next steps. It owns input instead of borrowing the pause menu,
//! so a finished match has a real destination rather than a keyboard-only
//! banner laid over gameplay.

use crate::game::{Game, SoundKind};
use crate::{render, theme};
use macroquad::prelude::*;
use oxide_protocol::{Key, MouseButton, RawEvent};
use oxide_sim::{GameResult, PlayerId, TICKS_PER_SECOND};

const ACTIONS: [&str; 3] = ["REMATCH", "WATCH REPLAY", "HOME"];

/// What a result frame decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// Stay on the report.
    Stay,
    /// Start the same authored match again.
    Rematch,
    /// Watch the completed command record.
    Watch,
    /// Return to the front door.
    Home,
}

fn out_for(index: usize) -> Out {
    match index {
        0 => Out::Rematch,
        1 => Out::Watch,
        _ => Out::Home,
    }
}

/// Touchable action geometry, injected for headless tests.
pub(crate) fn action_rects(viewport: Vec2, scale: f32) -> [Rect; 3] {
    let gap = 10.0 * scale;
    let margin = 24.0 * scale;
    let available = (viewport.x - margin * 2.0 - gap * 2.0).max(3.0);
    let width = (available / 3.0).min(210.0 * scale);
    let total = width * 3.0 + gap * 2.0;
    let x = (viewport.x - total) * 0.5;
    let y = viewport.y - 60.0 * scale;
    [
        Rect::new(x, y, width, crate::layout::MIN_TOUCH_TARGET * scale),
        Rect::new(
            x + width + gap,
            y,
            width,
            crate::layout::MIN_TOUCH_TARGET * scale,
        ),
        Rect::new(
            x + (width + gap) * 2.0,
            y,
            width,
            crate::layout::MIN_TOUCH_TARGET * scale,
        ),
    ]
}

fn action_at(point: Vec2, viewport: Vec2, scale: f32) -> Option<usize> {
    action_rects(viewport, scale)
        .iter()
        .position(|rect| rect.contains(point))
}

/// Stateful pointer/keyboard ownership for the report.
pub struct ResultsScreen {
    selected: usize,
    hover: Option<usize>,
    pressed: Option<usize>,
    pressed_touch: Option<(u64, usize)>,
}

impl ResultsScreen {
    /// Opens with Rematch selected.
    pub fn open() -> Self {
        Self {
            selected: 0,
            hover: None,
            pressed: None,
            pressed_touch: None,
        }
    }

    /// Keyboard cursor for the debug UI surface.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Pointer hover for the debug UI surface.
    pub fn hover(&self) -> Option<usize> {
        self.hover
    }

    /// Stable labels for automation and accessibility.
    pub fn items(&self) -> Vec<String> {
        ACTIONS.iter().map(|label| (*label).to_string()).collect()
    }

    /// Applies one frame through the same raw-event funnel as every menu.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        viewport: Vec2,
        scale: f32,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Out {
        for event in events {
            match *event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    self.hover = action_at(*mouse, viewport, scale);
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    self.pressed = action_at(vec2(x, y), viewport, scale);
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let released = action_at(vec2(x, y), viewport, scale);
                    let armed = self.pressed.take();
                    if let Some(index) = armed
                        && Some(index) == released
                    {
                        self.selected = index;
                        sounds.push((SoundKind::Click, None));
                        return out_for(index);
                    }
                }
                RawEvent::TouchDown { id, x, y } if self.pressed_touch.is_none() => {
                    *mouse = vec2(x, y);
                    self.hover = action_at(*mouse, viewport, scale);
                    self.pressed_touch = self.hover.map(|index| (id, index));
                }
                RawEvent::TouchMove { id, x, y }
                    if self.pressed_touch.is_some_and(|(finger, _)| finger == id) =>
                {
                    *mouse = vec2(x, y);
                    self.hover = action_at(*mouse, viewport, scale);
                }
                RawEvent::TouchUp { id, x, y }
                    if self.pressed_touch.is_some_and(|(finger, _)| finger == id) =>
                {
                    *mouse = vec2(x, y);
                    let released = action_at(*mouse, viewport, scale);
                    let (_, armed) = self.pressed_touch.take().expect("matching touch");
                    if released == Some(armed) {
                        self.selected = armed;
                        sounds.push((SoundKind::Click, None));
                        return out_for(armed);
                    }
                }
                RawEvent::KeyDown {
                    key: Key::Left | Key::Up,
                } => {
                    self.hover = None;
                    self.selected = self.selected.checked_sub(1).unwrap_or(ACTIONS.len() - 1);
                }
                RawEvent::KeyDown {
                    key: Key::Right | Key::Down,
                } => {
                    self.hover = None;
                    self.selected = (self.selected + 1) % ACTIONS.len();
                }
                RawEvent::KeyDown { key: Key::Enter } => {
                    sounds.push((SoundKind::Click, None));
                    return out_for(self.selected);
                }
                RawEvent::KeyDown { key: Key::Escape } => {
                    sounds.push((SoundKind::Click, None));
                    return Out::Home;
                }
                _ => {}
            }
        }
        Out::Stay
    }

    /// Draws the report over the rendered battlefield.
    pub fn draw(&self, game: &Game) {
        let viewport = render::viewport();
        let s = render::ui_scale();
        draw_rectangle(
            0.0,
            0.0,
            viewport.x,
            viewport.y,
            Color::new(0.025, 0.025, 0.035, 0.92),
        );
        let panel = Rect::new(
            12.0 * s,
            10.0 * s,
            viewport.x - 24.0 * s,
            viewport.y - 20.0 * s,
        );
        draw_rectangle(panel.x, panel.y, panel.w, panel.h, theme::SURFACE_MENU);
        draw_rectangle_lines(
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            1.5 * s,
            Color::new(0.65, 0.52, 0.34, 0.72),
        );

        let (title, title_color, subtitle) = verdict(game);
        let title_size = 40.0 * s;
        let dims = measure_text(title, None, title_size as u16, 1.0);
        draw_text(
            title,
            (viewport.x - dims.width) * 0.5,
            48.0 * s,
            title_size,
            title_color,
        );
        let stats = game.end_stats.as_ref();
        let duration = stats.map_or(0, |report| report.final_tick);
        let meta = format!("{subtitle}  ·  {}", format_duration(duration));
        let meta_dims = measure_text(&meta, None, (15.0 * s) as u16, 1.0);
        draw_text(
            &meta,
            (viewport.x - meta_dims.width) * 0.5,
            68.0 * s,
            15.0 * s,
            theme::TEXT_BODY,
        );

        let header_y = 91.0 * s;
        let row_h = if viewport.y <= 480.0 { 18.0 } else { 22.0 } * s;
        let left = panel.x + 20.0 * s;
        let right = panel.x + panel.w - 20.0 * s;
        let columns = [
            left,
            left + panel.w * 0.43,
            left + panel.w * 0.56,
            left + panel.w * 0.69,
            left + panel.w * 0.82,
        ];
        for (label, x) in ["PLAYER", "PEAK", "BUILT", "LOST", "COLLECTED"]
            .iter()
            .zip(columns)
        {
            draw_text(label, x, header_y, 12.0 * s, theme::TEXT_SECONDARY);
        }
        draw_line(
            left,
            header_y + 5.0 * s,
            right,
            header_y + 5.0 * s,
            1.0 * s,
            theme::TEXT_DISABLED,
        );

        if let Some(report) = stats {
            let mut seats: Vec<usize> = (0..game.state.players().len()).collect();
            seats.sort_by_key(|seat| (game.state.players()[*seat].team, *seat));
            for (row, seat) in seats.into_iter().enumerate() {
                let player = &game.state.players()[seat];
                let numbers = report
                    .players
                    .iter()
                    .find(|entry| usize::from(entry.seat) == seat);
                let y = header_y + (row as f32 + 1.35) * row_h;
                let color = render::seat_identity_color(game, PlayerId(seat as u8));
                draw_marker(columns[0], y - 5.0 * s, 4.0 * s, seat, color);
                let winner = game.state.winners().contains(&PlayerId(seat as u8));
                let crown = if winner { " *" } else { "" };
                let name = clipped_name(&player.name, if viewport.x < 800.0 { 15 } else { 24 });
                draw_text(
                    format!("T{}  {}{}", player.team + 1, name, crown),
                    columns[0] + 11.0 * s,
                    y,
                    13.0 * s,
                    color,
                );
                if let Some(numbers) = numbers {
                    let peak = numbers.army_value.iter().copied().max().unwrap_or(0);
                    let built = format!(
                        "{}u/{}b",
                        numbers.units_trained, numbers.buildings_completed
                    );
                    let lost = format!("{}u/{}b", numbers.units_lost, numbers.buildings_lost);
                    for (text, x) in [
                        (peak.to_string(), columns[1]),
                        (built, columns[2]),
                        (lost, columns[3]),
                        (numbers.scrap_collected.to_string(), columns[4]),
                    ] {
                        draw_text(&text, x, y, 13.0 * s, theme::TEXT_BODY);
                    }
                }
            }

            let graph_top = header_y + (report.players.len() as f32 + 1.8) * row_h + 5.0 * s;
            let button_top = action_rects(viewport, s)[0].y;
            let graph_bottom = (button_top - 25.0 * s).max(graph_top + 34.0 * s);
            draw_army_graph(
                game,
                report,
                Rect::new(
                    left,
                    graph_top,
                    right - left,
                    (graph_bottom - graph_top).max(34.0 * s),
                ),
                s,
            );
        } else {
            draw_text(
                "Compiling the final record...",
                left,
                header_y + 30.0 * s,
                16.0 * s,
                theme::TEXT_BODY,
            );
        }

        for (index, (label, rect)) in ACTIONS.iter().zip(action_rects(viewport, s)).enumerate() {
            let active = self.hover == Some(index) || self.selected == index;
            draw_rectangle(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if active {
                    theme::SURFACE_CARD
                } else {
                    theme::SURFACE_PANEL
                },
            );
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if active { 2.0 * s } else { 1.0 * s },
                if active {
                    theme::TEXT_ACCENT
                } else {
                    theme::TEXT_DISABLED
                },
            );
            let dims = measure_text(label, None, (14.0 * s) as u16, 1.0);
            draw_text(
                label,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + rect.h * 0.62,
                14.0 * s,
                if active {
                    theme::TEXT_PRIMARY
                } else {
                    theme::TEXT_BODY
                },
            );
        }
    }
}

fn verdict(game: &Game) -> (&'static str, Color, String) {
    let winners = game.state.winners();
    match game.state.result() {
        Some(GameResult::Victory { .. }) if winners.contains(&game.human) => (
            "VICTORY",
            theme::TEXT_ACCENT,
            format!(
                "{} take the field",
                winners
                    .into_iter()
                    .map(|seat| game.state.player(seat).name.to_uppercase())
                    .collect::<Vec<_>>()
                    .join(" & ")
            ),
        ),
        Some(GameResult::Victory { .. }) if game.state.player(game.human).resigned => (
            "SURRENDERED",
            theme::TEXT_DANGER,
            "your machines fell silent".to_string(),
        ),
        Some(GameResult::Victory { .. }) => (
            "DEFEAT",
            theme::TEXT_DANGER,
            format!(
                "{} take the field",
                winners
                    .into_iter()
                    .map(|seat| game.state.player(seat).name.to_uppercase())
                    .collect::<Vec<_>>()
                    .join(" & ")
            ),
        ),
        Some(GameResult::Draw) | None => (
            "MUTUAL DESTRUCTION",
            theme::TEXT_BODY,
            "no Foundry survived".to_string(),
        ),
    }
}

fn clipped_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let mut clipped: String = name.chars().take(max.saturating_sub(3)).collect();
    clipped.push_str("...");
    clipped
}

fn format_duration(ticks: u64) -> String {
    let seconds = ticks / u64::from(TICKS_PER_SECOND);
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn draw_marker(x: f32, y: f32, r: f32, seat: usize, color: Color) {
    match seat % 4 {
        0 => draw_circle(x, y, r, color),
        1 => draw_rectangle(x - r, y - r, r * 2.0, r * 2.0, color),
        2 => {
            for (a, b) in [
                (vec2(x, y - r), vec2(x + r, y)),
                (vec2(x + r, y), vec2(x, y + r)),
                (vec2(x, y + r), vec2(x - r, y)),
                (vec2(x - r, y), vec2(x, y - r)),
            ] {
                draw_line(a.x, a.y, b.x, b.y, 1.5, color);
            }
        }
        _ => {
            draw_line(x - r, y, x + r, y, 1.5, color);
            draw_line(x, y - r, x, y + r, 1.5, color);
        }
    }
}

fn draw_army_graph(game: &Game, report: &oxide_kit::stats::MatchStats, rect: Rect, scale: f32) {
    draw_rectangle(rect.x, rect.y, rect.w, rect.h, theme::SURFACE_PANEL);
    draw_rectangle_lines(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        1.0 * scale,
        theme::TEXT_DISABLED,
    );
    let top = report
        .players
        .iter()
        .flat_map(|player| player.army_value.iter().copied())
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    for player in &report.players {
        let color = render::seat_identity_color(game, PlayerId(player.seat));
        let n = player.army_value.len().max(2);
        let pattern = usize::from(player.seat) % 4;
        let mut previous: Option<Vec2> = None;
        for (sample, value) in player.army_value.iter().copied().enumerate() {
            let point = vec2(
                rect.x + rect.w * sample as f32 / (n - 1) as f32,
                rect.y + rect.h - rect.h * value as f32 / top,
            );
            if let Some(from) = previous
                && line_visible(sample, pattern)
            {
                draw_line(from.x, from.y, point.x, point.y, 1.8 * scale, color);
            }
            previous = Some(point);
        }
        if let Some(last) = previous {
            draw_marker(last.x, last.y, 3.0 * scale, usize::from(player.seat), color);
        }
    }
    let caption = "ARMY VALUE OVER THE MATCH";
    let dims = measure_text(caption, None, (10.0 * scale) as u16, 1.0);
    draw_text(
        caption,
        rect.x + (rect.w - dims.width) * 0.5,
        rect.y + rect.h + 13.0 * scale,
        10.0 * scale,
        theme::TEXT_SECONDARY,
    );
}

fn line_visible(segment: usize, pattern: usize) -> bool {
    match pattern {
        0 => true,
        1 => !segment.is_multiple_of(4),
        2 => segment % 4 < 2,
        _ => segment.is_multiple_of(3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key) -> RawEvent {
        RawEvent::KeyDown { key }
    }

    #[test]
    fn actions_fit_small_and_large_viewports() {
        for viewport in [vec2(640.0, 400.0), vec2(1024.0, 768.0)] {
            let rects = action_rects(viewport, 1.0);
            assert!(rects[0].x >= 0.0);
            assert!(rects[2].x + rects[2].w <= viewport.x);
            assert!(
                rects
                    .iter()
                    .all(|rect| rect.h >= crate::layout::MIN_TOUCH_TARGET)
            );
            assert!(
                rects
                    .windows(2)
                    .all(|pair| pair[0].x + pair[0].w < pair[1].x)
            );
        }
    }

    #[test]
    fn keyboard_wraps_and_escape_goes_home() {
        let mut screen = ResultsScreen::open();
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        assert_eq!(
            screen.update(
                &[key(Key::Left), key(Key::Enter)],
                &mut mouse,
                vec2(640.0, 400.0),
                1.0,
                &mut sounds,
            ),
            Out::Home
        );
        assert_eq!(screen.selected(), 2);
        assert_eq!(
            screen.update(
                &[key(Key::Escape)],
                &mut mouse,
                vec2(640.0, 400.0),
                1.0,
                &mut sounds,
            ),
            Out::Home
        );
    }

    #[test]
    fn touch_requires_release_inside_the_armed_action() {
        let viewport = vec2(640.0, 400.0);
        let rect = action_rects(viewport, 1.0)[1];
        let at = vec2(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
        let mut screen = ResultsScreen::open();
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        assert_eq!(
            screen.update(
                &[
                    RawEvent::TouchDown {
                        id: 7,
                        x: at.x,
                        y: at.y,
                    },
                    RawEvent::TouchUp {
                        id: 7,
                        x: at.x,
                        y: at.y,
                    },
                ],
                &mut mouse,
                viewport,
                1.0,
                &mut sounds,
            ),
            Out::Watch
        );
        assert_eq!(screen.selected(), 1);
    }

    #[test]
    fn duration_uses_sim_time() {
        assert_eq!(format_duration(0), "00:00");
        assert_eq!(format_duration(u64::from(TICKS_PER_SECOND) * 125), "02:05");
    }

    #[test]
    fn every_seat_line_has_a_non_color_pattern() {
        for pattern in 0..4 {
            let samples: Vec<bool> = (0..12)
                .map(|segment| line_visible(segment, pattern))
                .collect();
            assert!(samples.iter().any(|visible| *visible));
            if pattern > 0 {
                assert!(samples.iter().any(|visible| !*visible));
            }
        }
    }
}
