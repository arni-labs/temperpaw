use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};

fn bytes(module: &str) -> Vec<u8> {
    let path=std::env::var("BASELINE_EXPAND_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm",env!("CARGO_MANIFEST_DIR")));
    std::fs::read(path).unwrap()
}
async fn invoke(engine: &WasmEngine, module: &str, fields: Value) -> Value {
    let hash = engine.compile_and_cache(&bytes(module)).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "run-fixture".into(),
        trigger_action: "Next".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":fields["transition_count"].as_u64().unwrap_or(0)},"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::new(),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    invoke_with_host(
        engine,
        ctx,
        Arc::new(SimWasmHost::new().with_default_response(500, "unexpected provider IO")),
        hash,
    )
    .await
}
async fn invoke_with_host(
    engine: &WasmEngine,
    ctx: WasmInvocationContext,
    host: Arc<dyn WasmHost>,
    hash: String,
) -> Value {
    let r = engine
        .invoke(
            &hash,
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
    serde_json::to_value(r).unwrap()
}

#[tokio::test]
#[ignore = "Captured private free-time fixture supplied explicitly"]
async fn later_finding_reconciles_current_baseline_atomically() {
    let path = std::env::var("BASELINE_REFRESH_FIXTURE").unwrap();
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let fields = fixture["fields"].clone();
    let generated = fixture["generated"].clone();
    let engine = WasmEngine::new().unwrap();
    let mut input = fields.clone();
    input["reasoning_result"] = json!(generated.to_string());
    let out = invoke(&engine, "semantic_expand", input).await;
    assert_eq!(out["callback_action"], "Expanded", "{out}");
    let p: Value =
        serde_json::from_str(out["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    let old: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        p["baseline_history"]
            .as_array()
            .expect(
                "accepted findings must reconcile current baseline and preserve its predecessor"
            )
            .last()
            .unwrap()["prior_baseline"],
        old["baseline"]
    );
    assert_ne!(p["baseline"], old["baseline"]);
    assert_eq!(
        out["callback_params"]["started_at_ms"],
        fields["started_at_ms"]
    );
    let before: Value = serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    let after: Value =
        serde_json::from_str(out["callback_params"]["snapshot_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        &after["nodes"].as_array().unwrap()[..before["nodes"].as_array().unwrap().len()],
        before["nodes"].as_array().unwrap()
    );
    for mode in ["omitted", "unknown", "future"] {
        let mut bad = generated.clone();
        match mode {
            "omitted" => {
                bad.as_object_mut().unwrap().remove("baseline");
            }
            "unknown" => bad["baseline"]["observed"][0]["evidence_ids"] = json!(["fabricated"]),
            _ => {
                bad["research_evidence"][0]["evidence_metadata"]["publication_date"] = json!("2031")
            }
        };
        let mut input = fields.clone();
        input["reasoning_result"] = json!(bad.to_string());
        let rejected = invoke(&engine, "semantic_expand", input).await;
        assert_eq!(
            rejected["callback_action"], "CompositionRejected",
            "{mode}: {rejected}"
        );
        assert!(rejected["callback_params"]["snapshot_json"].is_null());
        let cp: Value = serde_json::from_str(
            rejected["callback_params"]["program_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(cp["baseline"], old["baseline"]);
        assert_eq!(cp["results"], old["results"]);
    }
}
