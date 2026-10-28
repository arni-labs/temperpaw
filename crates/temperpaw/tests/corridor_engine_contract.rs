//! Contract tests for the corridor world engine (paw-foresight 0.2).
//!
//! See os-apps/paw-foresight/adrs/002-corridor-world-engine.md and
//! docs/corridor-world-engine-rfc.md. These tests pin the entity surface
//! that WASM modules, souls, and Deep Sci-Fi 2.0 depend on.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.as_ref().display()))
}

fn parse_spec(path: &Path) -> toml::Value {
    read(path)
        .parse::<toml::Value>()
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()))
}

const CORRIDOR_WASM_MODULES: &[&str] = &[
    "seed_world",
    "sample_endpoints",
    "decompose_endpoint",
    "spawn_repairers",
    "spawn_adversaries",
    "aggregate_costs",
    "evidence_ingest",
    "register_forecasts",
    "render_artifacts",
    "consistency_gate",
    "grade_hindcast",
    "animate_dwellers",
    "adjudicate_nodes",
];

#[test]
fn entity_type_names_are_unique_across_apps() {
    // The tenant CSDL is merged across all installed apps and install_os_app
    // upserts specs by entity-type name: a duplicate name in two apps means
    // whichever installs second silently overwrites the other's automaton.
    //
    // Pre-existing collision allowlist — tracked for removal, do NOT add to it:
    //   WorkCycle: paw-harness vs paw-patrol (predates the corridor engine).
    let allowlist: BTreeSet<&str> = ["WorkCycle"].into_iter().collect();

    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let apps_dir = repo_root().join("os-apps");
    for app in fs::read_dir(&apps_dir).expect("read os-apps") {
        let app = app.expect("os-app entry").path();
        let specs = app.join("specs");
        if !specs.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&specs).expect("read specs dir") {
            let path = entry.expect("spec entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".ioa.toml"))
                .unwrap_or(false)
            {
                continue;
            }
            let spec = parse_spec(&path);
            if let Some(name) = spec
                .get("automaton")
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
            {
                owners
                    .entry(name.to_string())
                    .or_default()
                    .push(path.display().to_string());
            }
        }
    }

    let collisions: Vec<_> = owners
        .iter()
        .filter(|(name, paths)| paths.len() > 1 && !allowlist.contains(name.as_str()))
        .collect();
    assert!(
        collisions.is_empty(),
        "duplicate entity-type names across apps (merged-CSDL overwrite hazard): {collisions:?}"
    );
}

#[test]
fn csdl_serves_all_corridor_entity_sets() {
    // The OData layer serves entity sets from specs/model.csdl.xml, not from
    // the .ioa.toml automatons — a spec without a CSDL entry registers but
    // never serves (found the hard way during the A1 boot gate).
    let csdl = read(repo_root().join("os-apps/paw-foresight/specs/model.csdl.xml"));
    for set in [
        "Worlds",
        "EventNodes",
        "Endpoints",
        "Claims",
        "Paths",
        "Artifacts",
        "Dwellers",
        "Forecasts",
        "Hindcasts",
        "Lenses",
        // Legacy sets remain served until the ADR-003 removal commit.
        "Projections",
        "ForesightModels",
        "Observations",
        "Directions",
    ] {
        assert!(
            csdl.contains(&format!("EntitySet Name=\"{set}\"")),
            "model.csdl.xml must declare EntitySet {set}"
        );
    }
}

#[test]
fn corridor_wasm_modules_are_packaged_for_core_startup() {
    let root = repo_root();
    let dockerfile = read(root.join("Dockerfile"));
    let ci = read(root.join(".github/workflows/ci.yml"));
    let identity_contract =
        read(root.join("crates/temperpaw/tests/temperpaw_identity_contract.rs"));
    let build_script = read(root.join("os-apps/paw-foresight/wasm/build.sh"));

    assert!(
        dockerfile.contains("cd /app/os-apps/paw-foresight/wasm && bash build.sh"),
        "Dockerfile must build paw-foresight WASM before copying os-apps into the runtime image"
    );
    assert!(
        ci.contains("os-apps/paw-foresight/wasm/build.sh"),
        "CI must build paw-foresight WASM so fresh core startup images do not miss corridor modules"
    );
    assert!(
        identity_contract.contains("\"os-apps/paw-foresight/wasm/build.sh\""),
        "identity contract should keep paw-foresight in the audited WASM build-script set"
    );

    for module in CORRIDOR_WASM_MODULES {
        assert!(
            build_script.contains(module),
            "paw-foresight build.sh should build {module}"
        );
    }

    // Drive packaging: a loop need not spell every output filename literally.
    let output = std::process::Command::new("bash")
        .arg(root.join("scripts/test-wasm-build-artifacts.sh"))
        .output()
        .expect("WASM artifact regression should run");
    assert!(
        output.status.success(),
        "WASM artifact packaging failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
