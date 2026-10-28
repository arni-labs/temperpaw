use std::{fs, path::PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate should live under repo/crates/temperpaw")
        .to_path_buf()
}

fn read(path: impl Into<PathBuf>) -> String {
    let path = path.into();
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

#[test]
fn paw_media_wasm_is_built_into_ci_and_production_images() {
    let root = repo_root();
    let dockerfile = read(root.join("Dockerfile"));
    let ci = read(root.join(".github/workflows/ci.yml"));
    let identity_contract =
        read(root.join("crates/temperpaw/tests/temperpaw_identity_contract.rs"));
    let build_script = read(root.join("os-apps/paw-media/wasm/build.sh"));

    assert!(
        dockerfile.contains("cd /app/os-apps/paw-media/wasm && bash build.sh"),
        "Dockerfile must build paw-media WASM before copying os-apps into the runtime image"
    );
    assert!(
        ci.contains("os-apps/paw-media/wasm/build.sh"),
        "CI must build paw-media WASM so missing renderer packaging fails before deploy"
    );
    assert!(
        identity_contract.contains("\"os-apps/paw-media/wasm/build.sh\""),
        "identity contract should keep paw-media in the audited WASM build-script set"
    );
    assert!(
        build_script.contains("openai_codex_image_generate"),
        "paw-media build.sh must build openai_codex_image_generate; artifact packaging is exercised by the shared regression"
    );
}

#[test]
fn paw_media_is_a_core_startup_app() {
    let root = repo_root();
    temper_platform::os_apps::set_os_apps_dir(root.join("os-apps"));
    let startup_apps = temper_platform::os_apps::list_startup_os_apps();

    assert!(
        startup_apps.iter().any(|app| app == "paw-media"),
        "paw-media should be installed as a core startup app"
    );
}
