//! Exercise actual JIT field projection and spawn-copy semantics at overflow boundaries.
use serde_json::{Value, json};
use temper_jit::TransitionTable;
use temper_server::entity_actor::{
    EntityState,
    effects::{FieldSyncMode, sync_fields_with_metadata},
    process_action,
};
fn table(source: &str) -> TransitionTable {
    let source = if std::env::var_os("ARN518_TEST_OLD_OVERFLOW").is_some() {
        source.replace("8388608", "131072")
    } else {
        source.to_owned()
    };
    TransitionTable::from_ioa_source(&source)
}
fn state(kind: &str, status: &str) -> EntityState {
    serde_json::from_value(json!({"entity_type":kind,"entity_id":"fixture","status":status,"item_count":0,"fields":{}})).unwrap()
}
fn project(state: &mut EntityState, table: &TransitionTable, field: &str, payload: &str) {
    let params = json!({field:payload});
    let blobs = sync_fields_with_metadata(
        state,
        &params,
        FieldSyncMode::blob_refs_default(),
        Some(&table.state_var_metadata),
    );
    assert!(
        blobs.is_empty(),
        "{field} unexpectedly offloaded at {} bytes",
        payload.len()
    );
    assert_eq!(
        state.fields[field].as_str(),
        Some(payload),
        "{field} lost machine-readable text"
    );
}
#[test]
fn prompts_and_results_survive_real_projection_and_spawn() {
    let _clock = temper_runtime::scheduler::install_deterministic_context(518);
    let parent = table(include_str!(
        "../../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"
    ));
    let child = table(include_str!(
        "../../../os-apps/paw-agent/specs/session.ioa.toml"
    ));
    // JSON input is itself a string field: quotes/backslashes expand at persistence.
    for payload in ["x".repeat(150_130), "\\\"".repeat(3 * 1024 * 1024 / 2)] {
        let mut p = state("SemanticRun", "ReasoningSetup");
        let saved = process_action(
            &mut p,
            &parent,
            "LaunchReasoning",
            &json!({"system_prompt":"Research","user_message":payload,"tools_enabled":"","tool_choice":"none","max_turns":"1"}),
        );
        assert!(saved.success);
        project(&mut p, &parent, "user_message", &payload);
        let spawned = process_action(&mut p, &parent, "SpawnReasoning", &json!({}));
        assert!(spawned.success);
        assert_eq!(spawned.spawn_requests.len(), 1);
        let copied = &spawned.spawn_requests[0].copied_field_values;
        assert_eq!(copied["user_message"].as_str(), Some(payload.as_str()));
        let mut c = state("Session", "Created");
        let configured =
            process_action(&mut c, &child, "Configure", &Value::Object(copied.clone()));
        assert!(configured.success, "{configured:?}");
        project(&mut c, &child, "user_message", &payload);
    }
    let result = json!({"hypotheses":[],"detail":"x".repeat(200_000)}).to_string();
    project(
        &mut state("Session", "Completed"),
        &child,
        "result",
        &result,
    );
    for field in ["reasoning_result", "answer"] {
        project(
            &mut state("SemanticRun", "Expanding"),
            &parent,
            field,
            &result,
        );
    }
    let request = json!({"state":"\\\"".repeat(30_000)}).to_string();
    assert!(request.len() < 128 * 1024); // The field's serialized representation exceeds default inline size.
    project(
        &mut state("SemanticRun", "Choosing"),
        &parent,
        "request_json",
        &request,
    );
}
