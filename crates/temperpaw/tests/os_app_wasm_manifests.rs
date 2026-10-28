//! Every WASM module an os-app's specs trigger is one the app installs.
//!
//! The installer registers only the modules an app declares in `app.toml`
//! `[[wasm_modules]]`. It skips an undeclared artifact with a warning, so a
//! trigger naming that module fails later with "WASM module not found". A
//! trigger may name a module its own app or one of its dependencies declares.
//! The rule covers apps that ship a WASM build; an app with no build ships no
//! modules at all, which declaring them would not change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
struct AppManifest {
    name: String,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    wasm_modules: Vec<WasmModule>,
}

#[derive(serde::Deserialize)]
struct WasmModule {
    name: String,
}

struct App {
    dir: PathBuf,
    manifest: AppManifest,
}

fn os_apps_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../os-apps")
}

fn read_apps() -> BTreeMap<String, App> {
    let mut apps = BTreeMap::new();
    for entry in std::fs::read_dir(os_apps_dir()).expect("os-apps should be readable") {
        let dir = entry.expect("os-apps entry should be readable").path();
        let manifest_path = dir.join("app.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let source = std::fs::read_to_string(&manifest_path).expect("app.toml should be readable");
        let manifest: AppManifest = toml::from_str(&source)
            .unwrap_or_else(|error| panic!("{} should parse: {error}", manifest_path.display()));
        apps.insert(manifest.name.clone(), App { dir, manifest });
    }
    assert!(!apps.is_empty(), "os-apps should hold at least one app");
    apps
}

/// Collects every module a WASM trigger or integration in a spec names.
fn collect_triggered_modules(value: &toml::Value, modules: &mut BTreeSet<String>) {
    match value {
        toml::Value::Table(table) => {
            let is_wasm = table
                .get("kind")
                .and_then(toml::Value::as_str)
                .is_none_or(|kind| kind == "wasm");
            if is_wasm && let Some(module) = table.get("module").and_then(toml::Value::as_str) {
                modules.insert(module.to_string());
            }
            for child in table.values() {
                collect_triggered_modules(child, modules);
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                collect_triggered_modules(item, modules);
            }
        }
        _ => {}
    }
}

fn triggered_modules(app: &App) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(app.dir.join("specs")) else {
        return modules;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.to_string_lossy().ends_with(".ioa.toml") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("spec should be readable");
        let spec: toml::Value = toml::from_str(&source)
            .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()));
        collect_triggered_modules(&spec, &mut modules);
    }
    modules
}

#[test]
fn the_trigger_scan_finds_the_cron_job_module() {
    let apps = read_apps();
    let paw_agent = apps
        .get("paw-agent")
        .expect("paw-agent should be an os-app");
    assert!(
        triggered_modules(paw_agent).contains("cron_compute_next"),
        "the scan should find the CronJob spec's cron_compute_next trigger"
    );
}
