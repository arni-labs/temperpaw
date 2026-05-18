use std::collections::HashMap;
use std::fs;
use std::path::Path;

use temper_authz::{AuthzEngine, SecurityContext};

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn agent_context(id: &str, agent_type: &str) -> SecurityContext {
    SecurityContext::from_headers(&[
        ("X-Temper-Principal-Id".to_string(), id.to_string()),
        ("X-Temper-Principal-Kind".to_string(), "agent".to_string()),
        ("X-Temper-Agent-Type".to_string(), agent_type.to_string()),
    ])
}

fn resource_attrs(pairs: &[(&str, serde_json::Value)]) -> HashMap<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

#[test]
fn session_spec_defines_bounded_turn_pipeline() {
    let spec = fs::read_to_string(repo_root().join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");

    for needle in [
        "PreparingContext",
        "CallingProvider",
        "ApplyingProviderResponse",
        "name = \"ContextReady\"",
        "name = \"ProviderResponseReady\"",
        "name = \"prepare_context\"",
        "module = \"context_preparer\"",
        "name = \"call_provider\"",
        "module = \"provider_caller\"",
        "name = \"apply_provider_response\"",
        "module = \"provider_response_applier\"",
        "name = \"prepared_context_file_id\"",
        "name = \"provider_response_file_id\"",
    ] {
        assert!(
            spec.contains(needle),
            "session spec should contain {needle}"
        );
    }

    assert!(
        spec.contains("effect = [{ type = \"trigger\", name = \"prepare_context\" }]"),
        "session spec should route turn entry points through prepare_context"
    );
}

#[test]
fn session_routes_llm_calls_through_codex_auth_gate() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let build_sh = fs::read_to_string(root.join("os-apps/paw-agent/wasm/build.sh"))
        .expect("paw-agent wasm build script should exist");

    for needle in [
        "EnsuringProviderAuth",
        "EnsuringCompactionAuth",
        "ProviderAuthReady",
        "ProviderAuthExpired",
        "CompactionAuthReady",
        "CompactionAuthExpired",
        "name = \"ensure_provider_auth\"",
        "name = \"force_provider_auth_refresh\"",
        "name = \"ensure_compaction_auth\"",
        "name = \"force_compaction_auth_refresh\"",
        "module = \"provider_auth_gate\"",
        "ready_action = \"ProviderAuthReady\"",
        "ready_action = \"CompactionAuthReady\"",
        "auth_action = \"EnsureFresh\"",
        "auth_action = \"ForceRefresh\"",
        "default_llm_provider = \"{secret:llm_provider}\"",
    ] {
        assert!(
            spec.contains(needle),
            "session spec should route provider traffic through auth gate: {needle}"
        );
    }

    assert!(
        spec.contains("to = \"EnsuringProviderAuth\"")
            && spec.contains("effect = [{ type = \"trigger\", name = \"ensure_provider_auth\" }]"),
        "ContextReady should persist prepared context then enter the provider auth gate"
    );
    assert!(
        spec.contains("to = \"EnsuringCompactionAuth\"")
            && spec.contains("effect = [\n  { type = \"increment\", var = \"input_tokens\" },\n  { type = \"increment\", var = \"output_tokens\" },\n  { type = \"trigger\", name = \"ensure_compaction_auth\" }\n]"),
        "NeedsCompaction should enter the compaction auth gate before calling the compactor"
    );
    assert!(
        build_sh.contains("provider_auth_gate"),
        "provider_auth_gate must be built with the paw-agent wasm bundle"
    );
}

#[test]
fn session_admission_caps_provider_touching_actions() {
    let spec = fs::read_to_string(repo_root().join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");

    assert!(
        spec.contains(
            "max_concurrent_actions = { \"Configure\" = 5, \"ProviderAuthReady\" = 3, \"CompactionAuthReady\" = 1 }"
        ),
        "Session admission should cap provider and compaction actions, not just session creation"
    );
}

#[test]
fn channel_send_reply_trigger_is_bounded_and_reports_delivery_failure() {
    let spec = fs::read_to_string(repo_root().join("os-apps/paw-channels/specs/channel.ioa.toml"))
        .expect("channel.ioa.toml should exist");
    let trigger_block = spec
        .split("[[action.triggers]]")
        .skip(1)
        .find(|block| {
            block
                .lines()
                .any(|line| line.trim() == "name = \"send_reply\"")
        })
        .expect("Channel.SendReply should define a send_reply trigger");

    for needle in [
        "module = \"send_reply\"",
        "on_failure = \"ReplyFailed\"",
        "timeout_secs = \"30\"",
    ] {
        assert!(
            trigger_block.contains(needle),
            "send_reply trigger should contain {needle}"
        );
    }
}

#[test]
fn session_defines_non_codex_provider_auth_fast_path() {
    let spec = fs::read_to_string(repo_root().join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");

    for needle in [
        "name = \"ContextReadyAuthSkipped\"",
        "from = [\"PreparingContext\"]",
        "to = \"CallingProvider\"",
        "params = [\"prepared_context_file_id\", \"prepared_context_inline_json\", \"prepared_context_bytes\", \"prepared_context_entries_loaded\", \"prepared_context_content_files_loaded\", \"context_tokens\", \"system_prompt_hash\", \"system_prompt_file_id\", \"provider_auth_status\", \"provider_auth_checked_at_ms\", \"provider_auth_error\", \"provider_auth_retry_count\", \"compaction_auth_retry_count\"]",
        "effect = [{ type = \"trigger\", name = \"call_provider\" }]",
    ] {
        assert!(
            spec.contains(needle),
            "session spec should define provider-auth skipped fast path: {needle}"
        );
    }
}

#[test]
fn active_context_preparer_owns_delta_batch_read_contract() {
    let root = repo_root();
    let preparer =
        fs::read_to_string(root.join("os-apps/paw-agent/wasm/context_preparer/src/lib.rs"))
            .expect("context_preparer source should exist");
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");

    for needle in [
        "try_reuse_prepared_context",
        "build_context_refs_since",
        "read_text_file_versions_batch",
        "read_text_files_batch",
        "read_content_file_version_raw",
        "context_preparer: reused prepared context",
    ] {
        assert!(
            preparer.contains(needle),
            "active context_preparer should contain {needle}"
        );
    }

    assert!(
        spec.contains("reset_on = [\"ProgressMade\", \"ResumeContext\"]"),
        "PreparingContext state_timeout should reset on real ProgressMade"
    );
}

#[test]
fn entity_backed_session_appends_use_direct_session_entry_create() {
    let root = repo_root();
    let provider_applier = fs::read_to_string(
        root.join("os-apps/paw-agent/wasm/provider_response_applier/src/lib.rs"),
    )
    .expect("provider_response_applier source should exist");
    let monty_session =
        fs::read_to_string(root.join("os-apps/paw-agent/wasm/monty_repl/src/session.rs"))
            .expect("monty_repl session source should exist");
    let helpers = fs::read_to_string(root.join("os-apps/paw-agent/wasm/wasm-helpers/src/lib.rs"))
        .expect("wasm helpers source should exist");

    for (name, source) in [
        ("provider_response_applier", provider_applier.as_str()),
        ("monty_repl session", monty_session.as_str()),
    ] {
        assert!(
            source.contains("append_session_entry_inline"),
            "{name} should directly create one SessionEntry for entity-backed turn appends"
        );
    }

    assert!(
        helpers.contains("pub fn append_session_entry_inline"),
        "wasm helpers should expose the direct SessionEntry append primitive"
    );
}

#[test]
fn first_turn_session_entries_materialize_after_provider_success() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let csdl = fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml"))
        .expect("model.csdl.xml should exist");
    let workspace =
        fs::read_to_string(root.join("os-apps/paw-agent/wasm/workspace_provisioner/src/lib.rs"))
            .expect("workspace_provisioner source should exist");
    let preparer =
        fs::read_to_string(root.join("os-apps/paw-agent/wasm/context_preparer/src/lib.rs"))
            .expect("context_preparer source should exist");
    let applier = fs::read_to_string(
        root.join("os-apps/paw-agent/wasm/provider_response_applier/src/lib.rs"),
    )
    .expect("provider_response_applier source should exist");
    let helpers = fs::read_to_string(root.join("os-apps/paw-agent/wasm/wasm-helpers/src/lib.rs"))
        .expect("wasm helpers source should exist");

    for needle in [
        "[[state]]\nname = \"session_entries_materialized\"",
        "params = [\"workspace_id\", \"conversation_file_id\", \"file_manifest_id\", \"session_file_id\", \"session_leaf_id\", \"session_entries_materialized\"]",
        "params = [\"result\", \"conversation\", \"input_tokens\", \"output_tokens\", \"session_leaf_id\", \"session_entries_materialized\"",
    ] {
        assert!(
            spec.contains(needle),
            "Session spec should expose first-turn materialization state via {needle}"
        );
    }

    assert!(
        csdl.contains("<Property Name=\"SessionEntriesMaterialized\" Type=\"Edm.String\""),
        "Session CSDL should expose SessionEntriesMaterialized"
    );
    assert!(
        workspace.contains("create_virtual_hot_session_storage"),
        "workspace_provisioner should use a virtual first-turn SessionEntries ref"
    );
    assert!(
        workspace.contains("\"session_entries_materialized\": session_entries_materialized")
            && workspace.contains("\"false\".to_string()"),
        "WorkspaceReady should record that first-turn SessionEntries are not materialized yet"
    );
    assert!(
        preparer.contains("context_preparer: virtual first-turn session entries"),
        "context_preparer should explicitly prepare from Session.user_message for virtual first turns"
    );
    assert!(
        applier.contains("materialize_initial_session_entries_with_assistant"),
        "provider_response_applier should materialize header/user/assistant before terminal success"
    );
    assert!(
        applier.contains("params[\"session_entries_materialized\"] = json!(\"true\")"),
        "provider_response_applier terminal/tool params should mark materialization complete"
    );
    assert!(
        helpers.contains("pub fn materialize_initial_session_entries_with_assistant"),
        "wasm helpers should expose the verified first-turn materialization helper"
    );
}

#[test]
fn provider_caller_does_not_persist_provider_boundary_progress_by_default() {
    let source =
        fs::read_to_string(repo_root().join("os-apps/paw-agent/wasm/provider_caller/src/lib.rs"))
            .expect("provider_caller source should exist");

    for needle in [
        "fn provider_progress_dispatch_enabled",
        "provider_progress_dispatch_enabled",
        ".unwrap_or(false)",
        "if provider_progress_enabled",
    ] {
        assert!(
            source.contains(needle),
            "provider_caller should gate provider-boundary ProgressMade writes with {needle}"
        );
    }
}

#[test]
fn provider_caller_initial_heartbeat_is_opt_in() {
    let root = repo_root();
    let source = fs::read_to_string(root.join("os-apps/paw-agent/wasm/provider_caller/src/lib.rs"))
        .expect("provider_caller source should exist");
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");

    for needle in [
        "fn provider_initial_heartbeat_enabled",
        "fn should_send_initial_provider_heartbeat",
        "session_provider_initial_heartbeat_enabled",
        "if should_send_initial_provider_heartbeat",
    ] {
        assert!(
            source.contains(needle),
            "provider_caller should make eager provider Heartbeat opt-in with {needle}"
        );
    }

    assert!(
        spec.contains("provider_initial_heartbeat_enabled = \"false\""),
        "Session provider_caller config should make the eager pre-provider Heartbeat disabled by default"
    );
    assert!(
        !source.contains("if !mock_hang {\n        let _ = send_heartbeat"),
        "provider_caller should not emit an unconditional pre-provider Heartbeat on the fast path"
    );
}

#[test]
fn provider_caller_typing_indicator_is_route_aware() {
    let source =
        fs::read_to_string(repo_root().join("os-apps/paw-agent/wasm/provider_caller/src/lib.rs"))
            .expect("provider_caller source should exist");

    for needle in [
        "should_send_provider_typing_indicator(&ctx.entity_id, &fields)",
        "provider_caller: skipping typing indicator for direct or inline route",
        "\"reply_channel_type\", \"ReplyChannelType\"",
        "matches!(reply_channel_type.as_str(), \"cli\" | \"tui\")",
    ] {
        assert!(
            source.contains(needle),
            "provider_caller should keep typing indicator lookup off the direct/inline hot path via {needle}"
        );
    }
}

#[test]
fn route_message_carries_context_cache_fields_to_continuations() {
    let source =
        fs::read_to_string(repo_root().join("os-apps/paw-channels/wasm/route_message/src/lib.rs"))
            .expect("route_message source should exist");

    for needle in [
        "\"prepared_context_file_id\": prepared_context_storage.file_id",
        "fn continuation_prepared_context_storage",
        "file_id: str_field(",
        "\"prepared_context_file_id\"",
        "\"PreparedContextFileId\"",
    ] {
        assert!(
            source.contains(needle),
            "continuation Configure body should carry prepared context via {needle}"
        );
    }

    for needle in [
        "\"system_prompt_hash\": str_field(fields, &[\"system_prompt_hash\", \"SystemPromptHash\"]).unwrap_or(\"\")",
        "\"system_prompt_file_id\": str_field(fields, &[\"system_prompt_file_id\", \"SystemPromptFileId\"]).unwrap_or(\"\")",
    ] {
        assert!(
            source.contains(needle),
            "continuation Configure body should carry {needle}"
        );
    }
}

#[test]
fn route_message_records_immediate_parent_session_on_continuation() {
    let source =
        fs::read_to_string(repo_root().join("os-apps/paw-channels/wasm/route_message/src/lib.rs"))
            .expect("route_message source should exist");

    assert!(
        source.contains("\"parent_session_id\": prior_session_id,"),
        "continuation Configure body should record the immediate prior Session as parent_session_id"
    );
}

#[test]
fn inline_channel_reply_delivery_is_direct_but_policy_gated() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let csdl = fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml"))
        .expect("session model should exist");
    let route_message =
        fs::read_to_string(root.join("os-apps/paw-channels/wasm/route_message/src/lib.rs"))
            .expect("route_message source should exist");
    let agent_reply =
        fs::read_to_string(root.join("os-apps/paw-agent/wasm/agent_reply/src/lib.rs"))
            .expect("agent_reply source should exist");
    let policy = fs::read_to_string(root.join("os-apps/paw-channels/policies/channels.cedar"))
        .expect("channels policy should exist");

    for needle in [
        "name = \"reply_channel_type\"",
        "\"reply_channel_id\", \"reply_thread_id\", \"reply_channel_entity_id\", \"reply_channel_type\", \"reply_route_source\"",
    ] {
        assert!(
            spec.contains(needle),
            "Session spec should carry reply channel type via {needle}"
        );
    }

    for needle in [
        "<Property Name=\"ReplyChannelType\" Type=\"Edm.String\"/>",
        "<Parameter Name=\"reply_channel_type\" Type=\"Edm.String\" Nullable=\"true\"/>",
    ] {
        assert!(
            csdl.contains(needle),
            "Session CSDL should expose reply channel type via {needle}"
        );
    }

    for needle in [
        "channel_type = str_field(&fields, &[\"channel_type\", \"ChannelType\"]).unwrap_or(\"\")",
        "reply_channel_type",
        "delivery_route_snapshot_from_channel_message(",
        "&ctx.entity_id",
        "channel_type,",
    ] {
        assert!(
            route_message.contains(needle),
            "route_message should preserve inline route type via {needle}"
        );
    }

    for needle in [
        "fn channel_reply_action_url",
        "Paw.Channel.ReplyDelivered",
        "Paw.Channel.SendReply?await_integration=true",
        "route.channel_type.as_deref()",
    ] {
        assert!(
            agent_reply.contains(needle),
            "agent_reply should choose direct inline delivery via {needle}"
        );
    }

    for needle in [
        "action == Action::\"ReplyDelivered\"",
        "principal.agent_type == \"agent\"",
        "[\"cli\", \"tui\"].contains(resource.ChannelType)",
        "[\"cli\", \"tui\"].contains(resource.channel_type)",
    ] {
        assert!(
            policy.contains(needle),
            "channels policy should narrowly gate inline ReplyDelivered via {needle}"
        );
    }
}

#[test]
fn inline_reply_delivered_policy_authorizes_only_inline_channels() {
    let policy =
        fs::read_to_string(repo_root().join("os-apps/paw-channels/policies/channels.cedar"))
            .expect("channels policy should exist");
    let engine = AuthzEngine::new(&policy).expect("channels.cedar should parse");
    let agent = agent_context("agent-1", "agent");
    let cli_channel = resource_attrs(&[
        ("id", serde_json::json!("ch-cli")),
        ("ChannelType", serde_json::json!("cli")),
    ]);
    let discord_channel = resource_attrs(&[
        ("id", serde_json::json!("ch-discord")),
        ("ChannelType", serde_json::json!("discord")),
    ]);

    assert!(
        engine
            .authorize(&agent, "ReplyDelivered", "Channel", &cli_channel)
            .is_allowed(),
        "agent should be allowed to record direct ReplyDelivered on inline cli channels"
    );
    assert!(
        !engine
            .authorize(&agent, "ReplyDelivered", "Channel", &discord_channel)
            .is_allowed(),
        "agent must not be allowed to bypass send_reply on webhook-backed channels"
    );
    assert!(
        engine
            .authorize(&agent, "SendReply", "Channel", &discord_channel)
            .is_allowed(),
        "agent should still use SendReply for webhook-backed channels"
    );
}

#[test]
fn session_policy_authorizes_new_pipeline_callbacks_and_modules() {
    let policy = fs::read_to_string(repo_root().join("os-apps/paw-agent/policies/session.cedar"))
        .expect("session.cedar should exist");

    for needle in [
        "Action::\"ContextReady\"",
        "Action::\"ContextReadyAuthSkipped\"",
        "Action::\"ProviderAuthReady\"",
        "Action::\"ProviderAuthExpired\"",
        "Action::\"ProviderResponseReady\"",
        "\"context_preparer\"",
        "\"provider_auth_gate\"",
        "\"provider_caller\"",
        "\"provider_response_applier\"",
        "Action::\"CompactionAuthReady\"",
        "Action::\"CompactionAuthExpired\"",
    ] {
        assert!(
            policy.contains(needle),
            "session policy should contain {needle}"
        );
    }
}

#[test]
fn session_policy_authorizes_openai_codex_auth_wasm_boundaries() {
    let policy = fs::read_to_string(repo_root().join("os-apps/paw-agent/policies/session.cedar"))
        .expect("session.cedar should exist");

    assert!(
        policy.matches("\"openai_codex_auth\"").count() >= 2,
        "openai_codex_auth must be authorized for http_call and access_secret"
    );
}

#[test]
fn dashboard_and_monitors_cover_session_context_metrics() {
    let dashboard = fs::read_to_string(repo_root().join("dd-dashboards/temperpaw-overview.json"))
        .expect("dashboard json should exist");
    let monitors = fs::read_to_string(repo_root().join("dd-monitors/temperpaw-monitors.json"))
        .expect("monitor json should exist");

    for needle in [
        "temper_session_context_tokens",
        "temper_session_context_bytes",
        "temper_session_context_prepare_duration_ms",
        "temper_session_provider_request_bytes",
        "temper_session_provider_response_bytes",
        "temper_session_memory_limit_exceeded_total",
    ] {
        assert!(
            dashboard.contains(needle),
            "dashboard should contain {needle}"
        );
    }

    assert!(
        monitors.contains("temper_session_memory_limit_exceeded_total"),
        "monitors should alert on session memory-limit failures"
    );
}

#[test]
fn session_spec_passes_modal_bridge_url_to_modal_integrations() {
    let spec = fs::read_to_string(repo_root().join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");

    let count = spec
        .matches("modal_bridge_url = \"{secret:modal_bridge_url}\"")
        .count();

    assert!(
        count >= 3,
        "session spec should pass modal_bridge_url into sandbox-related integrations"
    );
}

#[test]
fn record_result_clears_pending_tool_state_on_terminal_completion() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let csdl = fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml"))
        .expect("model.csdl.xml should exist");
    let monty = fs::read_to_string(root.join("os-apps/paw-agent/wasm/monty_repl/src/lib.rs"))
        .expect("monty_repl source should exist");

    assert!(
        spec.contains(
            "params = [\"result\", \"conversation\", \"input_tokens\", \"output_tokens\", \"session_leaf_id\", \"session_entries_materialized\", \"repl_file_id\", \"tool_spans_file_id\", \"system_prompt_hash\", \"system_prompt_file_id\", \"provider_response_file_id\", \"provider_response_inline_json\", \"pending_tool_calls\", \"pending_tool_context\", \"pending_decision_id\"]"
        ),
        "RecordResult should be able to clear pending tool and approval fields on completion"
    );

    for needle in [
        "<Parameter Name=\"tool_spans_file_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"provider_response_file_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"provider_response_inline_json\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_tool_calls\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_tool_context\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_decision_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
    ] {
        assert!(
            csdl.contains(needle),
            "RecordResult CSDL should contain {needle}"
        );
    }

    for needle in [
        "done_params[\"pending_tool_calls\"] = json!(\"\")",
        "done_params[\"pending_tool_context\"] = json!(\"\")",
        "done_params[\"pending_decision_id\"] = json!(\"\")",
    ] {
        assert!(
            monty.contains(needle),
            "temper.done completion path should contain {needle}"
        );
    }
}

#[test]
fn record_result_no_reply_preserves_terminal_cleanup_without_delivery_trigger() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let csdl = fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml"))
        .expect("model.csdl.xml should exist");
    let applier = fs::read_to_string(
        root.join("os-apps/paw-agent/wasm/provider_response_applier/src/lib.rs"),
    )
    .expect("provider_response_applier source should exist");
    let policy = fs::read_to_string(root.join("os-apps/paw-agent/policies/session.cedar"))
        .expect("session policy should exist");

    assert!(
        spec.contains("name = \"RecordResultNoReply\""),
        "direct no-route sessions should have a spec-visible terminal action"
    );
    let action_start = spec
        .find("name = \"RecordResultNoReply\"")
        .expect("RecordResultNoReply action should exist");
    let action_tail = &spec[action_start..];
    let action_end = action_tail
        .find("# --- Actions: Heartbeat")
        .expect("RecordResultNoReply should appear before heartbeat actions");
    let action_block = &action_tail[..action_end];
    assert!(
        action_block.contains(
            "params = [\"result\", \"conversation\", \"input_tokens\", \"output_tokens\", \"session_leaf_id\", \"session_entries_materialized\", \"repl_file_id\", \"tool_spans_file_id\", \"system_prompt_hash\", \"system_prompt_file_id\", \"provider_response_file_id\", \"provider_response_inline_json\", \"pending_tool_calls\", \"pending_tool_context\", \"pending_decision_id\"]"
        ),
        "RecordResultNoReply should keep RecordResult cleanup/accounting params"
    );
    assert!(
        action_block.contains("{ type = \"trigger\", name = \"emit_ots_trajectory\" }"),
        "RecordResultNoReply must still emit terminal trajectory"
    );
    assert!(
        !action_block.contains("deliver_reply"),
        "RecordResultNoReply should not invoke no-op terminal reply delivery"
    );
    assert!(
        csdl.contains("<Action Name=\"RecordResultNoReply\" IsBound=\"true\">"),
        "CSDL should expose RecordResultNoReply"
    );

    for needle in [
        "<Parameter Name=\"tool_spans_file_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"provider_response_file_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"provider_response_inline_json\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_tool_calls\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_tool_context\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_decision_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
    ] {
        assert!(
            csdl.contains(needle),
            "RecordResultNoReply CSDL should contain {needle}"
        );
    }

    assert!(
        applier.contains("should_bypass_terminal_reply(&ctx.entity_id, &fields)")
            && applier.contains("\"RecordResultNoReply\""),
        "provider_response_applier should choose the no-reply action for direct no-route completion"
    );
    assert!(
        policy.contains("Action::\"RecordResultNoReply\""),
        "Cedar policy must permit the new Session callback action"
    );
}

#[test]
fn record_result_inline_reply_preserves_channel_audit_without_agent_reply() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let csdl = fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml"))
        .expect("model.csdl.xml should exist");
    let applier = fs::read_to_string(
        root.join("os-apps/paw-agent/wasm/provider_response_applier/src/lib.rs"),
    )
    .expect("provider_response_applier source should exist");
    let policy = fs::read_to_string(root.join("os-apps/paw-agent/policies/session.cedar"))
        .expect("session policy should exist");

    assert!(
        spec.contains("name = \"RecordResultInlineReply\""),
        "inline cli/tui routes should have a spec-visible terminal action"
    );
    let action_start = spec
        .find("name = \"RecordResultInlineReply\"")
        .expect("RecordResultInlineReply action should exist");
    let action_tail = &spec[action_start..];
    let action_end = action_tail
        .find("# --- Actions: Heartbeat")
        .expect("RecordResultInlineReply should appear before heartbeat actions");
    let action_block = &action_tail[..action_end];
    assert!(
        action_block.contains(
            "params = [\"result\", \"conversation\", \"input_tokens\", \"output_tokens\", \"session_leaf_id\", \"session_entries_materialized\", \"repl_file_id\", \"tool_spans_file_id\", \"system_prompt_hash\", \"system_prompt_file_id\", \"provider_response_file_id\", \"provider_response_inline_json\", \"pending_tool_calls\", \"pending_tool_context\", \"pending_decision_id\"]"
        ),
        "RecordResultInlineReply should keep RecordResult cleanup/accounting params"
    );
    assert!(
        action_block.contains("{ type = \"trigger\", name = \"emit_ots_trajectory\" }"),
        "RecordResultInlineReply must still emit terminal trajectory"
    );
    assert!(
        !action_block.contains("deliver_reply"),
        "RecordResultInlineReply should not invoke agent_reply after inline Channel audit"
    );

    for needle in [
        "<Action Name=\"RecordResultInlineReply\" IsBound=\"true\">",
        "<Parameter Name=\"tool_spans_file_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"provider_response_file_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"provider_response_inline_json\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_tool_calls\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_tool_context\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_decision_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
    ] {
        assert!(
            csdl.contains(needle),
            "RecordResultInlineReply CSDL should contain {needle}"
        );
    }

    for needle in [
        "try_dispatch_inline_reply(",
        "&result_text",
        "\"RecordResultInlineReply\"",
        "fn inline_reply_route",
        "fn inline_reply_action_url",
        "Paw.Channel.ReplyDelivered",
        "matches!(channel_type.trim(), \"cli\" | \"tui\")",
        "\"thread_id\": route.thread_id.as_str()",
        "\"agent_entity_id\": route.agent_entity_id.as_str()",
        "provider_response_applier: inline reply dispatch failed; falling back to RecordResult",
    ] {
        assert!(
            applier.contains(needle),
            "provider_response_applier should inline terminal cli/tui delivery via {needle}"
        );
    }

    assert!(
        policy.contains("Action::\"RecordResultInlineReply\""),
        "Cedar policy must permit the inline-reply Session callback action"
    );
}

#[test]
fn finalize_result_clears_pending_tool_state_on_terminal_completion() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let csdl = fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml"))
        .expect("model.csdl.xml should exist");
    let steering_checker =
        fs::read_to_string(root.join("os-apps/paw-agent/wasm/steering_checker/src/lib.rs"))
            .expect("steering_checker source should exist");

    assert!(
        spec.contains(
            "params = [\"result\", \"conversation\", \"session_leaf_id\", \"pending_tool_calls\", \"pending_tool_context\", \"pending_decision_id\"]"
        ),
        "FinalizeResult should be able to clear pending tool and approval fields on completion"
    );
    assert!(
        spec.contains("name = \"FinalizeResultNoReply\""),
        "direct no-reply steering completion should have a spec-visible terminal action"
    );
    let no_reply_start = spec
        .find("name = \"FinalizeResultNoReply\"")
        .expect("FinalizeResultNoReply action should exist");
    let no_reply_tail = &spec[no_reply_start..];
    let no_reply_end = no_reply_tail
        .find("name = \"Steer\"")
        .expect("FinalizeResultNoReply should appear before Steer");
    let no_reply_block = &no_reply_tail[..no_reply_end];
    assert!(
        no_reply_block.contains(
            "params = [\"result\", \"conversation\", \"session_leaf_id\", \"pending_tool_calls\", \"pending_tool_context\", \"pending_decision_id\"]"
        ),
        "FinalizeResultNoReply should keep FinalizeResult result and cleanup params"
    );
    assert!(
        no_reply_block.contains("{ type = \"trigger\", name = \"emit_ots_trajectory\" }"),
        "FinalizeResultNoReply must still emit terminal trajectory"
    );
    assert!(
        !no_reply_block.contains("deliver_reply"),
        "FinalizeResultNoReply should not invoke no-op terminal reply delivery"
    );

    for needle in [
        "<Action Name=\"FinalizeResult\" IsBound=\"true\">",
        "<Action Name=\"FinalizeResultNoReply\" IsBound=\"true\">",
        "<Parameter Name=\"pending_tool_calls\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_tool_context\" Type=\"Edm.String\" Nullable=\"true\"/>",
        "<Parameter Name=\"pending_decision_id\" Type=\"Edm.String\" Nullable=\"true\"/>",
    ] {
        assert!(
            csdl.contains(needle),
            "FinalizeResult CSDL should contain {needle}"
        );
    }

    for needle in [
        "\"pending_tool_calls\": \"\"",
        "\"pending_tool_context\": \"\"",
        "\"pending_decision_id\": \"\"",
        "\"FinalizeResultNoReply\"",
        "direct_no_reply",
    ] {
        assert!(
            steering_checker.contains(needle),
            "steering_checker finalize path should contain {needle}"
        );
    }

    let policy = fs::read_to_string(root.join("os-apps/paw-agent/policies/session.cedar"))
        .expect("session policy should exist");
    assert!(
        policy.contains("Action::\"FinalizeResultNoReply\""),
        "Cedar policy must permit the no-reply steering terminal callback"
    );
}
