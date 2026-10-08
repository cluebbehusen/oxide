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
                let layout = geometry(viewport, s, mode, 65.0 * s, Some((occupied, status_left)));
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
