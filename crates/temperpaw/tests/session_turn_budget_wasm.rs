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
    let path=std::env::var_os("SESSION_WASM_DIR").map(std::path::PathBuf::from).map(|p|p.join(format!("{module}.wasm"))).unwrap_or_else(||root.join(format!("os-apps/paw-agent/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm")));
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
async fn context_final_handoff_preserves_completed_results_and_json_contract() {
    let engine = WasmEngine::new().unwrap();
    let messages = json!([
        {"role":"user","content":"Question"},
        {"role":"assistant","content":[{"type":"tool_use","id":"search-1","name":"temper_web_search","input":{"query":"mechanism"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"search-1","content":"Actual retrieved evidence with unresolved limitations"}]}
    ]);
    for (limit, turns, final_turn) in [
        (json!("12"), 11, false),
        (json!("12"), 12, true),
        (json!("12"), 13, true),
        (json!("0"), 33, false),
        (Value::Null, 33, false),
    ] {
        let fields = json!({"model":"fixture","provider":"openai","user_message":"Question","system_prompt":"Return exact JSON with findings and unknowns.","conversation":messages.to_string(),"tools_enabled":"temper_web_search,temper_web_fetch","max_turns":limit,"turn_count":turns});
        let out = invoke(&engine, "context_preparer", &fields, &json!({"value":[]})).await;
        assert_eq!(out["callback_action"], "ContextReadyAuthSkipped", "{out}");
        let artifact: Value = serde_json::from_str(
            out["callback_params"]["prepared_context_inline_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(artifact["tools"].as_array().unwrap().is_empty(), final_turn);
        assert!(
            artifact["system_prompt"]
                .as_str()
                .unwrap()
                .contains("Return exact JSON with findings and unknowns.")
        );
        assert_eq!(
            &artifact["messages"].as_array().unwrap()[..3],
            messages.as_array().unwrap()
        );
        assert_eq!(
            artifact["messages"].as_array().unwrap().len(),
            if final_turn { 4 } else { 3 }
        );
        if final_turn {
            assert!(
                artifact["messages"][3]["content"]
                    .as_str()
                    .unwrap()
                    .contains("No further tools")
            );
        }
    }
}
