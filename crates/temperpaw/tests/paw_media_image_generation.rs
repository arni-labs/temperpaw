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
fn paw_media_is_a_core_startup_app() {
    let root = repo_root();
    temper_platform::os_apps::set_os_apps_dir(root.join("os-apps"));
    let startup_apps = temper_platform::os_apps::list_startup_os_apps();

    assert!(
        startup_apps.iter().any(|app| app == "paw-media"),
        "paw-media should be installed as a core startup app"
    );
}

#[test]
fn paw_media_offers_openrouter_as_a_second_image_provider() {
    let root = repo_root();
    let app = read(root.join("os-apps/paw-media/app.toml"));
    let spec_source = read(root.join("os-apps/paw-media/specs/media_generation.ioa.toml"));
    let csdl = read(root.join("os-apps/paw-media/specs/model.csdl.xml"));
    let build_script = read(root.join("os-apps/paw-media/wasm/build.sh"));
    let wasm = read(root.join("os-apps/paw-media/wasm/openrouter_image_generate/src/lib.rs"));

    // An undeclared module is never uploaded, and every trigger for it fails.
    assert!(
        app.contains("name = \"openrouter_image_generate\""),
        "app.toml must declare the OpenRouter module"
    );

    // Reading requests (they may hold a member's draft prompt) and spending
    // OpenRouter credit are for the platform's own principals only.
    let policy = read(root.join("os-apps/paw-media/policies/media_generation.cedar"));
    let forbid = policy
        .split("forbid(")
        .nth(1)
        .expect("media policy must forbid reads and OpenRouter spending for other principals");
    for needle in [
        "Action::\"read\"",
        "Action::\"list\"",
        "Action::\"GenerateWithOpenRouter\"",
        ") when {",
        "principal is Customer && !(principal has role && [\"owner\", \"curator\"].contains(principal.role))",
        "principal.agent_type == \"contributor\"",
    ] {
        assert!(
            forbid.contains(needle),
            "media policy forbid should contain {needle}"
        );
    }

    // The picture's File is created before the paid call, so a permission or
    // workspace problem fails before any OpenRouter credit is spent.
    let body = wasm
        .split("fn generate_and_store(")
        .nth(1)
        .expect("OpenRouter renderer must have generate_and_store");
    let created = body
        .find("create_image_file(")
        .expect("generate_and_store must create the File");
    let paid = body
        .find("call_openrouter(")
        .expect("generate_and_store must call OpenRouter");
    assert!(
        created < paid,
        "the File must be created before the paid OpenRouter call"
    );
    assert!(
        build_script.contains("openrouter_image_generate"),
        "build.sh must build and package the OpenRouter module"
    );

    let spec = temper_spec::automaton::parse_automaton(&spec_source).expect("media spec parses");
    let action = spec
        .actions
        .iter()
        .find(|a| a.name == "GenerateWithOpenRouter")
        .expect("GenerateWithOpenRouter action");
    assert_eq!(
        action.from,
        vec!["Created".to_string(), "Failed".to_string()]
    );
    assert_eq!(
        action.to.as_deref(),
        Some("Generating"),
        "OpenRouter skips the Codex auth gate"
    );
    for needle in [
        "module = \"openrouter_image_generate\"",
        "openrouter_api_key = \"{secret:openrouter_api_key}\"",
        "default_model = \"x-ai/grok-imagine-image-2.0\"",
    ] {
        assert!(
            spec_source.contains(needle),
            "media spec should contain {needle}"
        );
    }
    let model = temper_spec::csdl::parse_csdl(&csdl).expect("media CSDL parses");
    let actions: Vec<String> = model
        .schemas
        .iter()
        .flat_map(|s| s.actions.iter().map(|a| a.name.clone()))
        .collect();
    assert!(
        actions.iter().any(|a| a == "GenerateWithOpenRouter"),
        "CSDL must expose GenerateWithOpenRouter, got {actions:?}"
    );

    for needle in [
        "https://openrouter.ai/api/v1/images",
        "b64_json",
        "RecordResult",
        "RecordError",
        "streaming_call",
    ] {
        assert!(
            wasm.contains(needle),
            "OpenRouter renderer should contain {needle}"
        );
    }
}
