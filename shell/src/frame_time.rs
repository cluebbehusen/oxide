//! How much wall time one presented frame covers.

/// One presented frame's wall time. Presentation (camera glides, held
/// pans, effects, music) reads a clamped value so a suspension or a
/// clock jump cannot fling it; the live and replay clocks read the
/// unclamped value and cap their own catch-up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameTime {
    pub(crate) presentation: f32,
    pub(crate) raw: f32,
}

/// Presentation never advances more than this per frame.
const MAX_PRESENTATION_DT: f32 = 0.25;

impl FrameTime {
    /// Frame time comes from the system clock, which can step backward
    /// or produce garbage; either reads as no time passing, since a NaN
    /// in a clock accumulator would stall it for good.
    pub(crate) fn measure(reported: f32) -> Self {
        let raw = if reported.is_finite() {
            reported.max(0.0)
        } else {
            0.0
        };
        Self {
            presentation: raw.min(MAX_PRESENTATION_DT),
            raw,
        }
    }
}

#[cfg(test)]
mod tests {
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
}
