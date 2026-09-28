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
    paths.extend(["chassis", "sim", "bot", "opponent", "protocol", "kit", host].map(str::to_owned));
    // Only the shell links the multiplayer crate.
    if host == "shell" {
        paths.push("net".to_owned());
    }
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

/// Packages whose compiled sources decide how the frozen `oxide-bot` reference
/// plays: the controller and the simulation it runs on.
const REFERENCE_PACKAGES: [&str; 3] = ["bot", "sim", "chassis"];

/// Other files that decide how the reference plays: the host code that seats
/// it and collects its commands, and the locked dependency versions.
const REFERENCE_FILES: [&str; 3] = [
    "kit/src/controller.rs",
    "kit/src/bot_execution.rs",
    "Cargo.lock",
];

/// Digest of the [`REFERENCE_PACKAGES`] manifests and sources and the
/// [`REFERENCE_FILES`] as they are on disk. Evaluation reuses reference results
/// for as long as it is unchanged.
fn reference_digest(root: &Path) -> String {
    fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                collect(&path, files);
            } else {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for package in REFERENCE_PACKAGES {
        let package = root.join(package);
        files.push(package.join("Cargo.toml"));
        collect(&package.join("src"), &mut files);
    }
    files.extend(REFERENCE_FILES.map(|file| root.join(file)));
    let mut named: Vec<(String, PathBuf)> = files
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (name, path)
        })
        .collect();
    named.sort();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (name, path) in named {
        let contents = std::fs::read(&path).unwrap_or_default();
        feed(name.as_bytes());
        feed(&[0]);
        feed(&(contents.len() as u64).to_le_bytes());
        feed(&contents);
    }
    format!("fnv1a64:{hash:016x}")
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
    println!(
        "cargo:rustc-env=OXIDE_REFERENCE_DIGEST={}",
        reference_digest(root)
    );
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
                "chassis", "sim", "bot", "opponent", "protocol", "kit", "net", "shell", "driver",
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
    fn multiplayer_sources_mark_only_the_shell_dirty() {
        let repo = Repository::new();
        fs::write(repo.0.join("net/src/lib.rs"), "// changed transport").unwrap();
        assert_eq!(identity(&repo.0, &inputs("shell")).1, "true");
        assert_eq!(identity(&repo.0, &inputs("driver")).1, "false");
    }

    #[test]
    fn the_reference_digest_follows_only_reference_inputs() {
        let repo = Repository::new();
        let initial = reference_digest(&repo.0);
        assert!(initial.starts_with("fnv1a64:"));
        assert_eq!(reference_digest(&repo.0), initial);
        fs::write(repo.0.join("driver/src/lib.rs"), "// driver edit").unwrap();
        fs::write(repo.0.join("opponent/src/lib.rs"), "// new bot edit").unwrap();
        fs::write(repo.0.join("kit/src/lib.rs"), "// host edit").unwrap();
        fs::write(repo.0.join("bot/README.md"), "notes").unwrap();
        fs::write(repo.0.join("bot/src/.lib.rs.swp"), "editor state").unwrap();
        assert_eq!(reference_digest(&repo.0), initial);
        fs::create_dir_all(repo.0.join("bot/src/nested")).unwrap();
        fs::write(repo.0.join("bot/src/nested/new.rs"), "// new module").unwrap();
        let added = reference_digest(&repo.0);
        assert_ne!(added, initial);
        fs::write(repo.0.join("bot/src/lib.rs"), "// changed policy").unwrap();
        assert_ne!(reference_digest(&repo.0), added);
        fs::write(repo.0.join("bot/src/lib.rs"), "// initial source\n").unwrap();
        assert_eq!(reference_digest(&repo.0), added);
        for package in ["sim", "chassis"] {
            let source = repo.0.join(package).join("src/lib.rs");
            fs::write(&source, "// changed rules").unwrap();
            assert_ne!(reference_digest(&repo.0), added, "{package} source");
            fs::write(&source, "// initial source\n").unwrap();
            let manifest = repo.0.join(package).join("Cargo.toml");
            fs::write(&manifest, "[package]").unwrap();
            assert_ne!(reference_digest(&repo.0), added, "{package} manifest");
            fs::remove_file(&manifest).unwrap();
            assert_eq!(reference_digest(&repo.0), added);
        }
        for file in REFERENCE_FILES {
            let path = repo.0.join(file);
            fs::write(&path, "changed host").unwrap();
            assert_ne!(reference_digest(&repo.0), added, "{file}");
            fs::remove_file(&path).unwrap();
            assert_eq!(reference_digest(&repo.0), added);
        }
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
