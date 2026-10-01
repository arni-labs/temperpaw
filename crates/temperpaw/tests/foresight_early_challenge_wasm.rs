//! Actual guest proof of bounded independent batching and preserved dependencies.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
#[allow(dead_code)]
async fn invoke(
    engine: &WasmEngine,
    hash: &str,
    fields: Value,
    status: u16,
    response: &Value,
) -> Value {
    let host = SimWasmHost::new().with_default_response(status, &response.to_string());
    invoke_host(engine, hash, fields, Arc::new(host), "Recorded").await
}
async fn invoke_host(
    engine: &WasmEngine,
    hash: &str,
    fields: Value,
    host: Arc<dyn WasmHost>,
    expected_action: &str,
) -> Value {
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "retry-fixture".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some("semantic_call".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":fields,"counters":{"transition_count":fields["_transition_count"].as_u64().unwrap_or(0)}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("typesafe_api_key".into(), "fixture-key".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let r = engine
        .invoke(
            hash,
            &ctx,
            host,
            &WasmResourceLimits {
                max_memory: 256 * 1024 * 1024,
                max_fuel: 10_000_000_000,
                ..Default::default()
            },
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    assert_eq!(
        r.callback_action, expected_action,
        "{:?}",
        r.callback_params
    );
    r.callback_params
}

fn module(engine: &WasmEngine, name: &str) -> String {
    let path=if name=="semantic_step" { std::env::var("ARN518_CHALLENGE_STEP").ok() }else{None}.unwrap_or_else(||format!("{}/../../os-apps/paw-foresight/wasm/{name}/target/wasm32-unknown-unknown/release/{name}.wasm",env!("CARGO_MANIFEST_DIR")));
    engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap()
}
#[tokio::test]
async fn reserved_challenge_interrupts_pending_work_and_replans_without_losing_cached_judgments() {
    let engine = WasmEngine::new().unwrap();
    let hash = module(&engine, "semantic_step");
    let snapshot = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"e","kind":"evidence","statement":"Observed baseline","edges":"[]"},{"Id":"h","kind":"scenario","statement":"A future event by2030","edges":"[]"}]});
    let tasks = json!([{"nodeId":"h","function":"classify_temporal","depth":0},{"nodeId":"h","function":"classify_gap","depth":0},{"nodeId":"h","function":"estimate_likelihood","depth":0},{"nodeId":"h","function":"evaluate_novelty","depth":0},{"nodeId":"h","function":"decision_value","depth":0}]);
    let original = json!({"stage":"exploration","baseline_status":"established","baseline":{"as_of":"2026-09-30"},"cursor":2,"tasks":tasks,"continue_exploring":true,"round":3,"rounds":[],"http_calls":41,"evidence_ids":["e"],"results":{"e":{"classify_gap":"none"},"h":{"classify_temporal":"future_change","classify_gap":"evidence"}},"evaluations":{"e":{"classify_gap":{"type":"choice"}},"h":{"classify_gap":{"type":"choice","context":{"evidence_ids":["e"]}}}}});
    let trace = json!([{"index":0,"nodeId":"h","function":"classify_gap","decision":"evidence"}]);
    let mut fields = json!({"_transition_count":156,"started_at_ms":"9999999999999","snapshot_json":snapshot.to_string(),"program_json":original.to_string(),"trace_json":trace.to_string()});
    let mut completed_round = fields.clone();
    completed_round["_transition_count"] = json!(150);
    let mut completed_program = original.clone();
    completed_program["cursor"] = json!(5);
    completed_round["program_json"] = json!(completed_program.to_string());
    let anticipatory = invoke_host(
        &engine,
        &hash,
        completed_round,
        Arc::new(SimWasmHost::new()),
        "Reason",
    )
    .await;
    assert_eq!(
        anticipatory["phase"], "challenge",
        "another generation phase could jump over the reserved window"
    );
    let anticipatory_program: Value =
        serde_json::from_str(anticipatory["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        anticipatory_program["independent_challenge"]["trigger"],
        "reserved_before_next_exploration"
    );
    let out = invoke_host(
        &engine,
        &hash,
        fields.clone(),
        Arc::new(SimWasmHost::new()),
        "Reason",
    )
    .await;
    assert_eq!(out["phase"], "challenge");
    assert_eq!(out["reasoning_phase_polls"], 0);
    let p: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    for key in [
        "tasks",
        "cursor",
        "results",
        "evaluations",
        "http_calls",
        "round",
    ] {
        assert_eq!(p[key], original[key], "{key}");
    }
    assert_eq!(
        serde_json::from_str::<Value>(out["trace_json"].as_str().unwrap()).unwrap(),
        trace
    );
    assert!(out.get("started_at_ms").is_none());
    assert_eq!(
        p["independent_challenge"]["trigger"],
        "reserved_transition_window"
    );
    fields["program_json"] = out["program_json"].clone();
    fields["phase"] = json!("challenge");
    fields["reasoning_result"]=json!(json!({"premises_challenged":[{"assumption":"The existing mechanism remains necessary","alternative":"A different mechanism performs the purpose","prior_hypothesis_ids":["ref_0002"],"alternative_hypothesis_ids":["alternative"]}],"hypotheses":[{"id":"alternative","statement":"Another mechanism succeeds by2030","requires":["ref_0001"]}],"research_evidence":[],"continue_exploring":true,"exploration_note":"Challenge the premise"}).to_string());
    let expanded = invoke_host(
        &engine,
        &module(&engine, "semantic_expand"),
        fields.clone(),
        Arc::new(SimWasmHost::new()),
        "Expanded",
    )
    .await;
    assert_eq!(expanded["started_at_ms"], fields["started_at_ms"]);
    assert!(expanded.get("trace_json").is_none());
    let after: Value = serde_json::from_str(expanded["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(after["results"], original["results"]);
    assert_eq!(after["evaluations"], original["evaluations"]);
    assert_eq!(after["http_calls"], 41);
    assert_eq!(after["independent_challenge"]["status"], "completed");
    for f in ["estimate_likelihood", "evaluate_novelty", "decision_value"] {
        assert!(
            after["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == f)
        );
    }
    for f in ["classify_temporal", "classify_gap"] {
        assert!(
            !after["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == f)
        );
    }
    assert!(
        after["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["nodeId"] == "r4-alternative" && t["function"] == "classify_temporal")
    );
    let updated: Value = serde_json::from_str(expanded["snapshot_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        &updated["nodes"].as_array().unwrap()[..2],
        snapshot["nodes"].as_array().unwrap()
    );
    fields["program_json"] = expanded["program_json"].clone();
    fields["snapshot_json"] = expanded["snapshot_json"].clone();
    fields["_transition_count"] = json!(180);
    invoke_host(
        &engine,
        &hash,
        fields,
        Arc::new(SimWasmHost::new()),
        "Evaluate",
    )
    .await;
}

#[tokio::test]
async fn late_research_is_not_admitted_when_it_cannot_recheck_existing_evidence() {
    let engine = WasmEngine::new().unwrap();
    let hash = module(&engine, "semantic_step");
    let snapshot = json!({"world":{"Id":"w"},"nodes":[{"Id":"e","kind":"evidence","statement":"Observed baseline","edges":"[]"},{"Id":"a","kind":"scenario","statement":"Event A by2030","edges":"[]"},{"Id":"b","kind":"scenario","statement":"Event B by2030","edges":"[]"},{"Id":"c","kind":"scenario","statement":"Event C by2030","edges":"[]"}]});
    let p = json!({"stage":"exploration","baseline_status":"established","baseline":{"as_of":"2026-09-30"},"cursor":0,"tasks":[],"continue_exploring":true,"independent_challenge":{"status":"completed"},"results":{"a":{"classify_temporal":"future_change","estimate_likelihood":"0.4"},"b":{"classify_temporal":"future_change","estimate_likelihood":"0.6"},"c":{"classify_temporal":"future_change","estimate_likelihood":"0.5"}},"evaluations":{"a":{},"b":{},"c":{}},"evidence_ids":["e"],"http_calls":76});
    let fields = json!({"_transition_count":170,"started_at_ms":"9999999999999","snapshot_json":snapshot.to_string(),"program_json":p.to_string(),"trace_json":"[]"});
    let out = invoke_host(
        &engine,
        &hash,
        fields.clone(),
        Arc::new(SimWasmHost::new()),
        "SearchPlanned",
    )
    .await;
    let planned: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(planned["stage"], "combinations");
    assert_eq!(planned["exploration_admission"]["admitted"], false);
    assert_eq!(planned["exploration_admission"]["current_graph_tasks"], 16);
    assert_eq!(planned["results"], p["results"]);
    assert_eq!(planned["http_calls"], 76);
    assert_eq!(planned["evidence_ids"], p["evidence_ids"]);
    assert!(out.get("snapshot_json").is_none());
    assert!(out.get("trace_json").is_none());
    assert!(out.get("started_at_ms").is_none());
    let mut missing = fields.clone();
    let mut no_current = p.clone();
    no_current["results"] = json!({});
    no_current["evaluations"] = json!({});
    missing["program_json"] = json!(no_current.to_string());
    let failed = invoke_host(
        &engine,
        &hash,
        missing,
        Arc::new(SimWasmHost::new()),
        "Fail",
    )
    .await;
    assert!(
        failed["error_message"]
            .as_str()
            .unwrap()
            .contains("only 0 current eligible")
    );
    let mut first = fields.clone();
    first["snapshot_json"] =
        json!(json!({"world":snapshot["world"],"nodes":[snapshot["nodes"][0]]}).to_string());
    first["program_json"] = json!(no_current.to_string());
    let initial = invoke_host(
        &engine,
        &hash,
        first,
        Arc::new(SimWasmHost::new()),
        "Reason",
    )
    .await;
    assert_eq!(initial["phase"], "explore");
    let initial_program: Value =
        serde_json::from_str(initial["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        initial_program["exploration_admission"]["reason"],
        "initial_hypotheses_required"
    );
    let mut early = fields;
    early["_transition_count"] = json!(20);
    let permitted = invoke_host(
        &engine,
        &hash,
        early,
        Arc::new(SimWasmHost::new()),
        "Reason",
    )
    .await;
    assert_eq!(permitted["phase"], "explore");
    let admitted: Value =
        serde_json::from_str(permitted["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(admitted["exploration_admission"]["admitted"], true);
}

#[tokio::test]
#[ignore = "Requires captured later snapshot and disclosed admission reconstruction"]
async fn reconstructed_real_admission_reclaims_polling_overhead() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("ARN518_POLL_ADMISSION_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let snapshot = fixture["snapshot"].clone();
    let mut program = fixture["program"].clone();
    let observed = program["exploration_admission"].clone();
    assert_eq!(observed["required_transitions"], 126);
    assert_eq!(observed["remaining_transitions"], 105);
    program["stage"] = json!("exploration");
    program["cursor"] = json!(0);
    program["tasks"] = json!([]);
    program["stop_reason"] = json!("round_evaluated");
    program["continue_exploring"] = json!(true);
    program["independent_challenge"] = json!({"status":"completed"});
    let engine = WasmEngine::new().unwrap();
    let hash = module(&engine, "semantic_step");
    let expect_old = std::env::var("ARN518_EXPECT_OLD_BUDGET").is_ok();
    let result=invoke_host(&engine,&hash,json!({"_transition_count":87,"started_at_ms":"9999999999999","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]"}),Arc::new(SimWasmHost::new()),"Reason").await;
    assert_eq!(
        result["phase"],
        if expect_old { "compose" } else { "explore" }
    );
    let next: Value = serde_json::from_str(result["program_json"].as_str().unwrap()).unwrap();
    let admission = &next["exploration_admission"];
    assert_eq!(admission["estimated_batches"], 15);
    assert_eq!(admission["current_graph_tasks"], 107);
    assert_eq!(admission["current_graph_evaluation_transitions"], 30);
    assert_eq!(
        admission["reasoning_reserve"],
        if expect_old { 64 } else { 44 }
    );
    assert_eq!(
        admission["required_transitions"],
        if expect_old { 126 } else { 106 }
    );
    assert_eq!(
        admission["remaining_transitions"],
        if expect_old { 105 } else { 145 }
    );
    assert_eq!(admission["admitted"], !expect_old);
    assert_eq!(next["results"], program["results"]);
    assert_eq!(next["evaluations"], program["evaluations"]);
    assert_eq!(next["evidence_ids"], program["evidence_ids"]);
    assert!(result.get("started_at_ms").is_none());
}
