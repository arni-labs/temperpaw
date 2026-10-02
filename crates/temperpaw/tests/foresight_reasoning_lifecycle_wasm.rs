//! Monitor lifecycle with synthetic child responses; no provider sessions are created.
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};

#[tokio::test]
#[ignore = "requires locally built semantic_session WASM"]
async fn active_child_survives_ten_polls_without_resetting_global_limits() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bytes = std::fs::read(root.join("os-apps/paw-foresight/wasm/semantic_session/target/wasm32-unknown-unknown/release/semantic_session.wasm")).unwrap();
    let engine = WasmEngine::new().unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let started = now - 1_000;
    let ctx = WasmInvocationContext {
        tenant: "fixture".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "run-test".into(),
        trigger_action: "CheckReasoning".into(),
        wasm_module: Some("semantic_session".into()),
        trigger_params: json!({}),
        entity_state: json!({"Status":"Reasoning","fields":{"reasoning_session_id":"same-child","started_at_ms":started.to_string(),"phase":"compose","program_json":json!({"composition_correction":{"attempt":2,"validation_error":"prior invalid route"}}).to_string()},"counters":{"reasoning_phase_polls":10,"transition_count":350}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([(
            "temper_api_url".into(),
            "https://temper.test".into(),
        )]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let mut completion_count = 0;
    for (poll, transitions, start, status, expected) in [
        (10, 350, started, "CallingProvider", "ReasoningPending"),
        (11, 352, started, "Completed", "ReasoningComplete"),
        (11, 352, now - 3_600_000, "Completed", "Fail"),
        (11, 480, started, "Completed", "Fail"),
    ] {
        let mut invocation = ctx.clone();
        invocation.entity_state["counters"]["reasoning_phase_polls"] = json!(poll);
        invocation.entity_state["counters"]["transition_count"] = json!(transitions);
        invocation.entity_state["fields"]["started_at_ms"] = json!(start.to_string());
        let host=SimWasmHost::new().with_default_response(500,"unexpected operation")
            .with_response("https://temper.test/tdata/Sessions('same-child')?$select=Status,result,error_message,error,turn_count,provider_auth_status",200,&json!({"Status":status,"result":"unchanged completed answer","turn_count":1}).to_string());
        let result = engine
            .invoke(
                &hash,
                &invocation,
                Arc::new(host),
                &WasmResourceLimits::default(),
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap();
        assert!(result.success, "{result:?}");
        assert_eq!(result.callback_action, expected, "{result:?}");
        assert!(result.callback_params.get("started_at_ms").is_none());
        assert!(result.callback_params.get("reasoning_session_id").is_none());
        assert!(
            result
                .callback_params
                .get("reasoning_phase_polls")
                .is_none()
        );
        if expected == "ReasoningComplete" {
            completion_count += 1;
            assert_eq!(
                result.callback_params,
                json!({"reasoning_result":"unchanged completed answer"})
            );
        } else if expected == "ReasoningPending" {
            assert_eq!(result.callback_params, json!({}));
        } else {
            let error = result.callback_params["error_message"].as_str().unwrap();
            assert!(error.contains(if transitions == 480 {
                "transition budget"
            } else {
                "time budget"
            }));
        }
    }
    assert_eq!(completion_count, 1);
    // Completion leaves Reasoning; native action guards prevent subsequent polls consuming it again.
    let spec =
        std::fs::read_to_string(root.join("os-apps/paw-foresight/specs/semantic_run.ioa.toml"))
            .unwrap();
    assert!(spec.contains(
        "name = \"ReasoningComplete\"\nkind = \"input\"\nfrom = [\"Reasoning\"]\nto = \"Expanding\""
    ));
    assert!(spec.contains("state = \"Reasoning\"\nafter_seconds = 900\non_timeout = \"Fail\""));
}
