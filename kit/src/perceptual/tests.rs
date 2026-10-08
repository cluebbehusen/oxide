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
