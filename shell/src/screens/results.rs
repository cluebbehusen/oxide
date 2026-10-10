//! The decided-match report: team-grouped scoreboard, match curve, and
//! touchable next steps. It is its own screen with its own input, not an
//! overlay on the pause menu.

use crate::bot_label::{BotLabelStyle, bot_label};
use crate::game::{Game, SoundKind};
use crate::nav::{Axis, Nav, step_line};
use crate::numeric;
use crate::numeric::Fit;
use crate::press::{Fed, Press};
use crate::theme::Type;
use crate::{render, theme};
use macroquad::prelude::*;
use oxide_protocol::{Key, RawEvent};
use oxide_sim::{GameResult, PlayerId, TICKS_PER_SECOND};

const WIDE_STAT_HEADERS: [[&str; 2]; 6] = [
    ["PEAK ARMY", "VALUE"],
    ["UNITS", "BUILT"],
    ["BUILDINGS", "BUILT"],
    ["UNITS", "LOST"],
    ["BUILDINGS", "LOST"],
    ["SCRAP", "COLLECTED"],
];

/// What a result frame decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// Stay on the report.
    Stay,
    /// Start the same authored match again.
    Rematch,
    /// Watch the completed command record.
    Watch,
    /// Inspect the already-final battlefield without replaying it.
    ViewFinalMap,
    /// Return to the front door.
    Home,
}

/// One of the report's next steps, in button order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultAction {
    Rematch,
    Watch,
    ViewFinalMap,
    Home,
}

impl ResultAction {
    /// Every action, left to right.
    pub const ALL: [Self; 4] = [Self::Rematch, Self::Watch, Self::ViewFinalMap, Self::Home];

    fn label(self) -> &'static str {
        match self {
            Self::Rematch => "REMATCH",
            Self::Watch => "WATCH REPLAY",
            Self::ViewFinalMap => "VIEW FINAL MAP",
            Self::Home => "HOME",
        }
    }

    fn out(self) -> Out {
        match self {
            Self::Rematch => Out::Rematch,
            Self::Watch => Out::Watch,
            Self::ViewFinalMap => Out::ViewFinalMap,
            Self::Home => Out::Home,
        }
    }

    /// Where the button sits in the row.
    fn index(self) -> usize {
        match self {
            Self::Rematch => 0,
            Self::Watch => 1,
            Self::ViewFinalMap => 2,
            Self::Home => 3,
        }
    }
}

/// Touchable action geometry, injected for headless tests.
pub(crate) fn action_rects(viewport: Vec2, scale: f32) -> [Rect; ResultAction::ALL.len()] {
    let gap = 10.0 * scale;
    let margin = 24.0 * scale;
    let action_count = ResultAction::ALL.len() as f32;
    let available = (viewport.x - margin * 2.0 - gap * (action_count - 1.0)).max(action_count);
    let width = (available / action_count).min(210.0 * scale);
    let total = width * action_count + gap * (action_count - 1.0);
    let x = (viewport.x - total) * 0.5;
    let y = viewport.y - 60.0 * scale;
    std::array::from_fn(|index| {
        Rect::new(
            x + (width + gap) * index as f32,
            y,
            width,
            crate::theme::MIN_TOUCH_TARGET * scale,
        )
    })
}

#[derive(Debug, Clone, Copy)]
struct ResultsLayout {
    compact_roster: bool,
    wide_table: bool,
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
    graph_title_size: f32,
    graph_label_size: f32,
    graph_top: f32,
    graph_bottom: f32,
}

/// The report's vertical rhythm in logical px, before any fitting.
#[derive(Debug, Clone, Copy)]
struct ResultsMetrics {
    title_y: f32,
    title_size: f32,
    meta_y: f32,
    meta_size: f32,
    header_y: f32,
    header_size: f32,
    rule_offset: f32,
    row_height: f32,
    row_size: f32,
    /// Where in its row a player's text sits, as a fraction of the row.
    row_baseline: f32,
    marker_radius: f32,
    /// Rows of space between the table and the graph.
    graph_row_padding: f32,
    graph_padding: f32,
}

impl ResultsMetrics {
    /// Six or more players in a short window.
    const COMPACT: Self = Self {
        title_y: 43.0,
        title_size: Type::Title.px(),
        meta_y: 62.0,
        meta_size: Type::Small.px(),
        header_y: 79.0,
        header_size: Type::Caption.px(),
        rule_offset: 13.0,
        row_height: 16.0,
        row_size: Type::Small.px(),
        row_baseline: 0.78,
        marker_radius: 3.4,
        graph_row_padding: 0.45,
        graph_padding: 3.0,
    };

    /// Every other report; a short window packs its rows tighter.
    fn standard(short: bool) -> Self {
        Self {
            title_y: 48.0,
            title_size: Type::Title.px(),
            meta_y: 68.0,
            meta_size: Type::Body.px(),
            header_y: 91.0,
            header_size: Type::Body.px(),
            rule_offset: 16.0,
            row_height: if short { 22.0 } else { 27.0 },
            row_size: Type::Label.px(),
            row_baseline: 0.85,
            marker_radius: 4.0,
            graph_row_padding: 1.15,
            graph_padding: 5.0,
        }
    }
}

fn results_layout(viewport: Vec2, scale: f32, player_count: usize) -> ResultsLayout {
    let logical_width = viewport.x / scale.max(f32::EPSILON);
    let logical_height = viewport.y / scale.max(f32::EPSILON);
    let compact_roster = logical_height <= 480.0 && player_count >= 6;
    let wide_table = !compact_roster && logical_width >= 1_100.0;
    let ResultsMetrics {
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
    } = if compact_roster {
        ResultsMetrics::COMPACT
    } else {
        ResultsMetrics::standard(logical_height <= 480.0)
    };
    let rule_offset = rule_offset + if wide_table { header_size } else { 0.0 };
    let actions_y = action_rects(viewport, scale)[0].y / scale;
    let row_height = f32::min(
        row_height,
        (actions_y - header_y - rule_offset - 8.0) / player_count.max(1) as f32,
    );
    let graph_top = header_y
        + rule_offset
        + (player_count as f32 + graph_row_padding) * row_height
        + graph_padding;
    let graph_bottom = actions_y - 25.0;

    ResultsLayout {
        compact_roster,
        wide_table,
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
        graph_title_size: if compact_roster {
            Type::Small
        } else {
            Type::Body
        }
        .at(scale),
        graph_label_size: if compact_roster {
            Type::Small
        } else {
            Type::Body
        }
        .at(scale),
        graph_top: graph_top * scale,
        graph_bottom: graph_bottom * scale,
    }
}

fn action_at(point: Vec2, viewport: Vec2, scale: f32) -> Option<ResultAction> {
    ResultAction::ALL
        .into_iter()
        .zip(action_rects(viewport, scale))
        .find(|(_, rect)| rect.contains(point))
        .map(|(action, _)| action)
}

/// Where the scoreboard's columns sit: the player name's left edge, then
/// the six statistics (peak army; units and buildings built; units and
/// buildings lost; scrap). A wide table centers each statistic on its x.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TableColumns {
    player: f32,
    stats: [f32; 6],
}

fn table_columns(left: f32, right: f32, wide: bool) -> TableColumns {
    let width = right - left;
    let stats = if wide {
        let stats_left = player_column_right(left, right, true);
        let stat_width = (right - stats_left) / 6.0;
        std::array::from_fn(|index| stats_left + (index as f32 + 0.5) * stat_width)
    } else {
        [0.39, 0.50, 0.59, 0.68, 0.77, 0.87].map(|fraction| left + width * fraction)
    };
    TableColumns {
        player: left,
        stats,
    }
}

fn player_column_right(left: f32, right: f32, wide: bool) -> f32 {
    left + (right - left) * if wide { 0.34 } else { 0.39 }
}

fn draw_centered_text(text: &str, x: f32, y: f32, size: f32, color: Color) {
    let width = measure_text(text, None, numeric::font_size(size), 1.0).width;
    draw_text(text, x - width * 0.5, y, size, color);
}

fn player_name_with_controller(
    game: &Game,
    seat: usize,
    max_name_chars: usize,
    compact: bool,
) -> String {
    let player = &game.state.players()[seat];
    let name = clipped_name(&player.name, max_name_chars);
    let Some(spec) = game.scenario.players.get(seat) else {
        return name;
    };
    if !spec.bot {
        return name;
    }
    match (spec.bot_config, compact) {
        (Some(config), true) => format!(
            "{name}  {}",
            bot_label(config.difficulty, config.stance, BotLabelStyle::Compact,)
        ),
        (Some(config), false) => format!(
            "{name}  {}",
            bot_label(config.difficulty, config.stance, BotLabelStyle::Result,)
        ),
        (None, _) => format!("{name}  AI"),
    }
}

fn text_fits(text: &str, max_width: f32, measure: &impl Fn(&str) -> f32) -> bool {
    measure(text) <= max_width
}

fn elide_text_to_width(text: &str, max_width: f32, measure: &impl Fn(&str) -> f32) -> String {
    if text_fits(text, max_width, measure) {
        return text.to_string();
    }
    for cap in (4..text.chars().count()).rev() {
        let candidate = clipped_name(text, cap);
        if text_fits(&candidate, max_width, measure) {
            return candidate;
        }
    }
    String::new()
}

fn fitted_player_name_with_controller(
    game: &Game,
    seat: usize,
    max_name_chars: usize,
    prefer_compact: bool,
    max_width: f32,
    measure: &impl Fn(&str) -> f32,
) -> String {
    if !prefer_compact {
        let full = player_name_with_controller(game, seat, max_name_chars, false);
        if text_fits(&full, max_width, measure) {
            return full;
        }
    }

    let compact = player_name_with_controller(game, seat, max_name_chars, true);
    if text_fits(&compact, max_width, measure) {
        return compact;
    }
    for cap in (4..max_name_chars).rev() {
        let candidate = player_name_with_controller(game, seat, cap, true);
        if text_fits(&candidate, max_width, measure) {
            return candidate;
        }
    }

    let spec = game.scenario.players.get(seat);
    let fallback = match spec {
        Some(spec) if spec.bot => spec.bot_config.map_or_else(
            || "AI".to_string(),
            |config| bot_label(config.difficulty, config.stance, BotLabelStyle::Compact),
        ),
        _ => game.state.players()[seat].name.clone(),
    };
    elide_text_to_width(&fallback, max_width, measure)
}

/// Stateful pointer/keyboard ownership for the report.
pub struct ResultsScreen {
    selected: ResultAction,
    hover: Option<ResultAction>,
    press: Press<ResultAction>,
}

impl ResultsScreen {
    /// Opens with Rematch selected.
    pub fn open() -> Self {
        Self {
            selected: ResultAction::Rematch,
            hover: None,
            press: Press::default(),
        }
    }

    /// Keyboard cursor for the debug UI surface.
    pub fn selected(&self) -> usize {
        self.selected.index()
    }

    /// Pointer hover for the debug UI surface.
    pub fn hover(&self) -> Option<usize> {
        self.hover.map(ResultAction::index)
    }

    /// Stable labels for automation and accessibility.
    pub fn items() -> Vec<String> {
        ResultAction::ALL
            .iter()
            .map(|action| action.label().to_string())
            .collect()
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
            if let Fed::Activated(action) =
                self.press.feed(event, |p, _| action_at(p, viewport, scale))
            {
                if let Some(p) = crate::press::position(event) {
                    *mouse = p;
                }
                self.selected = action;
                sounds.push((SoundKind::Click, None));
                return action.out();
            }
            match *event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    self.hover = action_at(*mouse, viewport, scale);
                }
                RawEvent::TouchDown { id, x, y } | RawEvent::TouchMove { id, x, y }
                    if self.press.owns(id) =>
                {
                    *mouse = vec2(x, y);
                    self.hover = action_at(*mouse, viewport, scale);
                }
                RawEvent::KeyDown { key: Key::Enter } => {
                    sounds.push((SoundKind::Click, None));
                    return self.selected.out();
                }
                RawEvent::KeyDown { key: Key::Escape } => {
                    sounds.push((SoundKind::Click, None));
                    return Out::Home;
                }
                RawEvent::KeyDown { .. } => {
                    if let Some(next) = Nav::decode(event).and_then(|nav| {
                        let count = ResultAction::ALL.len();
                        step_line(count, self.selected.index(), nav, Axis::Both, count, |_| {
                            true
                        })
                    }) {
                        self.hover = None;
                        self.selected = ResultAction::ALL[next];
                    }
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
        draw_rectangle(0.0, 0.0, viewport.x, viewport.y, theme::VEIL);
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
            theme::Stroke::Edge.at(s),
            theme::EDGE_WARM,
        );

        let player_count = game.state.players().len();
        let layout = results_layout(viewport, s, player_count);
        let (title, title_color, subtitle) = verdict(game);
        let dims = measure_text(title, None, numeric::font_size(layout.title_size), 1.0);
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
        let meta_size =
            crate::typography::fit(&meta, layout.meta_size, panel.w - 24.0 * s, 10.0 * s);
        let meta_dims = measure_text(&meta, None, numeric::font_size(meta_size), 1.0);
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
        let columns = table_columns(left, right, layout.wide_table);
        draw_text(
            "PLAYER",
            columns.player,
            header_y,
            layout.header_size,
            theme::TEXT_SECONDARY,
        );
        if layout.wide_table {
            for (label, x) in WIDE_STAT_HEADERS.into_iter().zip(columns.stats) {
                for (line, text) in label.into_iter().enumerate() {
                    draw_centered_text(
                        text,
                        x,
                        header_y + line as f32 * layout.header_size,
                        layout.header_size,
                        theme::TEXT_SECONDARY,
                    );
                }
            }
        } else {
            draw_text(
                "PEAK",
                columns.stats[0],
                header_y,
                layout.header_size,
                theme::TEXT_SECONDARY,
            );
            draw_text(
                "BUILT",
                f32::midpoint(columns.stats[1], columns.stats[2]) - 17.0 * s,
                header_y,
                layout.header_size,
                theme::TEXT_SECONDARY,
            );
            draw_text(
                "LOST",
                f32::midpoint(columns.stats[3], columns.stats[4]) - 14.0 * s,
                header_y,
                layout.header_size,
                theme::TEXT_SECONDARY,
            );
            draw_text(
                "SCRAP",
                columns.stats[5],
                header_y,
                layout.header_size,
                theme::TEXT_SECONDARY,
            );
            for (x, kind) in [
                (columns.stats[1], StatIcon::Unit),
                (columns.stats[2], StatIcon::Building),
                (columns.stats[3], StatIcon::Unit),
                (columns.stats[4], StatIcon::Building),
            ] {
                draw_stat_icon(
                    vec2(x + 5.0 * s, header_y + 9.0 * s),
                    4.5 * s,
                    kind,
                    theme::TEXT_BODY,
                );
            }
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
                let color = render::seat_identity_color(&game.view(), PlayerId(seat.fit::<u8>()));
                draw_marker(
                    columns.player,
                    y - layout.row_size * 0.36,
                    layout.marker_radius,
                    seat,
                    color,
                );
                let winner = game.state.winners().contains(&PlayerId(seat.fit::<u8>()));
                let crown = if winner { " *" } else { "" };
                let max_name_chars = if viewport.x < 800.0 {
                    if game.scenario.players.get(seat).is_some_and(|spec| spec.bot) {
                        12
                    } else {
                        15
                    }
                } else {
                    24
                };
                let prefix = format!("T{}  ", player.team + 1);
                let player_text_x = columns.player + 11.0 * s;
                let player_text_right = player_column_right(left, right, layout.wide_table);
                let fixed_width = measure_text(
                    format!("{prefix}{crown}"),
                    None,
                    numeric::font_size(layout.row_size),
                    1.0,
                )
                .width;
                let name_width =
                    (player_text_right - player_text_x - 8.0 * s - fixed_width).max(0.0);
                let measure_name = |text: &str| {
                    measure_text(text, None, numeric::font_size(layout.row_size), 1.0).width
                };
                let name = fitted_player_name_with_controller(
                    game,
                    seat,
                    max_name_chars,
                    layout.compact_roster,
                    name_width,
                    &measure_name,
                );
                draw_text(
                    format!("{prefix}{name}{crown}"),
                    player_text_x,
                    y,
                    layout.row_size,
                    color,
                );
                if let Some(numbers) = numbers {
                    let peak = numbers.army_value.iter().copied().max().unwrap_or(0);
                    for (text, x) in [
                        (peak.to_string(), columns.stats[0]),
                        (numbers.units_trained.to_string(), columns.stats[1]),
                        (numbers.buildings_completed.to_string(), columns.stats[2]),
                        (numbers.units_lost.to_string(), columns.stats[3]),
                        (numbers.buildings_lost.to_string(), columns.stats[4]),
                        (numbers.scrap_collected.to_string(), columns.stats[5]),
                    ] {
                        if layout.wide_table {
                            draw_centered_text(&text, x, y, layout.row_size, theme::TEXT_BODY);
                        } else {
                            draw_text(&text, x, y, layout.row_size, theme::TEXT_BODY);
                        }
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
                    layout.graph_title_size,
                    layout.graph_label_size,
                );
            }
        } else {
            draw_text(
                "Compiling the final record...",
                left,
                header_y + 30.0 * s,
                crate::theme::Type::Body.at(s),
                theme::TEXT_BODY,
            );
        }

        for (action, rect) in ResultAction::ALL.into_iter().zip(action_rects(viewport, s)) {
            let active = self.hover == Some(action) || self.selected == action;
            crate::button::draw(rect, action.label(), active, s);
        }
    }
}

fn verdict(game: &Game) -> (&'static str, Color, String) {
    let winners = game.state.winners();
    match game.state.result() {
        Some(GameResult::Victory { .. }) if winners.contains(&game.presentation.human) => (
            "VICTORY",
            theme::TEXT_ACCENT,
            winner_subtitle(game, &winners),
        ),
        Some(GameResult::Victory { .. }) if game.state.player(game.presentation.human).resigned => {
            (
                "SURRENDERED",
                theme::TEXT_DANGER,
                "your machines fell silent".to_string(),
            )
        }
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

fn draw_army_graph(
    game: &Game,
    report: &oxide_kit::stats::MatchStats,
    rect: Rect,
    scale: f32,
    title_size: f32,
    label_size: f32,
) {
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
        title_size,
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
        let dims = measure_text(&label, None, numeric::font_size(label_size), 1.0);
        draw_text(
            &label,
            plot.x - dims.width - 5.0 * scale,
            y + 4.0 * scale,
            label_size,
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
        let dims = measure_text(&label, None, numeric::font_size(label_size), 1.0);
        let label_x =
            (x - dims.width * 0.5).clamp(rect.x + 2.0, rect.x + rect.w - dims.width - 2.0);
        draw_text(
            &label,
            label_x,
            plot.y + plot.h + 15.0 * scale,
            label_size,
            theme::TEXT_BODY,
        );
    }
    for player in &report.players {
        let color = render::seat_identity_color(&game.view(), PlayerId(player.seat));
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
mod tests;
