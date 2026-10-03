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
async fn actual_child_progress_resets_idle_only_without_resetting_global_limits() {
    let engine = WasmEngine::new().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let base = json!({"reasoning_session_id":"current","reasoning_progress_session_id":"current","reasoning_progress_token":"7","started_at_ms":now.to_string(),"transition_count":31});
    for (token, expected) in [
        (json!(8), "ReasoningProgress"),
        (json!(7), "ReasoningPending"),
        (json!(6), "ReasoningPending"),
        (json!(0), "ReasoningPending"),
        (Value::Null, "ReasoningPending"),
        (json!("invalid"), "ReasoningPending"),
    ] {
        let child = json!({"Status":"CallingProvider","progress_token":token,"last_progress_at":now.to_string(),"last_heartbeat_at":now.to_string()});
        let out = invoke(&engine, "semantic_session", &base, &child).await;
        assert_eq!(out["callback_action"], expected, "{out}");
        assert!(out["callback_params"].get("started_at_ms").is_none());
        assert!(out["callback_params"].get("transition_count").is_none());
        if expected == "ReasoningProgress" {
            assert_eq!(
                out["callback_params"],
                json!({"reasoning_progress_session_id":"current","reasoning_progress_token":"8"})
            );
            let mut saved = base.clone();
            saved["reasoning_progress_token"] = json!("8");
            assert_eq!(
                invoke(&engine, "semantic_session", &saved, &child).await["callback_action"],
                "ReasoningPending"
            );
        }
    }
    let mut next = base.clone();
    next["reasoning_session_id"] = json!("new-child");
    let child = json!({"Status":"CallingProvider","ProgressToken":1});
    assert_eq!(
        invoke(&engine, "semantic_session", &next, &child).await["callback_action"],
        "ReasoningProgress"
    );
    for (started, transitions) in [(now.saturating_sub(3_600_000), 31), (now, 480)] {
        let mut exhausted = base.clone();
        exhausted["started_at_ms"] = json!(started.to_string());
        exhausted["transition_count"] = json!(transitions);
        let out = invoke(
            &engine,
            "semantic_session",
            &exhausted,
            &json!({"Status":"CallingProvider","progress_token":99}),
        )
        .await;
        assert_eq!(out["callback_action"], "Fail", "{out}");
        let expected_error = if transitions == 480 {
            "Native transition budget exhausted"
        } else {
            "Semantic run time budget exhausted"
        };
        assert!(
            out["callback_params"]["error_message"]
                .as_str()
                .unwrap()
                .contains(expected_error)
        );
    }
    for (status, expected) in [
        ("Completed", "ReasoningComplete"),
        ("Failed", "Fail"),
        ("Cancelled", "Fail"),
    ] {
        let out = invoke(
            &engine,
            "semantic_session",
            &base,
            &json!({"Status":status,"result":"complete answer","progress_token":99}),
        )
        .await;
        assert_eq!(out["callback_action"], expected);
    }
}
