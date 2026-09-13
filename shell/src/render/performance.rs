//! Performance chrome uses the same measured geometry for drawing and input.

use crate::config::PerformanceDisplay;
use crate::performance::PerformanceView;
use crate::{layout::TOP_BAR_H, theme, typography};
use macroquad::prelude::*;

struct Geometry {
    panel: Rect,
    fps: Vec2,
    details_y: f32,
}

fn geometry(
    viewport: Vec2,
    scale: f32,
    mode: PerformanceDisplay,
    fps_width: f32,
    status_space: Option<(f32, f32)>,
) -> Geometry {
    let gap = 12.0 * scale;
    let in_bar = status_space.and_then(|(occupied, status_left)| {
        let x = status_left - gap - fps_width;
        (x >= occupied + gap).then_some(x)
    });
    let header_height = if in_bar.is_some() { 0.0 } else { 28.0 * scale };
    let detail_height = if mode == PerformanceDisplay::Detailed {
        130.0 * scale
    } else {
        0.0
    };
    let width = if mode == PerformanceDisplay::Detailed {
        240.0 * scale
    } else {
        fps_width + 20.0 * scale
    };
    let panel_y = if status_space.is_some() {
        TOP_BAR_H * scale + 6.0 * scale
    } else {
        8.0 * scale
    };
    let panel = if header_height + detail_height > 0.0 {
        Rect::new(
            viewport.x - gap - width,
            panel_y,
            width,
            header_height + detail_height,
        )
    } else {
        Rect::new(0.0, 0.0, 0.0, 0.0)
    };
    let fps = match in_bar {
        Some(x) => vec2(x, 26.0 * scale),
        None => vec2(
            panel.x + panel.w - 10.0 * scale - fps_width,
            panel.y + 20.0 * scale,
        ),
    };
    Geometry {
        panel,
        fps,
        details_y: panel.y + header_height,
    }
}

pub(super) fn draw(view: &PerformanceView, status_space: Option<(f32, f32)>) -> Rect {
    if view.mode == PerformanceDisplay::Off {
        return Rect::new(0.0, 0.0, 0.0, 0.0);
    }
    let s = super::ui_scale();
    let size = 14.0 * s;
    let fps = view
        .fps
        .map_or_else(|| "-- FPS".to_string(), |fps| format!("{fps:.0} FPS"));
    let width = typography::measure(&fps, size).width;
    let layout = geometry(super::viewport(), s, view.mode, width, status_space);
    let panel = layout.panel;
    if panel.w > 0.0 {
        draw_rectangle(
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            Color::from_rgba(15, 15, 19, 230),
        );
    }
    typography::draw(
        &fps,
        layout.fps.x,
        layout.fps.y,
        size,
        theme::TEXT_SECONDARY,
    );
    if view.mode != PerformanceDisplay::Detailed {
        return panel;
    }
    let x = panel.x + 10.0 * s;
    for (index, (label, value)) in [
        ("Frame", view.frame_ms),
        ("CPU work", view.work_ms),
        ("Max frame", view.max_ms),
    ]
    .into_iter()
    .enumerate()
    {
        let value = value.map_or_else(|| "--".to_string(), |value| format!("{value:.1}"));
        typography::draw(
            &format!("{label}: {value} ms"),
            x,
            layout.details_y + (19.0 + index as f32 * 19.0) * s,
            size,
            theme::TEXT_SECONDARY,
        );
    }
    let graph = Rect::new(x, layout.details_y + 67.0 * s, panel.w - 20.0 * s, 42.0 * s);
    draw_rectangle(
        graph.x,
        graph.y,
        graph.w,
        graph.h,
        Color::from_rgba(5, 5, 9, 160),
    );
    for ms in [1000.0 / 60.0, 2000.0 / 60.0] {
        let y = graph.y + graph.h * (1.0 - ms / 50.0);
        draw_line(
            graph.x,
            y,
            graph.x + graph.w,
            y,
            s,
            Color::from_rgba(120, 120, 125, 100),
        );
    }
    let step = graph.w / view.graph.len() as f32;
    for (index, ms) in view.graph.iter().enumerate() {
        if let Some(ms) = ms {
            let height = ((*ms / 50.0).clamp(0.0, 1.0) as f32 * graph.h).max(s);
            let x = graph.x + index as f32 * step;
            draw_rectangle(
                x,
                graph.y + graph.h - height,
                (step - s).max(s),
                height,
                theme::TEXT_SECONDARY,
            );
            if *ms > 50.0 {
                draw_rectangle(x, graph.y, (step - s).max(s), 3.0 * s, theme::TEXT_DANGER);
            }
        }
    }
    typography::draw(
        "5s  |  16.7 / 33.3  |  0-50 ms",
        x,
        graph.y + graph.h + 14.0 * s,
        11.0 * s,
        theme::TEXT_SECONDARY,
    );
    panel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_status_content_decides_whether_fps_fits() {
        for scale in [0.75_f32, 1.0, 1.25, 1.5] {
            for (viewport, occupied, status_left, in_bar) in [
                (vec2(1280.0, 800.0), 600.0, 1100.0, true),
                (vec2(640.0, 400.0), 530.0, 550.0, false),
            ] {
                let s = scale
                    .min((viewport.x / 640.0_f32).max(1.0))
                    .min((viewport.y / 400.0_f32).max(1.0));
                for mode in [PerformanceDisplay::Fps, PerformanceDisplay::Detailed] {
                    let layout =
                        geometry(viewport, s, mode, 65.0 * s, Some((occupied, status_left)));
                    assert_eq!(layout.fps.y == 26.0 * s, in_bar);
                    if in_bar {
                        assert!(layout.fps.x >= occupied + 12.0 * s);
                        assert!(layout.fps.x + 65.0 * s <= status_left - 12.0 * s);
                    }
                    if layout.panel.w > 0.0 {
                        assert!(layout.panel.x >= 0.0);
                        assert!(layout.panel.x + layout.panel.w <= viewport.x);
                        assert!(layout.panel.y >= TOP_BAR_H * s);
                        assert!(layout.panel.y + layout.panel.h <= viewport.y);
                        let model = crate::layout::LayoutModel {
                            performance: layout.panel,
                            ..Default::default()
                        };
                        assert!(model.chrome_owns(layout.panel.center()));
                        assert!(
                            !model.chrome_owns(vec2(layout.panel.x - 1.0, layout.panel.center().y))
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn spectator_has_a_backed_header_without_a_status_bar() {
        for mode in [PerformanceDisplay::Fps, PerformanceDisplay::Detailed] {
            let layout = geometry(vec2(640.0, 400.0), 1.0, mode, 65.0, None);
            assert_eq!(layout.panel.y, 8.0);
            assert!(layout.panel.contains(layout.fps));
            assert!(layout.panel.y + layout.panel.h < 200.0);
        }
    }
}
