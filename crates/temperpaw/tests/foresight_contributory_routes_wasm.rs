//! Actual expansion preserves two captured contributory routes to one consequence.
//! Replay uses the later immutable catalog and assessments, with only correction cursors cleared locally.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
fn artifact(engine: &WasmEngine, module: &str) -> String {
    let path=std::env::var(format!("ARN518_BRANCH_{}",module.to_uppercase())).unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm",env!("CARGO_MANIFEST_DIR")));
    engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap()
}
async fn run(engine: &WasmEngine, module: &str, fields: Value, host: Arc<dyn WasmHost>) -> Value {
    let context = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "fixture".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":fields["transition_count"].as_u64().unwrap_or(0)},"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([
            ("temper_api_url".into(), "http://fixture".into()),
            ("typesafe_api_key".into(), "fixture-only".into()),
        ]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    serde_json::to_value(
        engine
            .invoke(
                &artifact(engine, module),
                &context,
                host,
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
#[ignore = "Requires locally captured native learning checkpoint and draft"]
async fn captured_contributory_routes_are_both_preserved() {
    let base = std::path::Path::new("/private/tmp/arn518-sept29");
    let checkpoint: Value = serde_json::from_slice(
        &std::fs::read(base.join("learning-scope-checkpoint.json")).unwrap(),
    )
    .unwrap();
    let sessions: Value = serde_json::from_slice(
        &std::fs::read(base.join("learning-scope-generation-sessions.json")).unwrap(),
    )
    .unwrap();
    let source = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "01a0f4e1-4e2c-7562-8da3-b790f21d7fb0")
        .unwrap();
    let mut program = checkpoint["program"].clone();
    program["composition_correction"] = Value::Null;
    program["response_correction"] = Value::Null;
    let fields = json!({"phase":"compose","snapshot_json":checkpoint["snapshot"].to_string(),"program_json":program.to_string(),"reasoning_result":source["fields"]["result"],"started_at_ms":checkpoint["started_at_ms"]});
    let engine = WasmEngine::new().unwrap();
    let result = run(
        &engine,
        "semantic_expand",
        fields,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(
        result["callback_action"], "Expanded",
        "captured valid routes must both survive composition"
    );
    let snapshot: Value =
        serde_json::from_str(result["callback_params"]["snapshot_json"].as_str().unwrap()).unwrap();
    let world = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["kind"] == "world"
                && n["archived"] != true
                && n["chain"]
                    .as_array()
                    .is_some_and(|ls| ls.iter().any(|l| l["id"] == "projects_to_badges"))
        })
        .unwrap();
    let links = world["chain"].as_array().unwrap();
    let project = links
        .iter()
        .find(|l| l["id"] == "projects_to_badges")
        .unwrap();
    let work = links.iter().find(|l| l["id"] == "work_to_badges").unwrap();
    assert_eq!(project["to_id"], work["to_id"]);
    assert_ne!(project["from_ids"], work["from_ids"]);
    assert_eq!(project["by"], "2030-12-31");
    assert_eq!(work["by"], "2030-12-31");
    let next: Value =
        serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    for id in ["projects_to_badges", "work_to_badges"] {
        for function in ["check_transition", "conditional_on", "conditional_off"] {
            assert!(
                next["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["world_id"] == world["Id"]
                        && t["link_id"] == id
                        && t["function"] == function)
            );
        }
    }
    assert_eq!(
        result["callback_params"]["started_at_ms"],
        checkpoint["started_at_ms"]
    );
    std::fs::write(
        base.join("contributory-routes-actual-expanded.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
