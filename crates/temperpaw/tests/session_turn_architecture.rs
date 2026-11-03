use std::collections::HashMap;
use std::fs;
use std::path::Path;

use temper_authz::{AuthzEngine, SecurityContext};

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn agent_context(id: &str, agent_type: &str) -> SecurityContext {
    SecurityContext::from_resolved_identity(id, agent_type, None)
}

fn resource_attrs(pairs: &[(&str, serde_json::Value)]) -> HashMap<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn session_action<'a>(spec: &'a toml::Value, name: &str) -> &'a toml::value::Table {
    spec.get("action")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_table())
        .find(|a| a.get("name").and_then(|v| v.as_str()) == Some(name))
        .unwrap_or_else(|| panic!("session spec missing action {name}"))
}

/// Names of the `[[action.triggers]]` an action fires, in declaration order.
fn trigger_names(action: &toml::value::Table) -> Vec<&str> {
    action
        .get("triggers")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
        .collect()
}

/// The action's effect statements, e.g. `["input_tokens += 1"]`.
fn effect_statements(action: &toml::value::Table) -> Vec<&str> {
    action
        .get("effect")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect()
}

fn parse_session_spec(spec: &str) -> toml::Value {
    spec.parse::<toml::Value>()
        .expect("session.ioa.toml should parse as TOML")
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

    let parsed = parse_session_spec(&spec);
    for entry_point in [
        "WorkspaceReady",
        "SandboxReady",
        "ResumeWithPlanApproval",
        "RecoveryComplete",
    ] {
        assert_eq!(
            trigger_names(session_action(&parsed, entry_point)),
            ["prepare_context"],
            "session spec should route turn entry point {entry_point} through prepare_context"
        );
    }
}

#[test]
fn session_routes_llm_calls_through_codex_auth_gate() {
    let root = repo_root();
    let spec = fs::read_to_string(root.join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist");
    let csdl = fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml"))
        .expect("paw-agent model CSDL should exist");
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

    let parsed = parse_session_spec(&spec);
    let context_ready = session_action(&parsed, "ContextReady");
    assert!(
        context_ready.get("to").and_then(|v| v.as_str()) == Some("EnsuringProviderAuth")
            && trigger_names(context_ready) == ["ensure_provider_auth"],
        "ContextReady should persist prepared context then enter the provider auth gate"
    );
    let needs_compaction = session_action(&parsed, "NeedsCompaction");
    assert!(
        needs_compaction.get("to").and_then(|v| v.as_str()) == Some("EnsuringCompactionAuth")
            && effect_statements(needs_compaction) == ["input_tokens += 1", "output_tokens += 1"]
            && trigger_names(needs_compaction) == ["ensure_compaction_auth"],
        "NeedsCompaction should enter the compaction auth gate before calling the compactor"
    );
    assert!(
        spec.contains("name = \"compaction_skipped_reason\"")
            && spec.contains("name = \"compaction_skipped_leaf_id\"")
            && spec.contains("params = [\"session_leaf_id\", \"context_tokens\", \"system_prompt_hash\", \"system_prompt_file_id\", \"compaction_skipped_reason\", \"compaction_skipped_leaf_id\"]"),
        "CompactionComplete should persist explicit skip markers so PreparingContext cannot re-run the same impossible compaction forever"
    );
    let compaction_complete_action = csdl
        .split(r#"<Action Name="CompactionComplete" IsBound="true">"#)
        .nth(1)
        .and_then(|tail| tail.split("</Action>").next())
        .expect("CompactionComplete CSDL action should exist");
    for needle in [
        r#"<Parameter Name="compaction_skipped_reason" Type="Edm.String" Nullable="true"/>"#,
        r#"<Parameter Name="compaction_skipped_leaf_id" Type="Edm.String" Nullable="true"/>"#,
    ] {
        assert!(
            compaction_complete_action.contains(needle),
            "CompactionComplete CSDL action should expose skip marker parameter: {needle}"
        );
    }
    assert!(
        build_sh.contains("provider_auth_gate"),
        "provider_auth_gate must be built with the paw-agent wasm bundle"
    );
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
    ] {
        assert!(
            spec.contains(needle),
            "session spec should define provider-auth skipped fast path: {needle}"
        );
    }
    let parsed = parse_session_spec(&spec);
    assert_eq!(
        trigger_names(session_action(&parsed, "ContextReadyAuthSkipped")),
        ["call_provider"],
        "session spec should define provider-auth skipped fast path: ContextReadyAuthSkipped triggers call_provider"
    );
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
