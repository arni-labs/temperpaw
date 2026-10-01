//! Native guard/backoff preserves the checkpoint and limits provider retries.
use serde_json::json;
use temper_server::entity_actor::{EntityState, process_action};
#[test]
fn retry_is_bounded_scheduled_and_preserves_checkpoint() {
    let _clock = temper_runtime::scheduler::install_deterministic_context(518);
    let table = temper_jit::TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"
    ));
    let checkpoint = json!({"phase":"explore","snapshot_json":"snapshot","program_json":"program","trace_json":"trace","user_message":"same prompt","started_at_ms":"123"});
    let mut state:EntityState=serde_json::from_value(json!({"entity_type":"SemanticRun","entity_id":"retry","status":"Reasoning","item_count":0,"fields":checkpoint,"counters":{"reasoning_retry_count":0}})).unwrap();
    for attempt in 1..=3 {
        let result = process_action(
            &mut state,
            &table,
            "ReasoningRetry",
            &json!({"last_retry_error":"OpenAI Codex API returned503","last_retry_session_id":"failed-child"}),
        );
        assert!(result.success, "attempt {attempt}: {result:?}");
        assert_eq!(state.status, "ReasoningSetup");
        assert_eq!(state.counters["reasoning_retry_count"], attempt);
        assert_eq!(result.scheduled_actions.len(), 1);
        let schedule = serde_json::to_value(&result.scheduled_actions[0]).unwrap();
        assert_eq!(schedule["action"], "SpawnReasoning");
        assert_eq!(schedule["delay_seconds"], 15);
        for (key, value) in checkpoint.as_object().unwrap() {
            assert_eq!(&state.fields[key], value);
        }
        state.status = "Reasoning".into();
    }
    let denied = process_action(&mut state, &table, "ReasoningRetry", &json!({}));
    assert!(!denied.success);
    assert_eq!(state.counters["reasoning_retry_count"], 3);
}
#[test]
fn resume_with_pending_tasks_returns_to_evaluation() {
    let _clock = temper_runtime::scheduler::install_deterministic_context(519);
    let table = temper_jit::TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"
    ));
    let mut state:EntityState=serde_json::from_value(json!({"entity_type":"SemanticRun","entity_id":"resume","status":"Preparing","item_count":0,"fields":{}})).unwrap();
    let result = process_action(
        &mut state,
        &table,
        "ResumePrepared",
        &json!({"world_id":"w","snapshot_json":"snapshot","program_json":"pending tasks","trace_json":"preserved calls","started_at_ms":"123","agent_id":"agent","model":"model","provider":"provider","phase":"explore"}),
    );
    assert!(result.success, "{result:?}");
    assert_eq!(state.status, "Choosing");
    assert_eq!(state.fields["phase"], "explore");
    assert_eq!(state.fields["program_json"], "pending tasks");
}
