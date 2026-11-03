use std::{
    fs,
    path::{Path, PathBuf},
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn os_app_wasm_build_scripts_preserve_temper_host_imports() {
    let root = repo_root();
    let build_env = fs::read_to_string(root.join("os-apps/wasm-build-env.sh"))
        .expect("shared os-app WASM build environment should be readable");
    assert!(
        build_env.contains("link-arg=--allow-undefined"),
        "Temper WASM guest builds must preserve unresolved host functions as imports"
    );

    for script_path in [
        "os-apps/paw-agent/wasm/build.sh",
        "os-apps/paw-channels/wasm/build.sh",
        "os-apps/paw-fs/wasm/artifact_batch_apply/build.sh",
        "os-apps/paw-fs/wasm/blob_adapter/build.sh",
        "os-apps/paw-fs/wasm/workspace_fs/build.sh",
        "os-apps/paw-foresight/wasm/build.sh",
        "os-apps/paw-ingest/wasm/build.sh",
        "os-apps/paw-managed-agents/wasm/build.sh",
        "os-apps/paw-media/wasm/build.sh",
        "os-apps/paw-patrol/wasm/build.sh",
        "os-apps/paw-research/wasm/build.sh",
        "os-apps/paw-skills/wasm/build.sh",
    ] {
        let script = fs::read_to_string(root.join(script_path))
            .unwrap_or_else(|err| panic!("failed to read {script_path}: {err}"));
        assert!(
            script.contains("wasm-build-env.sh"),
            "{script_path} must source os-apps/wasm-build-env.sh so Temper host imports link in CI and deployment builds"
        );
    }
}
