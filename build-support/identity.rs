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
    paths.extend(["chassis", "sim", "opponent", "protocol", "kit", host].map(str::to_owned));
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
mod tests;
