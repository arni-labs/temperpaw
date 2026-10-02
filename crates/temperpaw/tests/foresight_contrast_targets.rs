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
        integration_config: BTreeMap::from([("typesafe_api_key".into(), "fixture-only".into())]),
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
async fn contrast_prompt_targets_current_four_not_historical_ten() {
    let ids: Vec<_> = (1..=4).map(|i| format!("current-{i}")).collect();
    let candidates: Vec<_> = ids
        .iter()
        .map(|id| json!({"id":id,"statement":"Developed current world"}))
        .collect();
    let historical: Vec<_> = (1..=6)
        .map(|i| json!({"id":format!("historical-{i}")}))
        .chain(candidates.iter().cloned())
        .collect();
    let program = json!({"endpoint_proposal_contract":2,"world_search_contract":1,"proposal_pool":{"stage":"contrast","candidates":candidates},"endpoint_proposal_history":[{"endpoints":historical}],"response_correction":{"attempt":2,"validation_error":"Extra historical candidates","rejected_draft":"Old ten-candidate draft"}});
    let fields = json!({"phase":"explore","snapshot_json":json!({"world":{"description":"How might games change?","target_date":"2030-12-31","last_ingest_date":"2026-10-03"},"nodes":[]}).to_string(),"program_json":program.to_string()});
    let result = invoke(
        &WasmEngine::new().unwrap(),
        "semantic_reasoning",
        &fields,
        &json!({}),
    )
    .await;
    assert_eq!(result["callback_action"], "LaunchReasoning", "{}", result);
    let input: Value =
        serde_json::from_str(result["callback_params"]["user_message"].as_str().unwrap()).unwrap();
    assert_eq!(input["contrast_target_ids"], json!(ids));
    let prompt = result["callback_params"]["system_prompt"].as_str().unwrap();
    assert!(prompt.contains("Authoritative current contrast targets: [\"current-1\",\"current-2\",\"current-3\",\"current-4\"]"));
    assert!(prompt.contains("audit history, not additional targets"));
    assert!(prompt.contains("Do not revive historical candidates"));
    assert!(
        input.to_string().contains("historical-6"),
        "audit history remains available"
    );
}
