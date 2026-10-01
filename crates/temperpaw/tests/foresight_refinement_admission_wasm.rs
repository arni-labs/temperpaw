//! Captured completed-pass admission and interruption proof through actual step WASM.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
fn artifact(engine: &WasmEngine, module: &str) -> String {
    let path=std::env::var(format!("ARN518_BRANCH_{}",module.to_uppercase())).unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm",env!("CARGO_MANIFEST_DIR")));
    engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap()
}
async fn run(engine: &WasmEngine, module: &str, fields: Value, host: Arc<dyn WasmHost>) -> Value {
    let context = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "fixture".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":fields["transition_count"].as_u64().unwrap_or(0)},"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([
            ("temper_api_url".into(), "http://fixture".into()),
            ("typesafe_api_key".into(), "fixture-only".into()),
        ]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    serde_json::to_value(
        engine
            .invoke(
                &artifact(engine, module),
                &context,
                host,
                &WasmResourceLimits {
                    max_memory: 256 * 1024 * 1024,
                    max_fuel: 10_000_000_000,
                    ..Default::default()
                },
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "Requires captured completed checkpoint and trace paths"]
async fn captured_complete_pass_admits_nominal_work_without_inventing_completion() {
    let c: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("REFINEMENT_CHECKPOINT").unwrap()).unwrap(),
    )
    .unwrap();
    let trace: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("REFINEMENT_TRACE").unwrap()).unwrap())
            .unwrap();
    let mut p = c["program"].clone();
    let old_history = p["world_refinement"].clone();
    p["stop_reason"] = json!("");
    let fields = json!({"phase":"compose","snapshot_json":c["snapshot"].to_string(),"program_json":p.to_string(),"trace_json":trace.to_string(),"started_at_ms":c["started_at_ms"],"transition_count":110});
    let engine = WasmEngine::new().unwrap();
    let next = run(
        &engine,
        "semantic_step",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(
        next["callback_action"], "SearchPlanned",
        "{}",
        next["callback_action"]
    );
    let admitted: Value =
        serde_json::from_str(next["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        admitted["refinement_admission"]["required_transitions"],
        123
    );
    assert_eq!(admitted["refinement_admission"]["estimated_batches"], 59);
    assert_eq!(admitted["world_pass"], 2);
    for id in admitted["active_world_ids"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        assert_eq!(
            admitted["world_refinement"][id]["rounds"],
            old_history[id]["rounds"]
        );
        assert!(admitted["results"][id]["estimate_likelihood"].is_null());
    }
    let mut low = fields.clone();
    low["transition_count"] = json!(435);
    let stopped = run(&engine, "semantic_step", low, Arc::new(SimWasmHost::new())).await;
    let declined: Value =
        serde_json::from_str(stopped["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(declined["results"], p["results"]);
    assert_eq!(declined["evaluations"], p["evaluations"]);
    assert_eq!(declined["world_pass"], 1);
    let mut interrupted = fields;
    let mut partial = admitted;
    partial["stop_reason"] = json!("provider_error");
    partial["last_error"] = json!("Two transient retries exhausted");
    interrupted["program_json"] = json!(partial.to_string());
    let stop = run(
        &engine,
        "semantic_step",
        interrupted,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(stop["callback_action"], "Reason");
    assert_eq!(stop["callback_params"]["phase"], "synthesize");
    let end: Value =
        serde_json::from_str(stop["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    for id in end["active_world_ids"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        assert_eq!(
            end["world_refinement"][id]["rounds"][0],
            old_history[id]["rounds"][0]
        );
        assert_eq!(end["world_refinement"][id]["rounds"][1]["complete"], false);
        assert!(end["world_refinement"][id]["rounds"][1]["probability"].is_null());
    }
}
