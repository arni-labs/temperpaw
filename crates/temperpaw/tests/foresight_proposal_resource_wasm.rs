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
