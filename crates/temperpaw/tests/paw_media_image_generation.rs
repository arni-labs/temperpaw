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
fn paw_media_declares_codex_subscription_image_generation_app() {
    let root = repo_root();
    let app = read(root.join("os-apps/paw-media/app.toml"));
    let spec = read(root.join("os-apps/paw-media/specs/media_generation.ioa.toml"));
    let model = read(root.join("os-apps/paw-media/specs/model.csdl.xml"));
    let policy = read(root.join("os-apps/paw-media/policies/media_generation.cedar"));
    let wasm = read(root.join("os-apps/paw-media/wasm/openai_codex_image_generate/src/lib.rs"));

    for needle in [
        "name = \"paw-media\"",
        "startup_install = \"core\"",
        "dependencies = [\"paw-agent\", \"paw-fs\"]",
        "name = \"openai_codex_image_generate\"",
    ] {
        assert!(
            app.contains(needle),
            "paw-media app.toml should contain {needle}"
        );
    }

    for needle in [
        "name = \"MediaGenerationRequest\"",
        "states = [\"Created\", \"Authorizing\", \"Generating\", \"Storing\", \"Complete\", \"Failed\"]",
        "media_type",
        "provider",
        "Generate",
        "RecordAuthReady",
        "RecordResult",
        "RecordError",
        "module = \"openai_codex_image_generate\"",
        "module = \"provider_auth_gate\"",
        "auth_action = \"EnsureFresh\"",
    ] {
        assert!(
            spec.contains(needle),
            "MediaGeneration spec should contain {needle}"
        );
    }

    for needle in [
        "<EntityType Name=\"MediaGenerationRequest\">",
        "<EntitySet Name=\"MediaGenerationRequests\"",
        "<Action Name=\"Generate\"",
        "<Action Name=\"RecordResult\"",
        "<Property Name=\"ResultFileId\"",
        "<Property Name=\"ProviderResponseId\"",
    ] {
        assert!(
            model.contains(needle),
            "paw-media CSDL should contain {needle}"
        );
    }

    for needle in [
        "resource is MediaGenerationRequest",
        "Action::\"Generate\"",
        "Action::\"RecordResult\"",
        "Action::\"RecordError\"",
        "context.module == \"openai_codex_image_generate\"",
        "Action::\"http_call\"",
        "Action::\"access_secret\"",
    ] {
        assert!(
            policy.contains(needle),
            "paw-media Cedar should contain {needle}"
        );
    }

    for needle in [
        "chatgpt.com/backend-api/codex/responses",
        "\"type\": \"image_generation\"",
        "\"action\": \"generate\"",
        "image_generation_call",
        "RecordResult",
        "Files",
    ] {
        assert!(
            wasm.contains(needle),
            "Codex image WASM should contain {needle}"
        );
    }
}

#[test]
fn image_generation_tool_is_exposed_through_default_agent_tools() {
    let root = repo_root();
    let tool_catalog = read(root.join("os-apps/paw-agent/wasm/tool-catalog/src/lib.rs"));
    let dispatch = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/dispatch.rs"));
    let paw_agent_manual = read(root.join("os-apps/paw-agent/agents/paw/AGENT.md"));
    let paw_agent_app = read(root.join("os-apps/paw-agent/app.toml"));
    let startup = read(root.join("crates/temperpaw/src/startup.rs"));
    let setup_api = read(root.join("crates/temperpaw/src/setup_api.rs"));
    let session_spec = read(root.join("os-apps/paw-agent/specs/session.ioa.toml"));
    let cron_spec = read(root.join("os-apps/paw-agent/specs/cron_job.ioa.toml"));
    let route_message = read(root.join("os-apps/paw-channels/wasm/route_message/src/lib.rs"));

    for (label, source) in [
        ("tool catalog", tool_catalog.as_str()),
        ("startup defaults", startup.as_str()),
        ("setup API defaults", setup_api.as_str()),
        ("Session tools default", session_spec.as_str()),
        ("CronJob tools default", cron_spec.as_str()),
        ("channel route defaults", route_message.as_str()),
    ] {
        assert!(
            source.contains("temper_image_generate"),
            "{label} should include temper_image_generate in default tools"
        );
    }

    for needle in [
        "method: \"image_generate\"",
        "token: Some(\"temper_image_generate\")",
        "generate an image through the paw-media app",
        "Use this tool for user image requests",
        "gpt-image-2",
    ] {
        assert!(
            tool_catalog.contains(needle),
            "tool catalog should expose {needle}"
        );
    }

    for needle in [
        "\"image_generate\" => Some(\"temper_image_generate\")",
        "\"image_generate\" => temper_image_generate",
        "/tdata/MediaGenerationRequests",
        "Temper.Generate?await_integration=true",
        "__temperpaw_image",
        "unwrap_or_else(|| \"low\".to_string())",
    ] {
        assert!(
            dispatch.contains(needle),
            "Monty dispatch should contain {needle}"
        );
    }

    for needle in [
        "temper.image_generate",
        "For user image requests, call this tool",
        "gpt-image-*",
    ] {
        assert!(
            paw_agent_manual.contains(needle),
            "Paw operating manual should contain {needle}"
        );
    }

    assert!(
        paw_agent_app.contains("name = \"openai_codex_auth\""),
        "paw-agent app.toml must declare openai_codex_auth so provider_auth_gate can run Codex subscription auth"
    );
}

#[test]
fn image_generation_uses_app_scoped_entity_set_route() {
    let root = repo_root();
    let model = read(root.join("os-apps/paw-media/specs/model.csdl.xml"));
    let dispatch = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/dispatch.rs"));
    let wasm = read(root.join("os-apps/paw-media/wasm/openai_codex_image_generate/src/lib.rs"));

    assert!(
        model.contains("<EntitySet Name=\"MediaGenerationRequests\" EntityType=\"TemperPaw.Media.MediaGenerationRequest\"/>"),
        "paw-media must expose an app-scoped entity set and entity type so it does not collide with legacy root MediaGeneration"
    );

    for (label, source) in [
        ("Monty dispatch", dispatch.as_str()),
        ("Codex image WASM", wasm.as_str()),
    ] {
        assert!(
            source.contains("/tdata/MediaGenerationRequests"),
            "{label} should call the app-scoped media entity set"
        );
        assert!(
            !source.contains("/tdata/MediaGenerations"),
            "{label} must not call the legacy root MediaGenerations route"
        );
    }
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
fn paw_media_policy_limits_result_callbacks_to_runtime_modules() {
    let root = repo_root();
    let policy = read(root.join("os-apps/paw-media/policies/media_generation.cedar"));

    let user_actions = r#"action in [
    Action::"create",
    Action::"read",
    Action::"list",
    Action::"Generate",
    Action::"GenerateWithOpenRouter"
  ]"#;
    assert!(
        policy.contains(user_actions),
        "user-facing MediaGenerationRequest policy should only expose create/read/list and the two Generate actions"
    );
    for forbidden in [
        "Action::\"RecordAuthReady\"",
        "Action::\"RecordStoring\"",
        "Action::\"RecordResult\"",
        "Action::\"RecordError\"",
    ] {
        assert!(
            !policy
                .split("resource is MediaGenerationRequest")
                .next()
                .unwrap_or_default()
                .contains(forbidden),
            "callback action {forbidden} must not be in the broad user-facing permit"
        );
    }
    for needle in [
        "context.module == \"provider_auth_gate\"",
        "context.module == \"openai_codex_image_generate\"",
        "context.module == \"openrouter_image_generate\"",
        "Action::\"RecordAuthReady\"",
        "Action::\"RecordStoring\"",
        "Action::\"RecordResult\"",
        "Action::\"RecordError\"",
    ] {
        assert!(
            policy.contains(needle),
            "paw-media callback policy should contain {needle}"
        );
    }
}

#[test]
fn image_generation_tool_rejects_empty_success_results() {
    let root = repo_root();
    let dispatch = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/dispatch.rs"));

    for needle in [
        "image_generate: generation completed without an image artifact",
        "file_id.is_empty()",
        "base64_data.is_empty()",
        "path.is_empty()",
    ] {
        assert!(
            dispatch.contains(needle),
            "Monty image result renderer should contain {needle}"
        );
    }
}

#[test]
fn image_generation_tool_defaults_to_session_or_default_workspace() {
    let root = repo_root();
    let dispatch = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/dispatch.rs"));

    assert!(
        dispatch.contains("resolve_image_workspace_id"),
        "Monty image generation should resolve a workspace for DM calls without explicit opts"
    );
    assert!(
        dispatch.contains("ensure_image_workspace"),
        "Monty image generation should create/use a default PawFS workspace when the Session has none"
    );
    assert!(
        !dispatch.contains("workspace_id is required because generated images are stored in PawFS"),
        "DM users should not see a missing workspace_id implementation error"
    );
}

#[test]
fn discord_delivery_accepts_pawfs_image_attachments() {
    let root = repo_root();
    let channel_spec = read(root.join("os-apps/paw-channels/specs/channel.ioa.toml"));
    let channel_model = read(root.join("os-apps/paw-channels/specs/model.csdl.xml"));
    let send_reply = read(root.join("os-apps/paw-channels/wasm/send_reply/src/lib.rs"));
    let agent_reply = read(root.join("os-apps/paw-agent/wasm/agent_reply/src/lib.rs"));
    let session_spec = read(root.join("os-apps/paw-agent/specs/session.ioa.toml"));
    let session_model = read(root.join("os-apps/paw-agent/specs/model.csdl.xml"));
    let monty_session = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/session.rs"));
    let discord_transport = read(root.join("crates/paw-transport/src/discord/transport.rs"));
    let discord_gateway = read(root.join("crates/paw-transport/src/discord/gateway.rs"));
    let paw_transport = read(root.join("crates/paw-transport/src/lib.rs"));

    for source in [channel_spec.as_str(), session_spec.as_str()] {
        assert!(
            source.contains("reply_attachments_json"),
            "Session and Channel specs should carry reply_attachments_json through entity actions"
        );
    }
    for source in [channel_model.as_str(), session_model.as_str()] {
        assert!(
            source.contains("reply_attachments_json"),
            "CSDL should expose reply_attachments_json on reply/result actions"
        );
    }
    assert!(
        monty_session.contains("reply_attachments_from_tool_results"),
        "Monty should capture image tool results as durable reply attachment metadata"
    );
    assert!(
        agent_reply.contains("\"reply_attachments_json\""),
        "agent_reply should forward Session reply attachments to Channel.SendReply"
    );
    assert!(
        send_reply.contains("\"reply_attachments_json\""),
        "send_reply should pass reply attachments to the transport webhook"
    );
    assert!(
        discord_transport.contains("deliver_reply_attachments")
            && discord_transport.contains("download_pawfs_attachment"),
        "Discord transport should download PawFS attachments before reply delivery"
    );
    assert!(
        discord_gateway.contains("send_discord_message_with_files")
            && discord_gateway.contains("payload_json")
            && discord_gateway.contains("files[0]"),
        "Discord gateway should upload generated images with multipart files"
    );
    assert!(
        paw_transport.contains("raw_get_bytes"),
        "PawApiClient should expose byte reads for PawFS $value downloads"
    );
}

#[test]
fn codex_image_renderer_streams_large_provider_responses() {
    let root = repo_root();
    let wasm = read(root.join("os-apps/paw-media/wasm/openai_codex_image_generate/src/lib.rs"));

    for needle in [
        "call_codex_image_generation",
        "temper_wasm_sdk::http_stream::streaming_call",
        "CODEX_RESPONSE_STREAM_CHUNK_BYTES",
        "CODEX_RESPONSE_MAX_BYTES",
    ] {
        assert!(
            wasm.contains(needle),
            "Codex image renderer should stream provider responses and contain {needle}"
        );
    }
    assert!(
        !wasm
            .contains("let resp = ctx.http_call(\"POST\", &url, &headers, &request.to_string())?;"),
        "Codex image renderer must not use the fixed 4MB SDK http_call buffer for image responses"
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

#[test]
fn paw_media_offers_openrouter_as_a_second_image_provider() {
    let root = repo_root();
    let app = read(root.join("os-apps/paw-media/app.toml"));
    let spec_source = read(root.join("os-apps/paw-media/specs/media_generation.ioa.toml"));
    let csdl = read(root.join("os-apps/paw-media/specs/model.csdl.xml"));
    let build_script = read(root.join("os-apps/paw-media/wasm/build.sh"));
    let wasm = read(root.join("os-apps/paw-media/wasm/openrouter_image_generate/src/lib.rs"));

    // An undeclared module is never uploaded, and every trigger for it fails.
    assert!(app.contains("name = \"openrouter_image_generate\""), "app.toml must declare the OpenRouter module");
    assert!(build_script.contains("openrouter_image_generate"), "build.sh must build and package the OpenRouter module");

    let spec = temper_spec::automaton::parse_automaton(&spec_source).expect("media spec parses");
    let action = spec
        .actions
        .iter()
        .find(|a| a.name == "GenerateWithOpenRouter")
        .expect("GenerateWithOpenRouter action");
    assert_eq!(action.from, vec!["Created".to_string(), "Failed".to_string()]);
    assert_eq!(action.to.as_deref(), Some("Generating"), "OpenRouter skips the Codex auth gate");
    for needle in [
        "module = \"openrouter_image_generate\"",
        "openrouter_api_key = \"{secret:openrouter_api_key}\"",
        "default_model = \"x-ai/grok-imagine-image-2.0\"",
    ] {
        assert!(spec_source.contains(needle), "media spec should contain {needle}");
    }
    let model = temper_spec::csdl::parse_csdl(&csdl).expect("media CSDL parses");
    let actions: Vec<String> = model.schemas.iter().flat_map(|s| s.actions.iter().map(|a| a.name.clone())).collect();
    assert!(actions.iter().any(|a| a == "GenerateWithOpenRouter"), "CSDL must expose GenerateWithOpenRouter, got {actions:?}");

    for needle in ["https://openrouter.ai/api/v1/images", "b64_json", "RecordResult", "RecordError", "streaming_call"] {
        assert!(wasm.contains(needle), "OpenRouter renderer should contain {needle}");
    }
}
