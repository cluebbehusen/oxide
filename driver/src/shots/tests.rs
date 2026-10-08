use super::{ReferenceResult, adopt_reference_set, check_reference};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Result<Self> {
        loop {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("oxide-shots-test-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn missing_reference_fails_without_adopting_capture() -> Result<()> {
    let dir = TestDir::new()?;
    let run = dir.0.join("run/example.png");
    let reference = dir.0.join("ref/example.png");
    std::fs::create_dir_all(run.parent().expect("has parent"))?;
    std::fs::write(&run, b"current capture")?;

    assert!(matches!(
        check_reference(&run, &reference)?,
        ReferenceResult::Missing
    ));
    assert!(!reference.exists());
    assert_eq!(std::fs::read(run)?, b"current capture");
    Ok(())
}

#[test]
fn bless_explicitly_adopts_the_complete_capture_set() -> Result<()> {
    let dir = TestDir::new()?;
    std::fs::create_dir_all(dir.0.join("run"))?;
    std::fs::create_dir_all(dir.0.join("ref"))?;
    std::fs::write(dir.0.join("run/a.png"), b"new a")?;
    std::fs::write(dir.0.join("run/b.png"), b"new b")?;
    std::fs::write(dir.0.join("ref/a.png"), b"old a")?;

    adopt_reference_set(&dir.0, &["a".to_string(), "b".to_string()])?;

    assert_eq!(std::fs::read(dir.0.join("ref/a.png"))?, b"new a");
    assert_eq!(std::fs::read(dir.0.join("ref/b.png"))?, b"new b");
    Ok(())
}

#[test]
fn failed_staging_leaves_the_old_reference_set_untouched() -> Result<()> {
    let dir = TestDir::new()?;
    std::fs::create_dir_all(dir.0.join("run"))?;
    std::fs::create_dir_all(dir.0.join("ref"))?;
    std::fs::write(dir.0.join("run/a.png"), b"new a")?;
    std::fs::write(dir.0.join("ref/a.png"), b"old a")?;

    assert!(adopt_reference_set(&dir.0, &["a".to_string(), "missing".to_string()]).is_err());
    assert_eq!(std::fs::read(dir.0.join("ref/a.png"))?, b"old a");
    Ok(())
}
