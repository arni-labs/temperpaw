//! Research permissions must persist before the reasoning child is spawned.
#[test]
fn reasoning_child_receives_exact_phase_tool_budget() {
    use serde_json::json;
    use temper_server::entity_actor::{EntityState, process_action};
    let _clock = temper_runtime::scheduler::install_deterministic_context(518);
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"),
    )
    .unwrap();
    let table = temper_jit::TransitionTable::from_ioa_source(&source);
    for (tools, choice, turns) in [
        ("temper_web_search,temper_web_fetch", "auto", "8"),
        ("", "none", "1"),
    ] {
        let mut state: EntityState = serde_json::from_value(json!({"entity_type":"SemanticRun","entity_id":"phase-test","status":"ReasoningSetup","item_count":0,"fields":{}})).unwrap();
        let saved = process_action(
            &mut state,
            &table,
            "LaunchReasoning",
            &json!({"system_prompt":"Investigate remaining premises","user_message":"Actual evidence","tools_enabled":tools,"tool_choice":choice,"max_turns":turns}),
        );
        assert!(saved.success, "{saved:?}");
        assert!(saved.spawn_requests.is_empty());
        let spawned = process_action(&mut state, &table, "SpawnReasoning", &json!({}));
        assert!(spawned.success);
        assert_eq!(spawned.spawn_requests.len(), 1);
        let fields = &spawned.spawn_requests[0].copied_field_values;
        assert_eq!(fields["tools_enabled"], tools);
        assert_eq!(fields["tool_choice"], choice);
        assert_eq!(fields["max_turns"], turns);
    }
}
