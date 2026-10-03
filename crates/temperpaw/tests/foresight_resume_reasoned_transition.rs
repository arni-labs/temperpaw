//! Native resume callback consumes a completed answer without a new child.
use serde_json::json;
use temper_jit::TransitionTable;
use temper_server::entity_actor::{EntityState, process_action};

#[test]
fn recovered_answer_enters_expand_once_and_restores_counters() {
    let _clock = temper_runtime::scheduler::install_deterministic_context(518);
    let table = TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"
    ));
    let mut state: EntityState = serde_json::from_value(json!({"entity_type":"SemanticRun","entity_id":"recovery","status":"Preparing","item_count":0,"fields":{}})).unwrap();
    let params = json!({"world_id":"w","reasoning_session_id":"saved-child","reasoning_result":"saved answer","started_at_ms":"123456","phase":"backward","program_json":"{}","snapshot_json":"{}","trace_json":"[]","agent_id":"a","model":"m","provider":"p","provider_options_json":"{}","transition_count":350,"reasoning_phase_polls":15,"reasoning_retry_count":2});
    let result = process_action(&mut state, &table, "ResumeReasoned", &params);
    assert!(result.success, "{result:?}");
    assert_eq!(state.status, "Expanding");
    assert!(result.spawn_requests.is_empty());
    let saved = serde_json::to_value(&state).unwrap();
    assert_eq!(saved["counters"]["transition_count"], 351);
    assert_eq!(saved["counters"]["reasoning_phase_polls"], 15);
    assert_eq!(saved["counters"]["reasoning_retry_count"], 2);
    assert_eq!(saved["fields"]["started_at_ms"], "123456");
    assert_eq!(saved["fields"]["reasoning_result"], "saved answer");
    assert!(!process_action(&mut state, &table, "ResumeReasoned", &params).success);
}

#[test]
fn reasoning_callbacks_persist_timing_program_without_resetting_parent_clock() {
    let _clock = temper_runtime::scheduler::install_deterministic_context(519);
    let table = TransitionTable::from_ioa_source(include_str!("../../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"));
    let mut state: EntityState = serde_json::from_value(json!({"entity_type":"SemanticRun","entity_id":"timing","status":"ReasoningSetup","item_count":0,"fields":{"started_at_ms":"123","program_json":"{}"}})).unwrap();
    let launched = json!({"reasoning_timing":{"phase":"backward","started_at_ms":1000}}).to_string();
    let result = process_action(&mut state, &table, "LaunchReasoning", &json!({"system_prompt":"contract","user_message":"question","tools_enabled":"","tool_choice":"none","max_turns":"1","program_json":launched}));
    assert!(result.success, "{result:?}");
    assert_eq!(serde_json::to_value(&state).unwrap()["fields"]["program_json"], launched);
    state.status = "Reasoning".into(); // Simulate the existing spawn callback, no child execution.
    let completed = json!({"reasoning_timing":{"phase":"backward","started_at_ms":1000,"completed":true},"reasoning_durations_ms":{"backward":900000}}).to_string();
    let result = process_action(&mut state, &table, "ReasoningComplete", &json!({"reasoning_result":"actual child answer","program_json":completed}));
    assert!(result.success, "{result:?}");
    let saved = serde_json::to_value(&state).unwrap();
    assert_eq!(state.status, "Expanding");
    assert_eq!(saved["fields"]["program_json"], completed);
    assert_eq!(saved["fields"]["started_at_ms"], "123");
    assert_eq!(saved["fields"]["reasoning_result"], "actual child answer");
}
