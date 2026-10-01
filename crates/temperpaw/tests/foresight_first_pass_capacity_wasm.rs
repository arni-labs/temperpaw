use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};

fn bytes(_module: &str) -> Vec<u8> {
    std::fs::read(std::env::var("WASM").unwrap()).unwrap()
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
        module,
        ctx,
        Arc::new(SimWasmHost::new().with_default_response(500, "unexpected provider IO")),
        hash,
    )
    .await
}
async fn invoke_with_host(
    engine: &WasmEngine,
    _module: &str,
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

// Private captured fixture is supplied explicitly; no provider IO is permitted.
#[tokio::test]
#[ignore = "requires captured composition fields and built expand WASM"]
async fn captured_first_world_pass_must_fit_before_commit() {
    let fields: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("FIELDS").unwrap()).unwrap()).unwrap();
    let engine = WasmEngine::new().unwrap();
    let rejected = invoke(&engine, "semantic_expand", fields.clone()).await;
    assert_eq!(rejected["callback_action"], "CompositionRejected");
    assert!(rejected["callback_params"]["snapshot_json"].is_null());
    let rejected_program: Value = serde_json::from_str(
        rejected["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        rejected_program["transition_count"],
        fields["transition_count"]
    );
    assert!(
        rejected_program["composition_correction"]["validation_error"]
            .as_str()
            .unwrap()
            .contains("complete first audit pass")
    );
    let mut smaller = fields.clone();
    let mut proposal: Value =
        serde_json::from_str(smaller["reasoning_result"].as_str().unwrap()).unwrap();
    proposal["worlds"].as_array_mut().unwrap().truncate(2);
    smaller["reasoning_result"] = json!(proposal.to_string());
    let accepted = invoke(&engine, "semantic_expand", smaller.clone()).await;
    assert_eq!(accepted["callback_action"], "Expanded", "{accepted}");
    assert_eq!(
        accepted["callback_params"]["started_at_ms"],
        fields["started_at_ms"]
    );
    let program: Value = serde_json::from_str(
        accepted["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(program["active_world_ids"].as_array().unwrap().len(), 2);
    assert_eq!(program["first_world_pass_admission"]["admitted"], true);
    let before: Value = serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    let after: Value = serde_json::from_str(
        accepted["callback_params"]["snapshot_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        &after["nodes"].as_array().unwrap()[..before["nodes"].as_array().unwrap().len()],
        before["nodes"].as_array().unwrap()
    );
    // A stale program counter cannot admit the same set at a later live counter.
    smaller["transition_count"] = json!(430);
    let late = invoke(&engine, "semantic_expand", smaller).await;
    assert_eq!(late["callback_action"], "CompositionRejected");
    let late_program: Value =
        serde_json::from_str(late["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(late_program["transition_count"], 430);
}
