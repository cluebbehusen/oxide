//! Measured selection contents and the regions shared by drawing and input.

use super::*;
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
    measure: impl Fn(&str, f32) -> f32,
) -> InfoLayout {
    let font = if compact { 14.0 } else { 15.0 } * scale;
    let line_h = if compact { 18.0 } else { 22.0 } * scale;
    let inner = width - 24.0 * scale;
    let title = wrap_words(
        &panel.title,
        |s| measure(s, 15.0 * scale),
        width - 64.0 * scale,
    );
    let mut y = (44.0 * scale).max(14.0 * scale + title.len() as f32 * 17.0 * scale);
    let status: Vec<_> = panel
        .info
        .status
        .iter()
        .flat_map(|text| wrap_words(text, |s| measure(s, font), inner))
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
        if measure(&row.label, font) + measure(&row.value, font) + 10.0 * scale <= text_width {
            lines.push(InfoLine {
                label: row.label.clone(),
                value: row.value.clone(),
                icon: row.icon,
                offset: y,
                section: row.section,
            });
            y += line_h;
        } else {
            for (index, label) in wrap_words(&row.label, |s| measure(s, font), text_width)
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
            for value in wrap_words(&row.value, |s| measure(s, font), text_width) {
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
    let roster_columns = (((width - 16.0 * scale + 4.0 * scale) / (roster_size + 4.0 * scale))
        .floor() as usize)
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
    pub hides_minimap: bool,
}
