//! Build-time executable provenance, shared by the two hosts.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn inputs(host: &str) -> Vec<String> {
    let mut paths: Vec<String> = [
        "Cargo.toml",
        "Cargo.lock",
        ".cargo",
        "rust-toolchain.toml",
        "build-support",
        "scenarios",
        "assets",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    paths.extend(["chassis", "sim", "bot", "protocol", "kit", host].map(str::to_owned));
    paths
}

fn identity(root: &Path, paths: &[String]) -> (String, String) {
    if git(root, &["rev-parse", "--show-toplevel"])
        .and_then(|path| PathBuf::from(path).canonicalize().ok())
        != root.canonicalize().ok()
    {
        return ("unknown".into(), "unknown".into());
    }
    let revision = git(root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let mut args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    args.extend(paths.iter().map(String::as_str));
    let dirty =
        git(root, &args).map_or(
            "unknown",
            |status| if status.is_empty() { "false" } else { "true" },
        );
    (revision, dirty.into())
}

/// Emit executable identity and its source watch inputs for Cargo.
pub fn configure(host: &str) {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let root = manifest.parent().expect("workspace package");
    let paths = inputs(host);
    for path in &paths {
        // Watching a missing optional file reruns the build script forever.
        let path = root.join(path);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    for name in [
        "HEAD".to_owned(),
        "index".to_owned(),
        "packed-refs".to_owned(),
        git(root, &["symbolic-ref", "HEAD"]).unwrap_or_default(),
    ] {
        if !name.is_empty()
            && let Some(path) = git(root, &["rev-parse", "--git-path", &name])
        {
            let path = root.join(path);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let (revision, dirty) = identity(root, &paths);
    println!("cargo:rustc-env=OXIDE_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=OXIDE_BUILD_DIRTY={dirty}");
}

#[cfg(test)]
mod tests {
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
                "chassis", "sim", "bot", "protocol", "kit", "shell", "driver",
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
        fs::write(repo.0.join("bot/src/lib.rs"), "// changed bot policy").unwrap();
        assert_eq!(
            identity(&repo.0, &paths),
            (initial.0.clone(), "true".into())
        );
        repo.run(&["add", "bot/src/lib.rs"]);
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
}
