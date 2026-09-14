//! Recording-build identity, outside deterministic state.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn main() {
    for tree in [
        "src",
        "../sim/src",
        "../chassis/src",
        "../shell/src",
        "../protocol/src",
        "../Cargo.lock",
        "../scenarios",
    ] {
        println!("cargo:rerun-if-changed={tree}");
    }
    for name in [
        "HEAD".to_owned(),
        git(&["symbolic-ref", "HEAD"]).unwrap_or_default(),
    ] {
        if !name.is_empty()
            && let Some(path) = git(&["rev-parse", "--git-path", &name])
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!(
        "cargo:rustc-env=OXIDE_BUILD_REVISION={}",
        git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into())
    );
    println!(
        "cargo:rustc-env=OXIDE_BUILD_DIRTY={}",
        git(&["status", "--porcelain"]).map_or("unknown", |s| if s.is_empty() {
            "false"
        } else {
            "true"
        })
    );
}
