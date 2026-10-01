use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};
#[tokio::test]
async fn bounded_recent_events_and_module_memory_handle_a_full_trace() {
    let engine = WasmEngine::new().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new("bash").current_dir(&root).args(["-c", "set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-foresight/wasm/semantic_call wasm32-unknown-unknown --locked"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hash = engine
        .compile_and_cache(
            &std::fs::read(String::from_utf8(output.stdout).unwrap().trim()).unwrap(),
        )
        .unwrap();
    let trace = serde_json::to_string(&vec![
        json!({"request":{"text":"x".repeat(8000)},"response":{"text":"y".repeat(4000)}});
        320
    ])
    .unwrap();
    for (tail, mb) in [(50, 64), (5, 256)] {
        let ctx = WasmInvocationContext {
            tenant: "test".into(),
            entity_type: "SemanticRun".into(),
            entity_id: "stress".into(),
            trigger_action: "Evaluate".into(),
            wasm_module: Some("semantic_call".into()),
            trigger_params: json!({}),
            entity_state: json!({"fields":{"program_json":"{\"cursor\":320}","trace_json":trace,"request_json":"{}"},"events":vec![json!({"params":{"trace_json":trace}});tail]}),
            agent_id: None,
            session_id: None,
            integration_config: BTreeMap::new(),
            trace_id: String::new(),
            workflow_root_entity_type: None,
            workflow_root_entity_id: None,
            workflow_run_id: None,
            http_request: None,
        };
        let limits = WasmResourceLimits {
            max_memory: mb * 1024 * 1024,
            ..Default::default()
        };
        let r = engine
            .invoke(
                &hash,
                &ctx,
                Arc::new(SimWasmHost::new()),
                &limits,
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await;
        match r {
            Ok(v) => {
                let v = serde_json::to_value(v).unwrap();
                println!("tail={tail} mb={mb}: {v}");
                assert_eq!(
                    v["callback_params"]["error_message"],
                    "Provider call budget exhausted"
                );
            }
            Err(e) => {
                println!(
                    "tail={tail} mb={mb}: {}",
                    e.to_string().chars().take(100).collect::<String>()
                );
                assert_eq!(tail, 50);
            }
        }
    }
}
