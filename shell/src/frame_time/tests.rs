use super::*;

#[test]
fn frame_time_bounds_presentation_and_sanitizes_the_clock() {
    let ordinary = FrameTime::measure(0.016);
    assert_eq!(ordinary.presentation, 0.016);
    assert_eq!(ordinary.raw, 0.016);
    let suspended = FrameTime::measure(60.0);
    assert_eq!(suspended.presentation, MAX_PRESENTATION_DT);
    assert_eq!(
        suspended.raw, 60.0,
        "the sim clocks cap their own catch-up and still see the gap"
    );
    for garbage in [-1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            FrameTime::measure(garbage),
            FrameTime {
                presentation: 0.0,
                raw: 0.0
            }
        );
    }
}
