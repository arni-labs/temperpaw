use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};
async fn invoke(engine: &WasmEngine, module: &str, fields: &Value, response: &Value) -> Value {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path=std::env::var_os("FORESIGHT_WASM_DIR").map(std::path::PathBuf::from).map(|p|p.join(format!("{module}.wasm"))).unwrap_or_else(||root.join(format!("os-apps/paw-foresight/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm")));
    let bytes = std::fs::read(path).unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "fixture".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "proposal-quality".into(),
        trigger_action: "Next".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":fields["transition_count"]},"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("temper_api_url".into(), "http://fixture".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let result = engine
        .invoke(
            &hash,
            &ctx,
            Arc::new(SimWasmHost::new().with_default_response(200, &response.to_string())),
            &WasmResourceLimits {
                max_memory: 256 * 1024 * 1024,
                max_fuel: 10_000_000_000,
                ..Default::default()
            },
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    serde_json::to_value(result).unwrap()
}

#[tokio::test]
#[ignore = "Requires the authorized captured games proposal checkpoint"]
async fn captured_unchecked_game_pool_keeps_reserved_deadline_reason() {
    let path = std::env::var("FORESIGHT_PROPOSAL_CAPTURE").expect("captured program JSON path");
    let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut before = saved.clone();
    // Reconstruct the scheduler input immediately before its captured finish callback.
    // Actual candidate/source/task contents remain unchanged; only finish outputs are undone.
    before["tasks"] = before["endpoint_proposal_attempt"]["tasks"].clone();
    before["endpoint_proposal_attempt"]["status"] = json!("checking");
    before["endpoint_proposal_attempt"]["checks"] = json!([]);
    before["endpoint_proposal_history"] = json!([]);
    before["stop_reason"] = json!("");
    assert_eq!(before["tasks"].as_array().unwrap().len(), 33);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let snapshot = json!({"world":before["endpoint_proposal_attempt"]["world"],"nodes":[]});
    let fields = json!({"snapshot_json":snapshot.to_string(),"program_json":before.to_string(),"trace_json":"[]","transition_count":67,"started_at_ms":now.saturating_sub(3_000_000).to_string()});
    let engine = WasmEngine::new().unwrap();
    let first = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(first["callback_action"], "SearchPlanned", "{first}");
    let after: Value =
        serde_json::from_str(first["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(after["stop_reason"], "time_budget");
    assert_eq!(after["endpoint_proposal_attempt"]["status"], "unresolved");
    assert_eq!(
        after["endpoint_proposal_attempt"]["checks"]
            .as_array()
            .unwrap()
            .len(),
        33
    );
    assert!(
        after["endpoint_proposal_attempt"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["result"].is_null() && c["evaluation"].is_null())
    );
    assert_eq!(after["results"], before["results"]);
    assert_eq!(after["evaluations"], before["evaluations"]);
    assert_eq!(after["proposal_pool"], before["proposal_pool"]);
    assert!(first["callback_params"].get("started_at_ms").is_none());
    let mut next = fields.clone();
    next["program_json"] = json!(after.to_string());
    let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
    assert_eq!(terminal["callback_action"], "Fail");
    let msg = terminal["callback_params"]["error_message"]
        .as_str()
        .unwrap();
    assert!(
        msg.contains("time_budget") && msg.contains("0 of 33"),
        "{msg}"
    );
    assert!(!msg.contains("did not produce sufficiently distinct"));
    for reason in [
        "call_budget",
        "transition_budget",
        "trace_budget",
        "provider_error",
    ] {
        let mut resource = after.clone();
        resource["stop_reason"] = json!(reason);
        next["program_json"] = json!(resource.to_string());
        let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
        assert_eq!(terminal["callback_action"], "Fail");
        assert!(
            terminal["callback_params"]["error_message"]
                .as_str()
                .unwrap()
                .contains(reason)
        );
    }
    let mut contrast = before.clone();
    contrast["proposal_pool"]["stage"] = json!("contrast");
    next["program_json"] = json!(contrast.to_string());
    let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
    assert_eq!(terminal["callback_action"], "Fail");
    assert!(
        terminal["callback_params"]["error_message"]
            .as_str()
            .unwrap()
            .contains("time_budget")
    );
    let mut quality = after.clone();
    quality["stop_reason"] = json!("endpoint_proposal_quality");
    next["program_json"] = json!(quality.to_string());
    let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
    assert!(
        terminal["callback_params"]["error_message"]
            .as_str()
            .unwrap()
            .contains("Endpoint proposal quality unresolved")
    );
}

#[tokio::test]
async fn contrast_resource_failure_never_reports_absent_or_stale_check_counts() {
    let engine = WasmEngine::new().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    for prior_attempt in [
        Value::Null,
        json!({"tasks":vec![json!({});33],"checks":vec![json!({"evaluation":{"type":"choice"}});33],"status":"examined"}),
    ] {
        let program = json!({"stage":"proposals","endpoint_proposal_contract":2,"proposal_pool":{"stage":"contrast"},"tasks":[],"cursor":0,"results":{},"endpoint_proposal_attempt":prior_attempt});
        let fields = json!({"snapshot_json":json!({"world":{},"nodes":[]}).to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":67,"started_at_ms":now.saturating_sub(3_000_000).to_string()});
        let out = invoke(&engine, "semantic_step", &fields, &json!({})).await;
        assert_eq!(out["callback_action"], "Fail", "{out}");
        let message = out["callback_params"]["error_message"].as_str().unwrap();
        assert!(
            message.contains("research remains incomplete") && message.contains("time_budget"),
            "{message}"
        );
        assert!(message.contains("No current comparison checks were scheduled"));
        assert!(!message.contains("0 of 0") && !message.contains("33 of 33"));
    }
}

#[tokio::test]
#[ignore = "Requires local deterministic deferred producer fixture"]
async fn deferred_native_completion_preserves_paths_and_original_clock() {
    let fixture: Value = serde_json::from_slice(&std::fs::read(std::env::var("FORESIGHT_DEFERRED_FIXTURE").unwrap()).unwrap()).unwrap();
    let engine = WasmEngine::new().unwrap();
    let mut program = fixture["passed"].clone();
    // Replay the actual step boundary with the recorded deterministic judgments.
    program["stage"] = json!("proposals");
    program["endpoint_proposal_attempt"]["status"] = json!("checking");
    program["tasks"] = program["endpoint_proposal_attempt"]["tasks"].clone();
    program["cursor"] = json!(program["tasks"].as_array().unwrap().len());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let original_start = now.saturating_sub(2_400_000).to_string();
    let fields = json!({"snapshot_json":fixture["snapshot"].to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":320,"started_at_ms":original_start});
    let mut pending_fields = fields.clone();
    pending_fields["program_json"] = json!(fixture["checking"].to_string());
    let scheduled = invoke(&engine, "semantic_step", &pending_fields, &json!({})).await;
    assert_eq!(scheduled["callback_action"], "Evaluate", "{scheduled}");
    assert!(scheduled["callback_params"].get("started_at_ms").is_none());
    let result = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(result["callback_action"], "SearchPlanned", "{result}");
    let completed: Value = serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(completed["endpoint_search"], program["endpoint_search"]);
    assert_eq!(completed["endpoint_novelty"], program["endpoint_novelty"]);
    assert_eq!(completed["deferred_novelty_recheck"]["status"], "completed");
    assert_eq!(completed["stage"], "exploration");
    assert_eq!(completed["transition_count"], 320);
    assert!(result["callback_params"].get("started_at_ms").is_none());
}

#[tokio::test]
#[ignore = "Requires authorized captured pass9 proposal checkpoint"]
async fn captured_checked_selection_enters_backward_without_rewriting_worlds() {
    let response: Value = serde_json::from_slice(&std::fs::read(std::env::var("FORESIGHT_PASS9_CAPTURE").unwrap()).unwrap()).unwrap();
    let rows: Value = serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let captured = &rows[0];
    let mut program: Value = serde_json::from_str(captured["program_json"].as_str().unwrap()).unwrap();
    let pair = program["endpoint_proposal_history"].as_array().unwrap().iter().find(|a|a["pool_stage"] == "pairs").unwrap().clone();
    let snapshot = json!({"world":pair["world"],"nodes":pair["source_evidence"]});
    // Reconstruct the recorded checkpoint before pair completion. Do not call
    // the provider or change the live run; recorded Jev checks remain unchanged.
    program["endpoint_proposal_attempt"] = pair.clone();
    program["endpoint_proposal_attempt"]["status"] = json!("checking");
    program["endpoint_proposal_history"] = json!([program["endpoint_proposal_history"][0]]);
    program["tasks"] = pair["tasks"].clone();
    program["cursor"] = json!(pair["tasks"].as_array().unwrap().len());
    program["stage"] = json!("proposals");
    program["proposal_pool"]["stage"] = json!("pairs");
    program["proposal_pool"]["candidates"] = pair["endpoints"].clone();
    program["proposal_pool"].as_object_mut().unwrap().remove("development");
    program.as_object_mut().unwrap().remove("endpoint_search");
    for check in pair["checks"].as_array().unwrap() {
        let task = &check["task"];
        program["results"][task["nodeId"].as_str().unwrap()][task["function"].as_str().unwrap()] = check["result"].clone();
        program["evaluations"][task["nodeId"].as_str().unwrap()][task["function"].as_str().unwrap()] = check["evaluation"].clone();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    // Offline simulator clock, not a live run retry or deadline change.
    let mut fields = json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":captured["counters"]["transition_count"],"started_at_ms":now.saturating_sub(1_800_000).to_string()});
    let engine = WasmEngine::new().unwrap();
    let selected = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(selected["callback_action"], "SearchPlanned", "{selected}");
    let frozen: Value = serde_json::from_str(selected["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(frozen["stage"], "exploration");
    assert!(frozen["proposal_pool"]["development"].is_null());
    assert_eq!(frozen["endpoint_novelty"], program["endpoint_novelty"]);
    for endpoint in frozen["endpoint_search"]["endpoints"].as_array().unwrap() {
        assert_eq!(Some(endpoint), pair["endpoints"].as_array().unwrap().iter().find(|e|e["id"]==endpoint["id"]));
    }
    fields["program_json"] = selected["callback_params"]["program_json"].clone();
    let next = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(next["callback_action"], "Reason", "{next}");
    assert_eq!(next["callback_params"]["phase"], "backward");
    assert!(next["callback_params"].get("started_at_ms").is_none());
}
