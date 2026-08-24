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
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Pixmap {
        let mut pixmap = Pixmap::new(w, h).unwrap();
        for px in pixmap.data_mut().as_chunks_mut::<4>().0 {
            px.copy_from_slice(&rgba);
        }
        pixmap
    }

    #[test]
    fn identical_images_score_zero() {
        let a = solid(8, 8, [10, 200, 30, 255]);
        let b = solid(8, 8, [10, 200, 30, 255]);
        assert_eq!(diff(&a, &b), Verdict::Score(0.0));
    }

    #[test]
    fn a_known_uniform_delta_scores_its_exact_mean() {
        // Premultiplied storage keeps these values as written (alpha
        // 255), so every red byte differs by 51: one channel of four,
        // 51/255 of scale -> exactly 5%.
        let a = solid(4, 4, [0, 0, 0, 255]);
        let b = solid(4, 4, [51, 0, 0, 255]);
        let Verdict::Score(score) = diff(&a, &b) else {
            panic!("dimensions match");
        };
        assert!((score - 5.0).abs() < 1e-9, "got {score}");
    }

    #[test]
    fn dimension_mismatch_is_named_not_scored() {
        let a = solid(8, 8, [0, 0, 0, 255]);
        let b = solid(8, 4, [0, 0, 0, 255]);
        assert_eq!(
            diff(&a, &b),
            Verdict::SizeMismatch {
                reference: (8, 8),
                candidate: (8, 4),
            }
        );
    }

    #[test]
    fn png_round_trip_compares_equal() {
        let dir = std::env::temp_dir().join(format!("oxide-perceptual-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = solid(6, 6, [90, 60, 30, 255]);
        let path_a = dir.join("a.png");
        let path_b = dir.join("b.png");
        a.save_png(&path_a).unwrap();
        a.save_png(&path_b).unwrap();
        assert_eq!(diff_pngs(&path_a, &path_b).unwrap(), Verdict::Score(0.0));
        std::fs::remove_dir_all(&dir).ok();
    }
}
