use super::*;

#[test]
fn bundle_probe_finds_mac_and_flat_ios_layouts() {
    let root = std::env::temp_dir().join(format!("oxide-bundle-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let atlas = |dir: &Path| {
        let sprites = dir.join("assets/sprites");
        std::fs::create_dir_all(&sprites).unwrap();
        std::fs::write(sprites.join("atlas.png"), b"").unwrap();
    };
    let mac = root.join("Oxide.app/Contents");
    std::fs::create_dir_all(mac.join("MacOS")).unwrap();
    atlas(&mac.join("Resources"));
    let found = bundle_resources_beside(&mac.join("MacOS")).expect("mac bundle");
    assert!(found.ends_with("MacOS/../Resources"));

    let ios = root.join("Oxide-ios.app");
    atlas(&ios);
    assert_eq!(bundle_resources_beside(&ios), Some(ios.clone()));

    let workspace = root.join("target/debug");
    std::fs::create_dir_all(&workspace).unwrap();
    assert_eq!(bundle_resources_beside(&workspace), None);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn derived_dirs_hang_off_the_one_data_root() {
    if let Some(data) = data_dir() {
        assert_eq!(autosave_dir(), Some(data.join("autosaves")));
        assert_eq!(saves_dir(), Some(data.join("saves")));
    }
}

#[test]
fn a_workspace_run_keeps_the_cwd_relative_replays_dir() {
    // Test binaries never sit inside a bundle, so every documented
    // `driver` invocation keeps reading the local replays/ dir.
    assert_eq!(replays_dir(), PathBuf::from("replays"));
}
