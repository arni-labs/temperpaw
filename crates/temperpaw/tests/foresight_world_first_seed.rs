//! The world-first lifecycle needs sourced present evidence, never seed predictions.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};

async fn invoke(engine: &WasmEngine, module: &str, fields: &Value, host: SimWasmHost) -> Value {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = root.join("os-apps/paw-foresight/wasm").join(module);
    let build = std::process::Command::new("cargo")
        .args([
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "--locked",
            "--manifest-path",
        ])
        .arg(directory.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let hash = engine
        .compile_and_cache(
            &std::fs::read(directory.join(format!(
                "target/wasm32-unknown-unknown/release/{module}.wasm"
            )))
            .unwrap(),
        )
        .unwrap();
    let context = WasmInvocationContext {
        tenant: "fixture".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "evidence-only".into(),
        trigger_action: "Next".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":0},"fields":fields}),
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
    serde_json::to_value(
        engine
            .invoke(
                &hash,
                &context,
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
    .unwrap()
}

#[tokio::test]
async fn evidence_only_prepare_establishes_baseline_and_starts_imagination() {
    let engine = WasmEngine::new().unwrap();
    let question = "How might people make games in 2030?";
    let source = json!({"Id":"evidence-1","world_id":"world-1","statement":"A documented present practice in 2026.","provenance":"observed","source_refs":"[\"https://example.test/report\"]","evidence_json":json!({"kind":"finding","publication_date":"2026-09-30","observation_period":{"start":"2026-09","end":"2026-09"},"retrieved_at":"2026-10-02"}).to_string()});
    let host = SimWasmHost::new().with_default_response(500,"Unexpected network request")
        .with_response("https://temper.test/tdata/Worlds('world-1')",200,&json!({"Id":"world-1","Status":"Active","description":question,"last_ingest_date":"2026-10-02","target_date":"2030-01-01","research_session_id":"research-1","agent_model":"configured-model","agent_provider":"configured-provider","agent_provider_options_json":r#"{ "reasoning_effort": "high", "custom": {"value":7} }"#}).to_string())
        .with_response("https://temper.test/tdata/EventNodes?$filter=world_id%20eq%20'world-1'&$top=513",200,&json!({"value":[source]}).to_string())
        .with_response("https://temper.test/tdata/Sessions('research-1')",200,&json!({"agent_id":"agent-1"}).to_string());
    let prepared = invoke(
        &engine,
        "semantic_prepare",
        &json!({"world_id":"world-1"}),
        host,
    )
    .await;
    assert_eq!(prepared["callback_action"], "Prepared", "{prepared}");
    let mut fields = prepared["callback_params"].clone();
    assert_eq!(fields["model"], "configured-model");
    assert_eq!(fields["provider"], "configured-provider");
    assert_eq!(
        fields["provider_options_json"],
        r#"{ "reasoning_effort": "high", "custom": {"value":7} }"#
    );
    let snapshot: Value = serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    assert_eq!(snapshot["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["nodes"][0]["kind"], "evidence");
    let program: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    assert!(
        program["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["function"] == "classify_gap")
    );
    fields["reasoning_result"] = json!({"baseline":{"as_of":"2026-10-02","observed":[{"claim":"A documented present practice in 2026.","evidence_ids":["ref_0001"]}],"assumptions":[],"unknowns":["Future adoption remains unknown."]},"scope_review":{"requested_question":question,"evidence_scope":"A present practice, without claims about future adoption.","status":"aligned","narrowing_basis":"none","limitations":[]}}).to_string().into();
    let baseline = invoke(
        &engine,
        "semantic_expand",
        &fields,
        SimWasmHost::new().with_default_response(500, "Unexpected network request"),
    )
    .await;
    assert_eq!(baseline["callback_action"], "Expanded", "{baseline}");
    for (key, value) in baseline["callback_params"].as_object().unwrap() {
        fields[key] = value.clone();
    }
    let program: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(program["baseline_status"], "established");
    let step = invoke(
        &engine,
        "semantic_step",
        &fields,
        SimWasmHost::new().with_default_response(500, "Unexpected network request"),
    )
    .await;
    assert_eq!(step["callback_action"], "Reason", "{step}");
    assert_eq!(step["callback_params"]["phase"], "imagine");
    assert_eq!(
        fields["snapshot_json"],
        prepared["callback_params"]["snapshot_json"]
    );
    assert_eq!(
        fields["started_at_ms"],
        prepared["callback_params"]["started_at_ms"]
    );
}

#[tokio::test]
async fn native_retry_preserves_provider_options_and_original_clock() {
    let engine = WasmEngine::new().unwrap();
    let started = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        - 1000)
        .to_string();
    for options in [
        None,
        Some(""),
        Some(r#"{ "reasoning_effort": "high", "custom": {"value":7} }"#),
    ] {
        let mut checkpoint = json!({"Id":"prior-run","Status":"Failed","world_id":"world-1","agent_id":"agent-1","model":"custom-model","provider":"custom-provider","phase":"explore","started_at_ms":started,"trace_json":"[]",
            "snapshot_json":json!({"world":{"Id":"world-1"},"nodes":[{"Id":"h","kind":"scenario","statement":"Future","edges":"[]"}]}).to_string(),
            "program_json":json!({"schema":"foresight-open-semantic-v2","tasks":[{"nodeId":"h","function":"estimate_likelihood"}],"cursor":0,"results":{"h":{"estimate_likelihood":"0.37"}},"evaluations":{},"rounds":[],"round":2}).to_string()});
        if let Some(options) = options {
            checkpoint["provider_options_json"] = json!(options);
        }
        let host = SimWasmHost::new().with_default_response(500,"Unexpected request")
            .with_response("https://temper.test/tdata/SemanticRuns('prior-run')?$select=Id,Status,world_id,agent_id,model,provider,provider_options_json,snapshot_json,program_json,trace_json,started_at_ms,phase",200,&checkpoint.to_string());
        let result = invoke(
            &engine,
            "semantic_prepare",
            &json!({"world_id":"world-1","resume_run_id":"prior-run"}),
            host,
        )
        .await;
        assert_eq!(result["callback_action"], "ResumePrepared", "{result}");
        let fields = &result["callback_params"];
        assert_eq!(fields["provider_options_json"], options.unwrap_or(""));
        assert_eq!(fields["model"], "custom-model");
        assert_eq!(fields["provider"], "custom-provider");
        assert_eq!(fields["started_at_ms"], started);
    }
}
