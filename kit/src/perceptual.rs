//! Perceptual image comparison for the screenshot suite.
//!
//! Byte-exact goldens don't survive GPU, font, or driver churn, so the
//! live-shell shot suite compares on a tolerance metric instead: mean
//! absolute per-channel difference, expressed as a percentage of full
//! scale. Zero means identical; small fractions of a percent absorb
//! anti-aliasing jitter; layout changes score orders of magnitude
//! higher. References are per-machine and live outside the repo — this
//! is a local gate, never a CI one.

use anyhow::{Context, Result};
use std::path::Path;
use tiny_skia::Pixmap;

/// How two images compared.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Same dimensions; the score is the mean absolute per-channel
    /// difference as a percentage (0.0 = identical, 100.0 = inverse).
    Score(f64),
    /// Different dimensions — not comparable (a window-size or DPI
    /// change; re-bless on this machine).
    SizeMismatch {
        /// Reference dimensions.
        reference: (u32, u32),
        /// Candidate dimensions.
        candidate: (u32, u32),
    },
}

/// Compares two pixmaps of any origin.
pub fn diff(reference: &Pixmap, candidate: &Pixmap) -> Verdict {
    if reference.width() != candidate.width() || reference.height() != candidate.height() {
        return Verdict::SizeMismatch {
            reference: (reference.width(), reference.height()),
            candidate: (candidate.width(), candidate.height()),
        };
    }
    let a = reference.data();
    let b = candidate.data();
    let total: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    let score = total as f64 / (a.len() as f64 * 255.0) * 100.0;
    Verdict::Score(score)
}

/// Compares two PNG files.
pub fn diff_pngs(reference: &Path, candidate: &Path) -> Result<Verdict> {
    let load = |path: &Path| -> Result<Pixmap> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Pixmap::decode_png(&bytes).with_context(|| format!("decoding {}", path.display()))
    };
    Ok(diff(&load(reference)?, &load(candidate)?))
}

#[cfg(test)]
mod tests;
