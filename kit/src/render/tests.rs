use super::*;

fn scratch(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("oxide-kit-render-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&path).ok();
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn save_png_creates_parents_and_writes_the_canonical_render() {
    let dir = scratch("save");
    let path = dir.join("nested/state.png");
    let state = oxide_sim::Scenario::skirmish().build().unwrap();

    save_png(&state, &path).unwrap();

    let saved = std::fs::read(&path).unwrap();
    assert_eq!(saved, png_bytes(&state).unwrap());
    let decoded = Pixmap::decode_png(&saved).expect("saved bytes are a PNG");
    assert_eq!(
        decoded.width(),
        u32::try_from(state.map().width()).unwrap() * TILE_PIXELS
    );
    assert_eq!(
        decoded.height(),
        u32::try_from(state.map().height()).unwrap() * TILE_PIXELS
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn an_extractor_frame_draws_as_a_gold_rimmed_block_unlike_rock() {
    let state = oxide_sim::Scenario::skirmish().build().unwrap();
    let frame = *state
        .map()
        .extractor_frames()
        .iter()
        .find(|frame| {
            !state
                .buildings()
                .iter()
                .any(|building| building.anchor == **frame)
        })
        .expect("an open extractor frame");
    let pixmap = render_state(&state);
    let at = |dx: u32, dy: u32| {
        let x = u32::try_from(frame.x).unwrap() * TILE_PIXELS + dx;
        let y = u32::try_from(frame.y).unwrap() * TILE_PIXELS + dy;
        let p = pixmap.pixel(x, y).expect("inside the map");
        (u32::from(p.red()) << 16) | (u32::from(p.green()) << 8) | u32::from(p.blue())
    };
    assert_eq!(at(1, 1), SCRAP_FULL, "the rim");
    assert_eq!(at(TILE_PIXELS, TILE_PIXELS), FRAME, "the block");
    assert_ne!(FRAME, ROCK);
}

#[test]
fn save_png_reports_a_parent_that_cannot_be_created() {
    let dir = scratch("bad-parent");
    let blocked = dir.join("not-a-directory");
    std::fs::write(&blocked, b"existing file").unwrap();
    let state = oxide_sim::Scenario::skirmish().build().unwrap();

    assert!(save_png(&state, &blocked.join("state.png")).is_err());
    assert_eq!(std::fs::read(&blocked).unwrap(), b"existing file");
    std::fs::remove_dir_all(dir).ok();
}
