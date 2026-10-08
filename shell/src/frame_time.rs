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
mod tests;
