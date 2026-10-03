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
