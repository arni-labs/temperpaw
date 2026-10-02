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
const OBSERVED: &str = "OpenAI Codex stream failed after visible output or final attempt: OpenAI SSE stream ended before response.completed";
#[tokio::test]
async fn terminal_sse_retry_preserves_original_limits() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let engine = WasmEngine::new().unwrap();
    for (status, error, retries, started, transitions, expected) in [
        ("Failed", OBSERVED, 0, now.as_str(), 0, "ReasoningRetry"),
        ("Failed", OBSERVED, 2, now.as_str(), 0, "ReasoningRetry"),
        ("Failed", OBSERVED, 3, now.as_str(), 0, "Fail"),
        ("Cancelled", OBSERVED, 0, now.as_str(), 0, "Fail"),
        ("Failed", OBSERVED, 0, "1", 0, "Fail"),
        ("Failed", OBSERVED, 0, now.as_str(), 480, "Fail"),
        (
            "Failed",
            "OpenAI Codex stream failed after visible output or final attempt: invalid JSON",
            0,
            now.as_str(),
            0,
            "Fail",
        ),
        (
            "Failed",
            "HTTP 500: permission denied",
            0,
            now.as_str(),
            0,
            "Fail",
        ),
    ] {
        let fields = json!({"reasoning_session_id":"failed-child","started_at_ms":started,"transition_count":transitions,"reasoning_retry_count":retries});
        let result = invoke(
            &engine,
            "semantic_session",
            &fields,
            &json!({"Status":status,"error_message":error}),
        )
        .await;
        assert_eq!(result["callback_action"], expected, "{result}");
        if expected == "ReasoningRetry" {
            assert_eq!(
                result["callback_params"],
                json!({"last_retry_error":OBSERVED,"last_retry_session_id":"failed-child"})
            );
        }
    }
}
