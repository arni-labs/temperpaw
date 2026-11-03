use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate should live under repo/crates/temperpaw")
        .to_path_buf()
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
