//! Replay generated acceptance checkpoints; no provider calls or native mutations.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};

#[tokio::test]
#[ignore = "requires captured checkpoint fixture and built semantic_prepare WASM"]
async fn completed_child_recovery_uses_saved_answer_and_original_checkpoint() {
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("FORESIGHT_RECOVERY_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let mut source = fixture["source"]["fields"].clone();
    source["Id"] = fixture["source"]["Id"].clone();
    source["Status"] = fixture["source"]["Status"].clone();
    for (key, value) in fixture["source"]["counters"].as_object().unwrap() {
        source[key] = value.clone();
    }
    let child = fixture["child"].clone();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let wasm = std::env::var("FORESIGHT_RECOVERY_WASM").map(std::path::PathBuf::from).unwrap_or_else(|_|root.join("os-apps/paw-foresight/wasm/semantic_prepare/target/wasm32-unknown-unknown/release/semantic_prepare.wasm"));
    let engine = WasmEngine::new().unwrap();
    let hash = engine
        .compile_and_cache(&std::fs::read(wasm).unwrap())
        .unwrap();
    let ctx = WasmInvocationContext {
        tenant: "fixture".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "recovery-test".into(),
        trigger_action: "Start".into(),
        wasm_module: Some("semantic_prepare".into()),
        trigger_params: json!({}),
        entity_state: json!({"Status":"Preparing","fields":{"world_id":source["world_id"],"resume_run_id":source["Id"]}}),
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
    for case in [
        "completed",
        "active",
        "consumed",
        "mismatched",
        "transitions",
        "expired",
    ] {
        let mut source = source.clone();
        let mut child = child.clone();
        match case {
            "active" => child["Status"] = json!("CallingProvider"),
            "consumed" => source["reasoning_result"] = child["result"].clone(),
            "mismatched" => child["user_message"] = json!("different phase input"),
            "transitions" => source["transition_count"] = json!(480),
            "expired" => source["started_at_ms"] = json!("1000"),
            _ => (),
        }
        let source_url = format!(
            "https://temper.test/tdata/SemanticRuns('{}')?$select=Id,Status,world_id,agent_id,model,provider,provider_options_json,snapshot_json,program_json,trace_json,started_at_ms,phase,reasoning_session_id,reasoning_result,system_prompt,user_message,error_message,transition_count,reasoning_phase_polls,reasoning_retry_count",
            source["Id"].as_str().unwrap()
        );
        let child_url = format!(
            "https://temper.test/tdata/Sessions('{}')?$select=Id,Status,result,system_prompt,user_message",
            source["reasoning_session_id"].as_str().unwrap()
        );
        let old_source_url = source_url.split(",reasoning_session_id,").next().unwrap();
        let host = SimWasmHost::new()
            .with_default_response(500, "unexpected request; no provider calls allowed")
            .with_response(&source_url, 200, &source.to_string())
            .with_response(old_source_url, 200, &source.to_string())
            .with_response(&child_url, 200, &child.to_string());
        let result = engine
            .invoke(
                &hash,
                &ctx,
                Arc::new(host),
                &WasmResourceLimits {
                    max_memory: 268435456,
                    max_fuel: 10_000_000_000,
                    ..Default::default()
                },
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap();
        assert!(result.success, "{case}: {result:?}");
        if case == "completed" {
            assert_eq!(result.callback_action, "ResumeReasoned", "{result:?}");
            for key in [
                "snapshot_json",
                "program_json",
                "trace_json",
                "started_at_ms",
                "phase",
                "model",
                "provider",
                "provider_options_json",
                "transition_count",
                "reasoning_phase_polls",
                "reasoning_retry_count",
                "reasoning_session_id",
            ] {
                assert_eq!(result.callback_params[key], source[key], "{key}");
            }
            assert_eq!(result.callback_params["reasoning_result"], child["result"]);
        } else if case == "consumed" {
            assert!(
                matches!(
                    result.callback_action.as_str(),
                    "Prepared" | "ResumePrepared"
                ),
                "{result:?}"
            );
        } else {
            assert_eq!(result.callback_action, "Fail", "{case}: {result:?}");
        }
    }
}
