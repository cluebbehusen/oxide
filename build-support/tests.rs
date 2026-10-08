use super::*;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

struct Repository(PathBuf);
impl Repository {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "oxide-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let repo = Self(path);
        repo.run(&["init", "-q"]);
        for package in [
            "chassis", "sim", "opponent", "protocol", "kit", "net", "shell", "driver",
        ] {
            fs::create_dir_all(repo.0.join(package).join("src")).unwrap();
            fs::write(
                repo.0.join(package).join("src/lib.rs"),
                "// initial source\n",
            )
            .unwrap();
        }
        repo.commit();
        repo
    }
    fn run(&self, args: &[&str]) -> String {
        git(&self.0, args).expect("fixture git operation succeeds")
    }
    fn commit(&self) {
        self.run(&["add", "."]);
        self.run(&[
            "-c",
            "user.name=Build test",
            "-c",
            "user.email=build@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture source",
        ]);
    }
}
impl Drop for Repository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn identity_tracks_shared_and_host_sources_without_private_artifacts() {
    let repo = Repository::new();
    let paths = inputs("shell");
    let initial = identity(&repo.0, &paths);
    assert_ne!(initial.0, "unknown");
    assert_eq!(initial.1, "false");
    fs::write(repo.0.join("PRIVATE_REVIEW.md"), "private note").unwrap();
    fs::write(repo.0.join("driver/src/lib.rs"), "// other host edit").unwrap();
    assert_eq!(identity(&repo.0, &paths), initial);
    assert_eq!(identity(&repo.0, &inputs("driver")).1, "true");
    fs::write(repo.0.join("opponent/src/lib.rs"), "// changed bot policy").unwrap();
    assert_eq!(
        identity(&repo.0, &paths),
        (initial.0.clone(), "true".into())
    );
    repo.run(&["add", "opponent/src/lib.rs"]);
    assert_eq!(identity(&repo.0, &paths).1, "true");
    repo.commit();
    let committed = identity(&repo.0, &paths);
    assert_ne!(committed.0, initial.0);
    assert_eq!(committed.1, "false");
    fs::write(repo.0.join("shell/src/new.rs"), "// untracked source").unwrap();
    assert_eq!(identity(&repo.0, &paths).1, "true");
    fs::remove_file(repo.0.join("shell/src/new.rs")).unwrap();
    assert_eq!(identity(&repo.0, &paths), committed);
    fs::remove_file(repo.0.join("sim/src/lib.rs")).unwrap();
    assert_eq!(identity(&repo.0, &paths).1, "true");
}

#[test]
fn multiplayer_sources_mark_only_the_shell_dirty() {
    let repo = Repository::new();
    fs::write(repo.0.join("net/src/lib.rs"), "// changed transport").unwrap();
    assert_eq!(identity(&repo.0, &inputs("shell")).1, "true");
    assert_eq!(identity(&repo.0, &inputs("driver")).1, "false");
}

#[test]
fn an_archive_does_not_borrow_the_enclosing_repository_identity() {
    let repo = Repository::new();
    let archive = repo.0.join("archive");
    fs::create_dir(&archive).unwrap();
    assert_eq!(
        identity(&archive, &inputs("shell")),
        ("unknown".into(), "unknown".into())
    );
    assert_eq!(
        identity(&archive.join("absent"), &inputs("shell")),
        ("unknown".into(), "unknown".into())
    );
}
