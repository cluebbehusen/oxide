//! Measured selection contents and the regions shared by drawing and input.

use super::*;
use crate::numeric;
use crate::panel::info::StatIcon;

pub(super) struct InfoLine {
    pub label: String,
    pub value: String,
    pub icon: Option<StatIcon>,
    pub offset: f32,
    pub section: bool,
}

pub(super) struct InfoLayout {
    pub title: Vec<String>,
    pub status: Vec<String>,
    pub health_y: f32,
    pub lines: Vec<InfoLine>,
    pub roster_y: f32,
    pub height: f32,
    pub font: f32,
    pub line_h: f32,
    pub roster_size: f32,
    pub roster_columns: usize,
}

pub(super) fn measure_info(
    panel: &crate::panel::Panel,
    width: f32,
    scale: f32,
    compact: bool,
    measure: super::hud::Measure<'_>,
) -> InfoLayout {
    use super::hud::Face;
    use crate::theme::Type;
    let measure_body = |text: &str, size: f32| measure(Face::Body, text, size);
    let font = if compact { Type::Small } else { Type::Body }.at(scale);
    let line_h = if compact { 18.0 } else { 22.0 } * scale;
    let inner = width - 24.0 * scale;
    let title = wrap_words(
        &panel.title,
        |s| measure(Face::Display, s, Type::Body.at(scale)),
        width - 64.0 * scale,
    );
    let mut y = (44.0 * scale).max(14.0 * scale + title.len() as f32 * 17.0 * scale);
    let status: Vec<_> = panel
        .info
        .status
        .iter()
        .flat_map(|text| wrap_words(text, |s| measure_body(s, font), inner))
        .collect();
    y += status.len() as f32 * line_h;
    let health_y = y;
    if panel.info.health.is_some() {
        y += 28.0 * scale;
    }
    let mut lines = Vec::new();
    for row in &panel.info.rows {
        if row.section {
            y += 4.0 * scale;
        }
        let text_width = inner - 20.0 * scale;
        if measure_body(&row.label, font) + measure_body(&row.value, font) + 10.0 * scale
            <= text_width
        {
            lines.push(InfoLine {
                label: row.label.clone(),
                value: row.value.clone(),
                icon: row.icon,
                offset: y,
                section: row.section,
            });
            y += line_h;
        } else {
            for (index, label) in wrap_words(&row.label, |s| measure_body(s, font), text_width)
                .into_iter()
                .enumerate()
            {
                lines.push(InfoLine {
                    label,
                    value: String::new(),
                    icon: (index == 0).then_some(row.icon).flatten(),
                    offset: y,
                    section: row.section,
                });
                y += line_h;
            }
            for value in wrap_words(&row.value, |s| measure_body(s, font), text_width) {
                lines.push(InfoLine {
                    label: String::new(),
                    value,
                    icon: None,
                    offset: y,
                    section: false,
                });
                y += line_h;
            }
        }
    }
    let roster_size = if compact { 44.0 } else { 64.0 } * scale;
    let roster_columns = numeric::to_usize(
        ((width - 16.0 * scale + 4.0 * scale) / (roster_size + 4.0 * scale)).floor(),
    )
    .max(1);
    let roster_y = y + 18.0 * scale;
    if !panel.roster.is_empty() {
        y = roster_y
            + panel.roster.len().min(8).div_ceil(roster_columns) as f32
                * (roster_size + 4.0 * scale);
    }
    InfoLayout {
        title,
        status,
        health_y,
        lines,
        roster_y,
        height: y + 6.0 * scale,
        font,
        line_h,
        roster_size,
        roster_columns,
    }
}

/// A panel's clickable regions, as the HUD publishes them.
pub(super) struct PanelGeometry {
    pub info: Rect,
    pub actions: Rect,
    pub orders: Rect,
    pub roster_slots: [(Rect, crate::panel::CardAction); 8],
    pub roster_count: usize,
    pub cards: [(Rect, crate::panel::CardAction); 16],
    pub card_count: usize,
    pub queue_slots: [(Rect, crate::panel::CardAction); 8],
    pub queue_count: usize,
    pub queue_stop: (Rect, crate::panel::CardAction),
    pub hides_minimap: bool,
}

impl PanelGeometry {
    /// No panel: nothing to click.
    pub(super) fn empty() -> Self {
        use crate::panel::CardAction;
        let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
        Self {
            info: zero,
            actions: zero,
            orders: zero,
            roster_slots: [(zero, CardAction::None); 8],
            roster_count: 0,
            cards: [(zero, CardAction::None); 16],
            card_count: 0,
            queue_slots: [(zero, CardAction::None); 8],
            queue_count: 0,
            queue_stop: (zero, CardAction::None),
            hides_minimap: false,
        }
    }
}
