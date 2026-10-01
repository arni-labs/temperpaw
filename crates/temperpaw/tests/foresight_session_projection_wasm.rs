//! A large reasoning input must not make polling its small result fail.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};
#[tokio::test]
async fn polling_projects_result_without_echoing_three_megabyte_prompt() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output=std::process::Command::new("bash").current_dir(&root).args(["-c","source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-foresight/wasm/semantic_session wasm32-unknown-unknown --locked"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact = std::env::var("ARN518_SESSION_WASM_OVERRIDE")
        .unwrap_or_else(|_| String::from_utf8(output.stdout).unwrap().trim().into());
    let engine = WasmEngine::new().unwrap();
    let hash = engine
        .compile_and_cache(&std::fs::read(artifact).unwrap())
        .unwrap();
    let source = json!({"Status":"Completed","result":"{\"hypotheses\":[]}","error_message":"","user_message":"x".repeat(3*1024*1024),"conversation":"private-unneeded-conversation"});
    let selected = json!({"Status":source["Status"],"result":source["result"],"error_message":source["error_message"]});
    let host = SimWasmHost::new()
        .with_default_response(500, "unexpected route")
        .with_response(
            "http://fixture/tdata/Sessions('child')",
            200,
            &source.to_string(),
        )
        .with_response(
            "http://fixture/tdata/Sessions('child')?$select=Status,result,error_message,error",
            200,
            &selected.to_string(),
        );
    let host = host.with_response(
        "http://fixture/tdata/Sessions('child')?$select=Status,result,error_message",
        200,
        &selected.to_string(),
    );
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "run".into(),
        trigger_action: "CheckReasoning".into(),
        wasm_module: Some("semantic_session".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":{"reasoning_session_id":"child","started_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string()}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("temper_api_url".into(), "http://fixture".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let result: Value = serde_json::to_value(
        engine
            .invoke(
                &hash,
                &ctx,
                Arc::new(host),
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
    .unwrap();
    assert_eq!(result["callback_action"], "ReasoningComplete", "{result}");
    assert_eq!(
        result["callback_params"]["reasoning_result"],
        source["result"]
    );
    let failure = json!({"Status":"Failed","result":"","error_message":"","error":"WASM module steering_checker not found"});
    let host = SimWasmHost::new()
        .with_default_response(500, "unexpected route")
        .with_response(
            "http://fixture/tdata/Sessions('child')?$select=Status,result,error_message,error",
            200,
            &failure.to_string(),
        )
        .with_response(
            "http://fixture/tdata/Sessions('child')?$select=Status,result,error_message",
            200,
            &json!({"Status":"Failed","result":"","error_message":""}).to_string(),
        );
    let failed: Value = serde_json::to_value(
        engine
            .invoke(
                &hash,
                &ctx,
                Arc::new(host),
                &WasmResourceLimits::default(),
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(failed["callback_action"], "Fail");
    assert!(
        failed["callback_params"]["error_message"]
            .as_str()
            .unwrap()
            .contains("WASM module steering_checker not found"),
        "{failed}"
    );
    let pending_host = SimWasmHost::new().with_response(
        "http://fixture/tdata/Sessions('child')?$select=Status,result,error_message,error",
        200,
        &json!({"Status":"Running","result":"","error_message":"","error":""}).to_string(),
    );
    let mut later = ctx.clone();
    later.entity_state["counters"] = json!({"check_count":360});
    let pending: Value = serde_json::to_value(
        engine
            .invoke(
                &hash,
                &later,
                Arc::new(pending_host),
                &WasmResourceLimits::default(),
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        pending["callback_action"], "ReasoningPending",
        "cumulative polling from earlier children must not prematurely fail a new child: {pending}"
    );
    for (status, error, count, expected) in [
        (
            "Failed",
            "OpenAI Codex API returned 500: native turn auth context mismatch: scopes",
            0,
            "ReasoningRetry",
        ),
        (
            "Failed",
            "OpenAI Codex API returned 500: native turn auth context mismatch: scopes",
            3,
            "Fail",
        ),
        ("Failed", "HTTP 500: authentication failed", 0, "Fail"),
        ("Failed", "HTTP 500: authentication required", 0, "Fail"),
        ("Failed", "HTTP 500: unauthenticated", 0, "Fail"),
        ("Failed", "HTTP 500: missing refresh token", 0, "Fail"),
        ("Failed", "HTTP 500: expired refresh token", 0, "Fail"),
        ("Failed", "HTTP 500: invalid_grant", 0, "Fail"),
        ("Failed", "HTTP 500: insufficient_scope", 0, "Fail"),
        ("Failed", "HTTP 500: upstream HTTP 401", 0, "Fail"),
        ("Failed", "HTTP 500: upstream API returned 403", 0, "Fail"),
        ("Failed", "HTTP 500: permission denied", 0, "Fail"),
        ("Failed", "HTTP 500: billing limit", 0, "Fail"),
        ("Failed", "HTTP 500: validation failed", 0, "Fail"),
        (
            "Failed",
            "OpenAI Codex API returned 503: upstream connect error or disconnect/reset before headers. connection timeout",
            0,
            "ReasoningRetry",
        ),
        (
            "Failed",
            "OpenAI Codex API returned 429: rate limited",
            2,
            "ReasoningRetry",
        ),
        (
            "Failed",
            "OpenAI Codex API returned 503: upstream connection timeout",
            3,
            "Fail",
        ),
        ("Cancelled", "OpenAI Codex API returned 503", 0, "Fail"),
        (
            "Failed",
            "OpenAI Codex API returned 403: permission denied",
            0,
            "Fail",
        ),
        ("Failed", "Invalid generated reference", 0, "Fail"),
    ] {
        let host = SimWasmHost::new().with_response(
            "http://fixture/tdata/Sessions('child')?$select=Status,result,error_message,error",
            200,
            &json!({"Status":status,"result":"","error_message":error,"error":""}).to_string(),
        );
        let mut context = ctx.clone();
        context.entity_state["counters"] = json!({"reasoning_retry_count":count});
        let result: Value = serde_json::to_value(
            engine
                .invoke(
                    &hash,
                    &context,
                    Arc::new(host),
                    &WasmResourceLimits::default(),
                    Arc::new(RwLock::new(StreamRegistry::default())),
                )
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["callback_action"], expected, "{result}");
        if expected == "ReasoningRetry" {
            assert_eq!(result["callback_params"]["last_retry_error"], error);
            assert_eq!(result["callback_params"]["last_retry_session_id"], "child");
            assert_eq!(
                result["callback_params"].as_object().unwrap().len(),
                2,
                "Retry must not reset clock, counters or retained work"
            );
        }
    }
    // The actual artifact must refuse even a completed child after the original deadline.
    let mut expired = ctx.clone();
    expired.entity_state["fields"]["started_at_ms"] = json!("1");
    let deadline = engine
        .invoke(
            &hash,
            &expired,
            Arc::new(
                SimWasmHost::new()
                    .with_default_response(200, r#"{"Status":"Completed","result":"{}"}"#),
            ),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    assert_eq!(deadline.callback_action, "Fail");
    assert!(
        deadline.callback_params["error_message"]
            .as_str()
            .unwrap()
            .contains("time budget")
    );
    for polls in [10, 11] {
        let mut context = ctx.clone();
        context.entity_state["counters"] = json!({"reasoning_phase_polls":polls});
        let host = SimWasmHost::new()
            .with_default_response(200, r#"{"Status":"Completed","result":"{}"}"#);
        let result = engine
            .invoke(
                &hash,
                &context,
                Arc::new(host),
                &WasmResourceLimits::default(),
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap();
        assert_eq!(
            result.callback_action,
            if polls == 10 {
                "ReasoningComplete"
            } else {
                "Fail"
            }
        );
    }
    // Simulation of measured Configure→RecordResult durations from the failed live
    // three-attempt compose phase; this is not a new live provider run.
    let spec: toml::Value = toml::from_str(include_str!(
        "../../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"
    ))
    .unwrap();
    let delay = spec["action"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"].as_str() == Some("SpawnReasoning"))
        .unwrap()["effect"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e.get("action").and_then(toml::Value::as_str) == Some("CheckReasoning"))
        .unwrap()["delay_seconds"]
        .as_integer()
        .unwrap() as u64
        * 1000;
    for (interval, expected_polls, expected_completed) in [(30_000, 10, false), (delay, 7, true)] {
        let mut polls = 0;
        let mut completed = 0;
        for duration in [137_469u64, 112_199, 109_172] {
            let mut elapsed = 0;
            loop {
                elapsed += interval;
                polls += 1;
                let mut context = ctx.clone();
                context.entity_state["counters"] = json!({"reasoning_phase_polls":polls});
                let status = if elapsed >= duration {
                    "Completed"
                } else {
                    "CallingProvider"
                };
                let reply = json!({"Status":status,"result":"{}"});
                let response = engine
                    .invoke(
                        &hash,
                        &context,
                        Arc::new(SimWasmHost::new().with_default_response(200, &reply.to_string())),
                        &WasmResourceLimits::default(),
                        Arc::new(RwLock::new(StreamRegistry::default())),
                    )
                    .await
                    .unwrap();
                if response.callback_action == "ReasoningComplete" {
                    completed += 1;
                    break;
                }
                if response.callback_action == "Fail" {
                    break;
                }
                assert_eq!(response.callback_action, "ReasoningPending");
            }
            if polls >= 10 {
                break;
            }
        }
        assert_eq!(polls, expected_polls);
        assert_eq!(completed == 3, expected_completed);
    }
}
