use super::*;

#[test]
fn screenshot_encoding_flips_gpu_rows_and_reports_path_errors() {
    let root = std::env::temp_dir().join(format!(
        "oxide-png-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let path = root.join("nested/shot.png");
    let image = Image {
        width: 2,
        height: 2,
        // GPU order: bottom row first, then top row.
        bytes: vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    };

    assert_eq!(write_png(&image, path.to_str().unwrap()).unwrap(), (2, 2));
    let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut decoded = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut decoded).unwrap();
    assert_eq!(info.width, 2);
    assert_eq!(info.height, 2);
    assert_eq!(
        &decoded[..info.buffer_size()],
        &[
            0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255,
        ],
        "PNG rows are top-down even though the captured framebuffer is bottom-up"
    );

    let blocked = root.join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    assert!(
        write_png(&image, blocked.join("shot.png").to_str().unwrap()).is_err(),
        "a malformed screenshot path is a protocol error, not a process panic"
    );
    std::fs::remove_dir_all(root).ok();
}
