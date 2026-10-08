use super::*;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("chassis-fsx-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temps_in(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".tmp."))
        })
        .collect()
}

#[test]
fn a_bare_path_uses_the_current_directory_as_its_parent() {
    assert_eq!(parent_dir(Path::new("session.json")), Path::new("."));
}

#[test]
fn writing_twice_to_one_path_keeps_the_second_payload() {
    // The cross-platform replace contract, pinned on every CI OS:
    // a rewrite lands and its content wins.
    let dir = scratch("twice");
    let path = dir.join("record.json");
    write_atomic::<std::io::Error, _>(&path, |w| w.write_all(b"first")).unwrap();
    write_atomic::<std::io::Error, _>(&path, |w| w.write_all(b"second")).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    assert!(temps_in(&dir).is_empty(), "no temp survives a success");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_write_leaves_no_temp_behind() {
    // The closure fails mid-write: the guard must reap the temp.
    let dir = scratch("closure-err");
    let path = dir.join("record.json");
    let result = write_atomic::<std::io::Error, _>(&path, |w| {
        w.write_all(b"partial")?;
        Err(std::io::Error::other("serialization refused"))
    });
    assert!(result.is_err());
    assert!(!path.exists(), "nothing was published");
    assert!(temps_in(&dir).is_empty(), "the temp was removed");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_rewrite_preserves_the_previous_record() {
    let dir = scratch("preserve-on-error");
    let path = dir.join("record.json");
    std::fs::write(&path, b"complete old record").unwrap();

    let result = write_atomic::<std::io::Error, _>(&path, |writer| {
        writer.write_all(b"truncated replacement")?;
        Err(std::io::Error::other("serialization refused"))
    });

    assert!(result.is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"complete old record",
        "a failed save must not damage the last durable record"
    );
    assert!(temps_in(&dir).is_empty(), "the failed temp was removed");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_rename_leaves_no_temp_behind() {
    // The destination exists as a directory, so the final rename
    // fails after the temp was fully written.
    let dir = scratch("rename-err");
    let path = dir.join("record.json");
    std::fs::create_dir_all(&path).unwrap();
    let result = write_atomic::<std::io::Error, _>(&path, |w| w.write_all(b"payload"));
    assert!(result.is_err());
    assert!(temps_in(&dir).is_empty(), "the temp was removed");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_sweep_reaps_only_old_temps() {
    let dir = scratch("sweep");
    std::fs::write(dir.join("orphan.tmp.999.0"), b"stale").unwrap();
    std::fs::write(dir.join("keeper.json"), b"real").unwrap();
    // Zero threshold: everything temp-named is old enough.
    assert_eq!(sweep_temps(&dir, Duration::ZERO), 1);
    assert!(dir.join("keeper.json").exists(), "real files never swept");
    // A fresh temp under a real threshold is a write in flight.
    std::fs::write(dir.join("inflight.tmp.999.1"), b"live").unwrap();
    assert_eq!(sweep_temps(&dir, Duration::from_secs(3600)), 0);
    assert!(dir.join("inflight.tmp.999.1").exists());
    std::fs::remove_dir_all(&dir).ok();
}
