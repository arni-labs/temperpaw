//! Branch generation crosses actual reasoning, expansion and Jev guest boundaries.
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
async fn generation_routes_expose_branch_schema_and_catalog() {
    let engine = WasmEngine::new().unwrap();
    let snapshot = json!({"world":{"last_ingest_date":"2026-10-01","target_date":"2036-10-01"},"nodes":[{"Id":"h","kind":"scenario","statement":"An event","edges":"[]"}],"branches":[{"id":"prior","condition":{"kind":"all_occurring","event_ids":["h"]},"by":"2030-01-01"}]});
    for phase in ["explore", "challenge"] {
        let result = run(
            &engine,
            "semantic_reasoning",
            json!({"phase":phase,"snapshot_json":snapshot.to_string(),"program_json":"{}"}),
            Arc::new(SimWasmHost::new()),
        )
        .await;
        let params = &result["callback_params"];
        let prompt = params["system_prompt"].as_str().unwrap();
        let shape = prompt
            .split("Return JSON ONLY: ")
            .nth(1)
            .unwrap()
            .split(".\n")
            .next()
            .unwrap();
        let parsed: Value = serde_json::from_str(shape).unwrap();
        assert!(
            parsed["branches"].is_array(),
            "{phase}: producer schema must declare branches"
        );
        assert!(
            parsed["hypotheses"][0]["branch_id"].is_string(),
            "{phase}: consequence must attach branch"
        );
        assert!(prompt.contains("set branch_id on each consequence"));
        let input: Value = serde_json::from_str(params["user_message"].as_str().unwrap()).unwrap();
        assert_eq!(input["branches"][0]["id"], "prior");
    }
}
