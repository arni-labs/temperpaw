use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.as_ref().display()))
}

#[test]
fn session_link_is_a_reusable_temperpaw_child_session_monitor() {
    let root = repo_root();
    let spec_path = root.join("os-apps/paw-agent/specs/session_link.ioa.toml");
    let spec = read(&spec_path);
    let wiki_builder = read(root.join("os-apps/paw-wiki/wasm/build_session_message/src/lib.rs"));

    for needle in [
        "name = \"SessionLink\"",
        "name = \"Configure\"",
        "name = \"CheckChild\"",
        "name = \"ChildPending\"",
        "name = \"ParentNotified\"",
        "name = \"NotifyFailed\"",
        "module = \"session_link_monitor\"",
        "max_checks",
        "[[state_timeout]]",
        "state = \"Created\"",
        "state = \"Watching\"",
        "from = [\"Created\", \"Watching\"]",
    ] {
        assert!(
            spec.contains(needle),
            "SessionLink spec should contain {needle}"
        );
    }

    assert!(
        !spec.contains("allow_indefinite_states = [\"Watching\"]"),
        "SessionLink.Watching must be bounded by max_checks, not indefinite"
    );

    assert!(
        spec.contains("initial = \"80\""),
        "SessionLink should default to the 40 minute bounded monitor budget at a 30s poll interval"
    );
    assert!(
        spec.contains("\"schedule('CheckChild', 30)\""),
        "SessionLink pending child checks should avoid 10s write-amplifying polling"
    );

    assert!(
        wiki_builder.contains("/tdata/SessionLinks")
            && wiki_builder.contains("ParentEntitySet")
            && wiki_builder.contains("ChildSessionId")
            && wiki_builder.contains("OnFailureAction")
            && wiki_builder.contains("\"MaxChecks\": \"80\""),
        "WikiJob should use the reusable SessionLink monitor instead of bespoke child-session polling"
    );

    assert!(
        wiki_builder.contains("dispatch_wiki_job_failure")
            && wiki_builder.contains("SessionLink setup failed"),
        "WikiJob should fail visibly if child-session monitoring cannot be established"
    );
}

#[test]
fn wiki_build_session_message_emits_step_metrics_for_spawn_path() {
    let root = repo_root();
    let wiki_builder = read(root.join("os-apps/paw-wiki/wasm/build_session_message/src/lib.rs"));

    assert!(
        wiki_builder.contains("temper_wiki_build_session_message_step_duration_ms"),
        "WikiJob child-session spawn should emit an app-specific step-duration histogram"
    );
    assert!(
        wiki_builder.contains("emit_build_session_step_duration")
            && wiki_builder.contains("Context::get_time_millis"),
        "WikiJob child-session spawn should measure each stateful OData boundary"
    );

    for needle in [
        "\"ensure_workspace\"",
        "\"create_session\"",
        "\"configure_session\"",
        "\"session_spawned\"",
        "\"create_session_link\"",
        "\"configure_session_link\"",
        "\"total\"",
        "\"result\": result",
    ] {
        assert!(
            wiki_builder.contains(needle),
            "WikiJob build_session_message metrics should include {needle}"
        );
    }
}

#[test]
fn runtime_model_provider_selection_has_no_hardcoded_llm_fallbacks() {
    let root = repo_root();
    let checked_files = [
        "os-apps/paw-agent/specs/session.ioa.toml",
        "os-apps/paw-agent/specs/agent.ioa.toml",
        "os-apps/paw-agent/specs/cron_job.ioa.toml",
        "os-apps/paw-wiki/specs/wiki_job.ioa.toml",
        "os-apps/paw-managed-agents/specs/managed_agent.ioa.toml",
        "os-apps/paw-agent/wasm/context_preparer/src/lib.rs",
        "os-apps/paw-agent/wasm/provider_caller/src/lib.rs",
        "os-apps/paw-agent/wasm/provider_response_applier/src/lib.rs",
        "os-apps/paw-agent/wasm/context_compactor/src/lib.rs",
        "os-apps/paw-agent/wasm/monty_repl/src/entity_ops.rs",
        "os-apps/paw-wiki/wasm/build_session_message/src/lib.rs",
        "os-apps/paw-channels/wasm/route_message/src/lib.rs",
        "os-apps/paw-managed-agents/wasm/common.rs",
        "os-apps/paw-managed-agents/wasm/session_orchestrator/src/lib.rs",
    ];
    let prohibited = [
        "initial = \"claude-sonnet",
        "initial = \"anthropic\"",
        "unwrap_or(\"claude-sonnet",
        "unwrap_or(\"anthropic\")",
        "unwrap_or_else(|| \"claude-sonnet",
        "unwrap_or_else(|| \"anthropic\"",
        "\"provider\": \"anthropic\"",
        "falling back to {alt}",
        "let alternatives = [\"anthropic\"",
        "infer_provider(",
    ];

    for relative in checked_files {
        let contents = read(root.join(relative));
        for needle in prohibited {
            assert!(
                !contents.contains(needle),
                "{relative} should not contain hardcoded runtime model/provider fallback `{needle}`"
            );
        }
    }
}

#[test]
fn session_spec_uses_provider_specific_llm_secrets_and_urls() {
    let root = repo_root();
    let spec = read(root.join("os-apps/paw-agent/specs/session.ioa.toml"));

    assert!(
        !spec
            .lines()
            .any(|line| { line.trim() == "api_key = \"{secret:anthropic_api_key}\"" }),
        "session integrations should not inject a generic anthropic api_key into multi-provider LLM modules"
    );

    for needle in [
        "anthropic_api_key = \"{secret:anthropic_api_key}\"",
        "openai_api_key = \"{secret:openai_api_key}\"",
        "openai_codex_access_token = \"{secret:openai_codex_access_token}\"",
        "openai_codex_refresh_token = \"{secret:openai_codex_refresh_token}\"",
        "openai_codex_expires_at_ms = \"{secret:openai_codex_expires_at_ms}\"",
        "openai_codex_account_id = \"{secret:openai_codex_account_id}\"",
        "openai_codex_token = \"{secret:openai_codex_token}\"",
        "openrouter_api_key = \"{secret:openrouter_api_key}\"",
        "openrouter_api_url = \"{secret:openrouter_api_url}\"",
        "huggingface_api_key = \"{secret:huggingface_api_key}\"",
        "hf_token = \"{secret:hf_token}\"",
        "fireworks_api_key = \"{secret:fireworks_api_key}\"",
        "sakana_fugu_api_key = \"{secret:sakana_fugu_api_key}\"",
        "openai_compatible_api_key = \"{secret:openai_compatible_api_key}\"",
        "openai_compatible_headers_json = \"{secret:openai_compatible_headers_json}\"",
        "openai_api_url = \"https://api.openai.com/v1/responses\"",
        "openai_codex_api_url = \"https://chatgpt.com/backend-api/codex/responses\"",
        "huggingface_api_url = \"{secret:huggingface_api_url}\"",
        "fireworks_api_url = \"{secret:fireworks_api_url}\"",
        "sakana_fugu_api_url = \"{secret:sakana_fugu_api_url}\"",
        "openai_compatible_api_url = \"{secret:openai_compatible_api_url}\"",
        "local_openai_api_url = \"{secret:local_openai_api_url}\"",
    ] {
        assert!(
            spec.contains(needle),
            "session spec should contain {needle}"
        );
    }
}

#[test]
fn agent_and_session_specs_carry_provider_options_json() {
    let root = repo_root();
    let agent = read(root.join("os-apps/paw-agent/specs/agent.ioa.toml"));
    let session = read(root.join("os-apps/paw-agent/specs/session.ioa.toml"));
    let model = read(root.join("os-apps/paw-agent/specs/model.csdl.xml"));

    for (name, source) in [("Agent", agent.as_str()), ("Session", session.as_str())] {
        assert!(
            source.contains("name = \"provider_options_json\""),
            "{name} spec should define provider_options_json state"
        );
        assert!(
            source.contains("\"provider_options_json\""),
            "{name} Configure/Update actions should accept provider_options_json"
        );
    }

    for needle in [
        "<Property Name=\"ProviderOptionsJson\" Type=\"Edm.String\"/>",
        "<Parameter Name=\"provider_options_json\" Type=\"Edm.String\" Nullable=\"true\"/>",
    ] {
        assert!(
            model.contains(needle),
            "CSDL should expose provider_options_json: {needle}"
        );
    }
}

#[test]
fn governance_approval_prompts_include_decision_details_and_scope_choices() {
    let root = repo_root();
    let approval_wasm = read(root.join("os-apps/paw-agent/wasm/request_approval/src/lib.rs"));
    let transport_lib = read(root.join("crates/paw-transport/src/lib.rs"));
    let discord_transport = read(root.join("crates/paw-transport/src/discord/transport.rs"));
    let slack_transport = read(root.join("crates/paw-transport/src/slack/transport.rs"));

    for needle in [
        "fetch_pending_decision",
        "format_approval_content",
        "Action: `",
        "Resource: `",
        "Reason: ",
        "Allow Always",
        "Allow Session",
        "Allow Once",
        "approve_always:",
        "approve_session:",
        "approve_once:",
    ] {
        assert!(
            approval_wasm.contains(needle),
            "request_approval should include detailed approval prompts and scoped button `{needle}`"
        );
    }

    for needle in [
        "approval_scope_from_action",
        "approval_body_for_scope",
        "\"action\": \"this_action\"",
        "\"duration\": \"session\"",
        "\"resource\": \"this_resource\"",
    ] {
        assert!(
            transport_lib.contains(needle),
            "transport helper should construct complete scoped Cedar approval body containing `{needle}`"
        );
    }

    for (name, source) in [
        ("discord", discord_transport.as_str()),
        ("slack", slack_transport.as_str()),
    ] {
        for needle in ["approval_scope_from_action", "approval_body_for_scope"] {
            assert!(
                source.contains(needle),
                "{name} approval handling should use scoped Cedar approval helper `{needle}`"
            );
        }
    }
}

#[test]
fn managed_agent_inner_sessions_preserve_parent_for_approval_routing() {
    let root = repo_root();
    let managed_session_spec =
        read(root.join("os-apps/paw-managed-agents/specs/managed_session.ioa.toml"));
    let orchestrator =
        read(root.join("os-apps/paw-managed-agents/wasm/session_orchestrator/src/lib.rs"));
    let monty_dispatch = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/dispatch.rs"));

    assert!(
        managed_session_spec.contains("name = \"parent_session_id\""),
        "ManagedSession should record the originating Session so inner SWE approvals can route back"
    );
    assert!(
        orchestrator.contains("parent_session_id")
            && orchestrator.contains("\"parent_session_id\": parent_session_id"),
        "session_orchestrator should propagate ManagedSession.parent_session_id into inner Session.Configure"
    );
    assert!(
        monty_dispatch.contains("with_session_parent_provenance")
            && monty_dispatch.contains("\"ManagedSessions\""),
        "temper.create should stamp ManagedSessions with the current Session as parent_session_id"
    );
    assert!(
        monty_dispatch.contains("\"CurationJobs\"")
            && monty_dispatch.contains("with_curation_job_action_parent")
            && monty_dispatch.contains("\"ConfigureAndSubmit\""),
        "temper.create/action should stamp Katagami CurationJobs with parent_session_id so approvals route back to chat"
    );
}

#[test]
fn paw_agent_defines_temper_native_openai_codex_auth_entity() {
    let root = repo_root();
    let spec = read(root.join("os-apps/paw-agent/specs/openai_codex_auth.ioa.toml"));
    let model = read(root.join("os-apps/paw-agent/specs/model.csdl.xml"));
    let setup_api = read(root.join("crates/temperpaw/src/setup_api.rs"));

    for needle in [
        "name = \"OpenAICodexAuth\"",
        "StartDeviceLogin",
        "PollDeviceLogin",
        "Refresh",
        "EnsureFresh",
        "ForceRefresh",
        "Disconnect",
        "module = \"openai_codex_auth\"",
        "mode = \"ensure\"",
        "mode = \"force_refresh\"",
        "openai_auth_base_url",
        "name = \"error_message\"",
        "state = \"Starting\"",
        "state = \"Polling\"",
        "state = \"Refreshing\"",
        "on_timeout = \"Fail\"",
    ] {
        assert!(
            spec.contains(needle),
            "OpenAICodexAuth spec should contain {needle}"
        );
    }

    for needle in [
        "<EntityType Name=\"OpenAICodexAuth\">",
        "<Property Name=\"ErrorMessage\" Type=\"Edm.String\"/>",
        "<Action Name=\"EnsureFresh\" IsBound=\"true\">",
        "<Action Name=\"ForceRefresh\" IsBound=\"true\">",
        "<EntitySet Name=\"OpenAICodexAuths\" EntityType=\"TemperPaw.OpenAICodexAuth\"/>",
    ] {
        assert!(
            model.contains(needle),
            "paw-agent CSDL should expose OpenAICodexAuth through OData: {needle}"
        );
    }

    assert!(
        setup_api.contains(".dispatch_tenant_action_ext(")
            && setup_api.contains("await_integration: true"),
        "setup Codex auth routes should wait for OpenAICodexAuth WASM before reporting readiness"
    );
}
