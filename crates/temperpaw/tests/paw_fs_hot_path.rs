use std::fs;
use std::path::{Path, PathBuf};

const OLD_TEMPER_REV: &str = "c584a52b59924e66502576646f50131b0d763a2a";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("temperpaw crate should live under crates/temperpaw")
        .to_path_buf()
}

fn repo_file(path: &str) -> String {
    let root = repo_root();
    fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The Temper rev the server builds against: temper-platform's pin in crates/temperpaw/Cargo.toml.
fn server_temper_rev() -> String {
    repo_file("crates/temperpaw/Cargo.toml")
        .lines()
        .find(|line| {
            line.starts_with("temper-platform = ") && line.contains("arni-labs/temper.git")
        })
        .and_then(|line| line.split("rev = \"").nth(1))
        .and_then(|rest| rest.get(..40))
        .map(str::to_string)
        .expect("crates/temperpaw/Cargo.toml must pin temper-platform to an arni-labs/temper rev")
}

fn collect_cargo_tomls(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let entry = entry.unwrap_or_else(|e| panic!("read_dir entry {}: {e}", dir.display()));
        let file_type = entry
            .file_type()
            .unwrap_or_else(|e| panic!("file_type {}: {e}", entry.path().display()));
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_cargo_tomls(&path, out);
        } else if file_type.is_file() && path.file_name().is_some_and(|name| name == "Cargo.toml") {
            out.push(path);
        }
    }
}

#[test]
fn packaged_wasm_sdk_pins_match_temper_dependency_revision() {
    let root = repo_root();
    let expected_rev = server_temper_rev();
    let mut manifests = Vec::new();
    collect_cargo_tomls(&root.join("os-apps"), &mut manifests);

    let mut stale = Vec::new();
    for manifest in manifests {
        let source = fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
        if source.contains("temper-wasm-sdk")
            && (source.contains(OLD_TEMPER_REV) || !source.contains(&expected_rev))
        {
            stale.push(
                manifest
                    .strip_prefix(&root)
                    .unwrap_or(&manifest)
                    .display()
                    .to_string(),
            );
        }
    }

    assert!(
        stale.is_empty(),
        "packaged WASM manifests must pin temper-wasm-sdk to the server's Temper rev {expected_rev}; stale: {stale:?}"
    );
}
