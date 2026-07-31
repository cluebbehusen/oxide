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

#[derive(Debug, Clone, Copy)]
struct ResultsLayout {
    title_y: f32,
    title_size: f32,
    meta_y: f32,
    meta_size: f32,
    header_y: f32,
    header_size: f32,
    rule_offset: f32,
    row_height: f32,
    row_size: f32,
    row_baseline: f32,
    marker_radius: f32,
    graph_top: f32,
    graph_bottom: f32,
}

fn results_layout(viewport: Vec2, scale: f32, player_count: usize) -> ResultsLayout {
    let logical_height = viewport.y / scale.max(f32::EPSILON);
    let compact_roster = logical_height <= 480.0 && player_count >= 6;
    let (
        title_y,
        title_size,
        meta_y,
        meta_size,
        header_y,
        header_size,
        rule_offset,
        row_height,
        row_size,
        row_baseline,
        marker_radius,
        graph_row_padding,
        graph_padding,
    ) = if compact_roster {
        (
            43.0, 32.0, 62.0, 13.0, 79.0, 11.0, 13.0, 16.0, 12.0, 0.78, 3.4, 0.45, 3.0,
        )
    } else {
        (
            48.0,
            40.0,
            68.0,
            15.0,
            91.0,
            13.0,
            16.0,
            if logical_height <= 480.0 { 19.0 } else { 23.0 },
            14.0,
            0.85,
            4.0,
            1.15,
            5.0,
        )
    };
    let graph_top = header_y
        + rule_offset
        + (player_count as f32 + graph_row_padding) * row_height
        + graph_padding;
    let graph_bottom = action_rects(viewport, scale)[0].y / scale - 25.0;

    ResultsLayout {
        title_y: title_y * scale,
        title_size: title_size * scale,
        meta_y: meta_y * scale,
        meta_size: meta_size * scale,
        header_y: header_y * scale,
        header_size: header_size * scale,
        rule_offset: rule_offset * scale,
        row_height: row_height * scale,
        row_size: row_size * scale,
        row_baseline,
        marker_radius: marker_radius * scale,
        graph_top: graph_top * scale,
        graph_bottom: graph_bottom * scale,
    }
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

        let player_count = game.state.players().len();
        let layout = results_layout(viewport, s, player_count);
        let (title, title_color, subtitle) = verdict(game);
        let dims = measure_text(title, None, layout.title_size as u16, 1.0);
        draw_text(
            title,
            (viewport.x - dims.width) * 0.5,
            layout.title_y,
            layout.title_size,
            title_color,
        );
        let stats = game.end_stats.as_ref();
        let duration = stats.map_or(0, |report| report.final_tick);
        let meta = format!("{subtitle}  |  {}", format_duration(duration));
        let mut meta_size = layout.meta_size;
        let min_meta_size = 10.0 * s;
        let max_meta_width = panel.w - 24.0 * s;
        let mut meta_dims = measure_text(&meta, None, meta_size as u16, 1.0);
        while meta_dims.width > max_meta_width && meta_size > min_meta_size {
            meta_size = (meta_size - s).max(min_meta_size);
            meta_dims = measure_text(&meta, None, meta_size as u16, 1.0);
        }
        draw_text(
            &meta,
            (viewport.x - meta_dims.width) * 0.5,
            layout.meta_y,
            meta_size,
            theme::TEXT_BODY,
        );

        let header_y = layout.header_y;
        let row_h = layout.row_height;
        let left = panel.x + 20.0 * s;
        let right = panel.x + panel.w - 20.0 * s;
        let table_w = right - left;
        let columns = [
            left,
            left + table_w * 0.39,
            left + table_w * 0.50,
            left + table_w * 0.59,
            left + table_w * 0.68,
            left + table_w * 0.77,
            left + table_w * 0.87,
        ];
        draw_text(
            "PLAYER",
            columns[0],
            header_y,
            layout.header_size,
            theme::TEXT_SECONDARY,
        );
        draw_text(
            "PEAK",
            columns[1],
            header_y,
            layout.header_size,
            theme::TEXT_SECONDARY,
        );
        draw_text(
            "BUILT",
            (columns[2] + columns[3]) * 0.5 - 17.0 * s,
            header_y,
            layout.header_size,
            theme::TEXT_SECONDARY,
        );
        draw_text(
            "LOST",
            (columns[4] + columns[5]) * 0.5 - 14.0 * s,
            header_y,
            layout.header_size,
            theme::TEXT_SECONDARY,
        );
        draw_text(
            "SCRAP",
            columns[6],
            header_y,
            layout.header_size,
            theme::TEXT_SECONDARY,
        );
        for (x, kind) in [
            (columns[2], StatIcon::Unit),
            (columns[3], StatIcon::Building),
            (columns[4], StatIcon::Unit),
            (columns[5], StatIcon::Building),
        ] {
            draw_stat_icon(
                vec2(x + 5.0 * s, header_y + 9.0 * s),
                4.5 * s,
                kind,
                theme::TEXT_BODY,
            );
        }
        let rule_y = header_y + layout.rule_offset;
        draw_line(left, rule_y, right, rule_y, 1.0 * s, theme::TEXT_DISABLED);

        if let Some(report) = stats {
            let mut seats: Vec<usize> = (0..game.state.players().len()).collect();
            seats.sort_by_key(|seat| (game.state.players()[*seat].team, *seat));
            for (row, seat) in seats.into_iter().enumerate() {
                let player = &game.state.players()[seat];
                let numbers = report
                    .players
                    .iter()
                    .find(|entry| usize::from(entry.seat) == seat);
                let y = rule_y + (row as f32 + layout.row_baseline) * row_h;
                let color = render::seat_identity_color(game, PlayerId(seat as u8));
                draw_marker(
                    columns[0],
                    y - layout.row_size * 0.36,
                    layout.marker_radius,
                    seat,
                    color,
                );
                let winner = game.state.winners().contains(&PlayerId(seat as u8));
                let crown = if winner { " *" } else { "" };
                let name = clipped_name(&player.name, if viewport.x < 800.0 { 15 } else { 24 });
                draw_text(
                    format!("T{}  {}{}", player.team + 1, name, crown),
                    columns[0] + 11.0 * s,
                    y,
                    layout.row_size,
                    color,
                );
                if let Some(numbers) = numbers {
                    let peak = numbers.army_value.iter().copied().max().unwrap_or(0);
                    for (text, x) in [
                        (peak.to_string(), columns[1]),
                        (numbers.units_trained.to_string(), columns[2]),
                        (numbers.buildings_completed.to_string(), columns[3]),
                        (numbers.units_lost.to_string(), columns[4]),
                        (numbers.buildings_lost.to_string(), columns[5]),
                        (numbers.scrap_collected.to_string(), columns[6]),
                    ] {
                        draw_text(&text, x, y, layout.row_size, theme::TEXT_BODY);
                    }
                }
            }

            if layout.graph_bottom - layout.graph_top >= 60.0 * s {
                draw_army_graph(
                    game,
                    report,
                    Rect::new(
                        left,
                        layout.graph_top,
                        right - left,
                        layout.graph_bottom - layout.graph_top,
                    ),
                    s,
                );
            }
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
            winner_subtitle(game, &winners),
        ),
        Some(GameResult::Victory { .. }) if game.state.player(game.human).resigned => (
            "SURRENDERED",
            theme::TEXT_DANGER,
            "your machines fell silent".to_string(),
        ),
        Some(GameResult::Victory { .. }) => (
            "DEFEAT",
            theme::TEXT_DANGER,
            winner_subtitle(game, &winners),
        ),
        Some(GameResult::Draw) | None => (
            "MUTUAL DESTRUCTION",
            theme::TEXT_BODY,
            "no Foundry survived".to_string(),
        ),
    }
}

fn winner_subtitle(game: &Game, winners: &[PlayerId]) -> String {
    if winners.len() > 2 {
        let team = game.state.player(winners[0]).team;
        if winners
            .iter()
            .all(|winner| game.state.player(*winner).team == team)
        {
            return format!("TEAM {} TAKES THE FIELD", team + 1);
        }
    }
    format!(
        "{} take the field",
        winners
            .iter()
            .map(|seat| game.state.player(*seat).name.to_uppercase())
            .collect::<Vec<_>>()
            .join(" & ")
    )
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatIcon {
    Unit,
    Building,
}

fn draw_stat_icon(center: Vec2, size: f32, kind: StatIcon, color: Color) {
    match kind {
        StatIcon::Unit => {
            draw_rectangle(
                center.x - size,
                center.y - size * 0.55,
                size * 2.0,
                size * 1.1,
                color,
            );
            draw_circle(
                center.x - size * 0.62,
                center.y + size * 0.72,
                size * 0.32,
                color,
            );
            draw_circle(
                center.x + size * 0.62,
                center.y + size * 0.72,
                size * 0.32,
                color,
            );
        }
        StatIcon::Building => {
            draw_rectangle_lines(
                center.x - size,
                center.y - size * 0.65,
                size * 2.0,
                size * 1.65,
                1.5,
                color,
            );
            draw_line(
                center.x - size,
                center.y - size * 0.65,
                center.x,
                center.y - size * 1.15,
                1.5,
                color,
            );
            draw_line(
                center.x,
                center.y - size * 1.15,
                center.x + size,
                center.y - size * 0.65,
                1.5,
                color,
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeatMarker {
    Circle,
    Square,
    Diamond,
    Plus,
    RingDot,
    BoxDot,
    Triangle,
    Cross,
}

fn seat_marker(seat: usize) -> SeatMarker {
    match seat {
        0 => SeatMarker::Circle,
        1 => SeatMarker::Square,
        2 => SeatMarker::Diamond,
        3 => SeatMarker::Plus,
        4 => SeatMarker::RingDot,
        5 => SeatMarker::BoxDot,
        6 => SeatMarker::Triangle,
        _ => SeatMarker::Cross,
    }
}

fn draw_marker(x: f32, y: f32, r: f32, seat: usize, color: Color) {
    match seat_marker(seat) {
        SeatMarker::Circle => draw_circle(x, y, r, color),
        SeatMarker::Square => draw_rectangle(x - r, y - r, r * 2.0, r * 2.0, color),
        SeatMarker::Diamond => {
            for (a, b) in [
                (vec2(x, y - r), vec2(x + r, y)),
                (vec2(x + r, y), vec2(x, y + r)),
                (vec2(x, y + r), vec2(x - r, y)),
                (vec2(x - r, y), vec2(x, y - r)),
            ] {
                draw_line(a.x, a.y, b.x, b.y, 1.5, color);
            }
        }
        SeatMarker::Plus => {
            draw_line(x - r, y, x + r, y, 1.5, color);
            draw_line(x, y - r, x, y + r, 1.5, color);
        }
        SeatMarker::RingDot => {
            draw_circle_lines(x, y, r, 1.5, color);
            draw_circle(x, y, r * 0.32, color);
        }
        SeatMarker::BoxDot => {
            draw_rectangle_lines(x - r, y - r, r * 2.0, r * 2.0, 1.5, color);
            draw_circle(x, y, r * 0.32, color);
        }
        SeatMarker::Triangle => {
            let points = [vec2(x, y - r), vec2(x + r, y + r), vec2(x - r, y + r)];
            for index in 0..3 {
                let a = points[index];
                let b = points[(index + 1) % points.len()];
                draw_line(a.x, a.y, b.x, b.y, 1.5, color);
            }
        }
        SeatMarker::Cross => {
            draw_line(x - r, y - r, x + r, y + r, 1.5, color);
            draw_line(x - r, y + r, x + r, y - r, 1.5, color);
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
    let max_value = report
        .players
        .iter()
        .flat_map(|player| player.army_value.iter().copied())
        .max()
        .unwrap_or(1)
        .max(1);
    let ceiling = graph_ceiling(max_value);
    draw_text(
        "ARMY VALUE",
        rect.x + 9.0 * scale,
        rect.y + 17.0 * scale,
        14.0 * scale,
        theme::TEXT_BODY,
    );
    let plot = Rect::new(
        rect.x + (42.0 * scale).min(rect.w * 0.16),
        rect.y + (30.0 * scale).min(rect.h * 0.36),
        (rect.w - (52.0 * scale).min(rect.w * 0.24)).max(1.0),
        (rect.h - (50.0 * scale).min(rect.h * 0.72)).max(1.0),
    );
    for value in [ceiling, ceiling / 2, 0] {
        let y = plot.y + plot.h - plot.h * value as f32 / ceiling as f32;
        draw_line(
            plot.x,
            y,
            plot.x + plot.w,
            y,
            1.0 * scale,
            Color::new(0.55, 0.55, 0.62, 0.18),
        );
        let label = value.to_string();
        let dims = measure_text(&label, None, (13.0 * scale) as u16, 1.0);
        draw_text(
            &label,
            plot.x - dims.width - 5.0 * scale,
            y + 4.0 * scale,
            13.0 * scale,
            theme::TEXT_BODY,
        );
    }
    let final_tick = report.final_tick.max(1);
    let mut x_ticks = vec![0, report.final_tick / 2, report.final_tick];
    x_ticks.dedup();
    for tick in x_ticks {
        let x = plot.x + plot.w * tick as f32 / final_tick as f32;
        draw_line(
            x,
            plot.y,
            x,
            plot.y + plot.h,
            1.0 * scale,
            Color::new(0.55, 0.55, 0.62, 0.13),
        );
        let label = format_duration(tick);
        let dims = measure_text(&label, None, (13.0 * scale) as u16, 1.0);
        let label_x =
            (x - dims.width * 0.5).clamp(rect.x + 2.0, rect.x + rect.w - dims.width - 2.0);
        draw_text(
            &label,
            label_x,
            plot.y + plot.h + 15.0 * scale,
            13.0 * scale,
            theme::TEXT_BODY,
        );
    }
    for player in &report.players {
        let color = render::seat_identity_color(game, PlayerId(player.seat));
        let points = graph_points(
            &report.sample_ticks,
            &player.army_value,
            report.final_tick,
            ceiling,
            plot,
        );
        for pair in points.windows(2) {
            draw_line(
                pair[0].x,
                pair[0].y,
                pair[1].x,
                pair[1].y,
                1.8 * scale,
                color,
            );
        }
        let marker_every = (points.len() / 5).max(1);
        for (sample, point) in points.iter().enumerate() {
            if sample.is_multiple_of(marker_every) || sample + 1 == points.len() {
                draw_marker(
                    point.x,
                    point.y,
                    2.7 * scale,
                    usize::from(player.seat),
                    color,
                );
            }
        }
    }
}

fn graph_ceiling(max_value: u32) -> u32 {
    let max_value = max_value.max(1);
    let magnitude = 10u32.pow(max_value.ilog10());
    [1, 2, 5, 10]
        .into_iter()
        .map(|multiple| magnitude.saturating_mul(multiple))
        .find(|candidate| *candidate >= max_value)
        .unwrap_or(u32::MAX)
}

fn graph_points(
    sample_ticks: &[u64],
    values: &[u32],
    final_tick: u64,
    ceiling: u32,
    plot: Rect,
) -> Vec<Vec2> {
    let final_tick = final_tick.max(1);
    let ceiling = ceiling.max(1);
    sample_ticks
        .iter()
        .copied()
        .zip(values.iter().copied())
        .map(|(tick, value)| {
            vec2(
                plot.x + plot.w * tick.min(final_tick) as f32 / final_tick as f32,
                plot.y + plot.h - plot.h * value.min(ceiling) as f32 / ceiling as f32,
            )
        })
        .collect()
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
    fn eight_player_results_reserve_a_readable_small_screen_graph() {
        let layout = results_layout(vec2(640.0, 400.0), 1.0, 8);
        let last_row =
            layout.header_y + layout.rule_offset + (7.0 + layout.row_baseline) * layout.row_height;

        assert_eq!(layout.title_size, 32.0);
        assert!(layout.meta_y < layout.header_y);
        assert!(last_row + layout.marker_radius < layout.graph_top);
        assert!(layout.graph_bottom - layout.graph_top >= 80.0);
        assert!(layout.graph_bottom < action_rects(vec2(640.0, 400.0), 1.0)[0].y);
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
    fn graph_ceiling_uses_readable_steps() {
        for (value, expected) in [
            (0, 1),
            (1, 1),
            (2, 2),
            (3, 5),
            (11, 20),
            (52, 100),
            (501, 1_000),
        ] {
            assert_eq!(graph_ceiling(value), expected);
        }
    }

    #[test]
    fn graph_uses_sample_time_and_keeps_every_segment() {
        let plot = Rect::new(10.0, 20.0, 100.0, 50.0);
        let points = graph_points(&[0, 41, 82, 100], &[0, 10, 40, 100], 100, 100, plot);
        assert_eq!(points.len(), 4);
        assert_eq!(points.windows(2).count(), 3);
        assert!((points[1].x - 51.0).abs() < f32::EPSILON);
        assert!((points[2].x - 92.0).abs() < f32::EPSILON);
        assert_eq!(points[3], vec2(110.0, 20.0));
    }

    #[test]
    fn every_supported_seat_has_a_distinct_results_marker() {
        let markers = (0..8).map(seat_marker).collect::<Vec<_>>();
        for (index, marker) in markers.iter().enumerate() {
            assert!(
                !markers[..index].contains(marker),
                "seat {index} aliases an earlier graph marker"
            );
        }
    }
}
