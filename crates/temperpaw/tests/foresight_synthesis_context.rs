//! Replay a failed synthesis through the real reasoning WASM producer.
use serde_json::{json, Value};
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
#[ignore = "requires an authorized captured failed synthesis run"]
async fn captured_synthesis_uses_writer_projection_without_raw_search_history() {
    let capture: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("FORESIGHT_SYNTHESIS_CAPTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let result = invoke(
        &WasmEngine::new().unwrap(),
        "semantic_reasoning",
        &capture["fields"],
        SimWasmHost::new().with_default_response(500, "Synthesis must not make an HTTP request"),
    )
    .await;
    assert_eq!(result["callback_action"], "LaunchReasoning", "{result}");
    let message = result["callback_params"]["user_message"].as_str().unwrap();
    let input: Value = serde_json::from_str(message).unwrap();
    assert!(input.get("endpoint_search").is_none());
    assert!(input.get("proposal_pool").is_none());
    assert!(input.get("endpoint_lineage").is_some());
    assert!(input.get("world_audits").is_some());
    assert!(input.get("source_evidence").is_some());
    assert!(
        message.len() < 300_000,
        "Captured food context regressed: {} bytes",
        message.len()
    );
    assert_eq!(result["callback_params"]["tools_enabled"], "");
    if let Ok(path) = std::env::var("FORESIGHT_SYNTHESIS_RECEIPT") {
        std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
}
