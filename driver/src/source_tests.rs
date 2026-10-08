//! Test modules live in their own files, which cargo-llvm-cov's default
//! filter skips, so the coverage floors measure production code alone.

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable source directory") {
        let path = entry.expect("readable directory entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Whether cargo-llvm-cov's default filename filter skips `path`, given
/// relative to the workspace root.
fn coverage_skips(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name.ends_with("-tests.rs")
        || path.parent().is_some_and(|dir| {
            dir.components().any(|part| {
                matches!(
                    part.as_os_str().to_str(),
                    Some("tests" | "examples" | "benches")
                )
            })
        })
}

/// The file that `mod name;` in `file` loads, honoring a `#[path]` override.
fn module_file(file: &Path, name: &str, path_override: Option<&str>) -> PathBuf {
    let dir = file.parent().expect("a source file sits in a directory");
    if let Some(path) = path_override {
        return dir.join(path);
    }
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let base = if matches!(stem, "lib" | "main" | "mod") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    let flat = base.join(format!("{name}.rs"));
    if flat.exists() {
        flat
    } else {
        base.join(name).join("mod.rs")
    }
}

/// The `#[cfg(test)]` modules declared in `file` that coverage would count:
/// inline ones, and out-of-line ones whose file the filter keeps.
fn counted_test_modules(root: &Path, file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).expect("readable source file");
    let lines: Vec<&str> = text.lines().collect();
    let shown = file.strip_prefix(root).unwrap_or(file);
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[cfg(test)]" {
            continue;
        }
        let mut path_override = None;
        let mut item = index + 1;
        while let Some(next) = lines.get(item).map(|line| line.trim()) {
            if let Some(path) = next
                .strip_prefix("#[path = \"")
                .and_then(|rest| rest.strip_suffix("\"]"))
            {
                path_override = Some(path);
            } else if !next.starts_with("#[") && !next.starts_with("//") {
                break;
            }
            item += 1;
        }
        let Some(declaration) = lines.get(item).map(|line| line.trim()) else {
            continue;
        };
        let declaration = declaration
            .strip_prefix("pub(crate) ")
            .or_else(|| declaration.strip_prefix("pub "))
            .unwrap_or(declaration);
        let Some(module) = declaration.strip_prefix("mod ") else {
            continue;
        };
        if let Some(name) = module.strip_suffix(" {") {
            found.push(format!(
                "{}:{}: inline `mod {name}`",
                shown.display(),
                item + 1
            ));
        } else if let Some(name) = module.strip_suffix(';') {
            let loaded = module_file(file, name, path_override);
            let relative = loaded.strip_prefix(root).unwrap_or(&loaded);
            if !coverage_skips(relative) {
                found.push(format!(
                    "{}:{}: `mod {name}` loads {}, which coverage counts",
                    shown.display(),
                    item + 1,
                    relative.display()
                ));
            }
        }
    }
    found
}

#[test]
fn test_modules_live_in_files_that_coverage_skips() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the driver sits in the workspace");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).expect("readable workspace root") {
        let member = entry.expect("readable workspace entry").path();
        if member.join("Cargo.toml").is_file() && member.join("src").is_dir() {
            sources(&member.join("src"), &mut files);
        }
    }
    sources(&root.join("build-support"), &mut files);
    files.sort();
    let found: Vec<String> = files
        .iter()
        .flat_map(|file| counted_test_modules(root, file))
        .collect();
    assert!(
        found.is_empty(),
        "move each test module into a child `tests.rs` file:\n{}",
        found.join("\n")
    );
}
