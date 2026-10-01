//! Resume large persisted checkpoints through the actual HTTP host ABI, without network calls.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};
fn fixture() -> Value {
    let snapshot = json!({"world":{"Id":"world"},"nodes":[{"Id":"h1","kind":"scenario","statement":"One future","edges":"[]"},{"Id":"h2","kind":"scenario","statement":"Another future","edges":"[]"}]});
    let trace:Vec<_>=(0..1228).map(|index|json!({"index":index,"nodeId":"h1","function":"estimate_likelihood","request":{"model":"jev-1.13.0","state_ref":{"quoted_evidence":"A dated source quotation. ".repeat(250)}},"response":{"model":"jev-1.13.0","answers":{"result":{"type":"noul","noul":0.37}}}})).collect();
    let program = json!({"schema":"foresight-open-semantic-v2","stage":"combinations","tasks":[],"cursor":0,"results":{"h1":{"estimate_likelihood":"0.37"}},"evaluations":{},"rounds":[],"round":2});
    json!({"Id":"old-run","Status":"Failed","world_id":"world","agent_id":"agent","model":"gpt-5.5","provider":"openai_codex","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":json!(trace).to_string(),"started_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string(),"phase":"compose"})
}
fn bytes() -> Vec<u8> {
    if let Ok(path) = std::env::var("ARN518_RESUME_WASM_OVERRIDE") {
        return std::fs::read(path).unwrap();
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let build=std::process::Command::new("bash").current_dir(root).args(["-c","source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-foresight/wasm/semantic_prepare wasm32-unknown-unknown --locked"]).output().unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    std::fs::read(String::from_utf8(build.stdout).unwrap().trim()).unwrap()
}
async fn check(record: Value) {
    let partial = record.get("fields").unwrap_or(&record)["Status"] == "Completed";
    let fields = record.get("fields").unwrap_or(&record);
    let keys = [
        "Id",
        "Status",
        "world_id",
        "agent_id",
        "model",
        "provider",
        "snapshot_json",
        "program_json",
        "trace_json",
        "started_at_ms",
        "phase",
    ];
    let selected: serde_json::Map<String, Value> = keys
        .iter()
        .map(|k| ((*k).to_owned(), fields[*k].clone()))
        .collect();
    let selected = Value::Object(selected);
    let body = selected.to_string();
    assert!(body.len() > 4 * 1024 * 1024);
    let url = format!(
        "http://fixture/tdata/SemanticRuns('{}')?$select=Id,Status,world_id,agent_id,model,provider,snapshot_json,program_json,trace_json,started_at_ms,phase",
        selected["Id"].as_str().unwrap()
    );
    let host = SimWasmHost::new()
        .with_default_response(500, "Unexpected route")
        .with_response(&url, 200, &body);
    let engine = WasmEngine::new().unwrap();
    let hash = engine.compile_and_cache(&bytes()).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "new-run".into(),
        trigger_action: "Start".into(),
        wasm_module: Some("semantic_prepare".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":{"world_id":selected["world_id"],"resume_run_id":selected["Id"]}}),
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
            Arc::new(host),
            &WasmResourceLimits {
                max_memory: 256 * 1024 * 1024,
                max_fuel: 10_000_000_000,
                ..Default::default()
            },
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    assert_eq!(
        result.callback_action,
        if partial {
            "ResumePrepared"
        } else {
            "Prepared"
        },
        "{}",
        result.callback_params["error_message"]
    );
    for key in [
        "snapshot_json",
        "trace_json",
        "started_at_ms",
        "world_id",
        "agent_id",
        "model",
        "provider",
    ] {
        assert_eq!(
            result.callback_params[key], selected[key],
            "{key} must survive resume exactly"
        );
    }
    let before: Value = serde_json::from_str(selected["program_json"].as_str().unwrap()).unwrap();
    let mut after: Value =
        serde_json::from_str(result.callback_params["program_json"].as_str().unwrap()).unwrap();
    if after["stop_reason"] == "time_budget"
        || (partial
            && before["stop_reason"] == "provider_error"
            && after["stop_reason"] == "resumed_after_provider_error")
    {
        if let Some(reason) = before.get("stop_reason") {
            after["stop_reason"] = reason.clone();
        } else {
            after.as_object_mut().unwrap().remove("stop_reason");
        }
    }
    assert_eq!(
        after, before,
        "Only budget exhaustion or explicit interrupted-provider resumption may update stop_reason"
    );
    let trace: Value =
        serde_json::from_str(result.callback_params["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        trace.as_array().unwrap().len(),
        if partial { 1229 } else { 1228 }
    );
    println!(
        "checkpoint HTTP bytes={}; preserved checks={}",
        body.len(),
        trace.as_array().unwrap().len()
    );
}
#[tokio::test]
async fn checkpoint_larger_than_sdk_buffer_preserves_every_check() {
    check(fixture()).await;
}
#[tokio::test]
#[ignore = "Optional local saved checkpoint; never commit private run data"]
async fn real_saved_checkpoint_roundtrips() {
    let path = std::env::var("ARN518_RESUME_RECORD").expect("saved record path");
    check(serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()).await;
}

#[tokio::test]
async fn partial_completed_checkpoint_preserves_pending_checks_and_history() {
    let mut record = fixture();
    record["Status"] = json!("Completed");
    record["phase"] = json!("synthesize");
    let mut program: Value =
        serde_json::from_str(record["program_json"].as_str().unwrap()).unwrap();
    program["stage"] = json!("worlds");
    program["tasks"] = json!([{"nodeId":"h1","function":"estimate_likelihood"}]);
    program["stop_reason"] = json!("provider_error");
    program["world_refinement"] =
        json!({"world-a":{"rounds":[{"round":1,"complete":false,"probability":null}]}});
    record["program_json"] = json!(program.to_string());
    let mut trace: Value = serde_json::from_str(record["trace_json"].as_str().unwrap()).unwrap();
    let mut last = trace[1227].clone();
    last["index"] = json!(1228);
    trace.as_array_mut().unwrap().push(last);
    record["trace_json"] = json!(trace.to_string());
    check(record).await;
}
