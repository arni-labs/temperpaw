//! Structural world-set inputs retain definitions, not operational or presentation state.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
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

struct Capture(std::sync::Mutex<Vec<Value>>);
#[async_trait::async_trait]
impl WasmHost for Capture {
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("unexpected binary".into())
    }
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Ok("fixture-only".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_call(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        let request: Value = serde_json::from_str(body).unwrap();
        let answers:serde_json::Map<String,Value>=request["questions"].as_object().unwrap().keys().map(|k|(k.clone(),json!({"type":"choice","choice":"uncertain","probabilities":{"alternative_answers":0.0,"complementary_slices":0.0,"uncertain":1.0}}))).collect();
        self.0.lock().unwrap().push(request);
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
}
#[tokio::test]
#[ignore = "Requires captured holidays checkpoint; mocked provider is not a live token-limit claim"]
async fn captured_set_preserves_definitions_conditions_and_qualifications_without_bookkeeping() {
    let checkpoint: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("WORLD_SET_CHECKPOINT").unwrap()).unwrap(),
    )
    .unwrap();
    let engine = WasmEngine::new().unwrap();
    let mut outputs = vec![];
    for mutate_irrelevant in [false, true] {
        let mut snapshot = checkpoint["snapshot"].clone();
        if mutate_irrelevant {
            for node in snapshot["nodes"].as_array_mut().unwrap() {
                for k in [
                    "Status",
                    "source_session_id",
                    "research_status",
                    "before_gap",
                    "scene",
                    "signals",
                    "falsifiers",
                    "what_you_can_do",
                    "evaluations",
                ] {
                    node[k] = json!("irrelevant bookkeeping or display change".repeat(100));
                }
            }
        }
        let fields = json!({"snapshot_json":snapshot.to_string(),"program_json":checkpoint["program"].to_string(),"trace_json":"[]","started_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string()});
        let host = Arc::new(Capture(std::sync::Mutex::new(vec![])));
        let result = run(&engine, "semantic_call", fields, host.clone()).await;
        assert_eq!(result["callback_action"], "Recorded", "{result}");
        let request = {
            let requests = host.0.lock().unwrap();
            assert_eq!(requests.len(), 1);
            requests[0].clone()
        };
        assert!(
            request.to_string().len() < 75_000,
            "structural projection exceeded captured regression ceiling: {}",
            request.to_string().len()
        );
        let mut state = request["state"]["cases"]["q0"].clone();
        for (k, v) in request["state"]["common"].as_object().unwrap() {
            state[k] = v.clone();
        }
        let ids = |records: &Value| -> std::collections::BTreeSet<String> {
            records
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["Id"].as_str().unwrap().to_owned())
                .collect()
        };
        let expected_worlds: std::collections::BTreeSet<String> =
            checkpoint["program"]["active_world_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
        assert_eq!(ids(&state["proposed_worlds"]), expected_worlds);
        let expected_components: std::collections::BTreeSet<String> =
            checkpoint["snapshot"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|n| expected_worlds.contains(n["Id"].as_str().unwrap()))
                .flat_map(|w| {
                    ["component_ids", "counter_ids"]
                        .into_iter()
                        .flat_map(move |key| {
                            w[key]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|v| v.as_str().unwrap().to_owned())
                        })
                })
                .collect();
        assert_eq!(ids(&state["components"]), expected_components);
        let expected_evidence: std::collections::BTreeSet<String> = checkpoint["snapshot"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| matches!(n["kind"].as_str(), Some("evidence" | "research_evidence")))
            .map(|n| n["Id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids(&state["source_evidence"]), expected_evidence);
        for (group, fields) in [
            (
                "proposed_worlds",
                vec![
                    "Id",
                    "statement",
                    "mechanism",
                    "narrative",
                    "shared_question",
                    "trajectory_answer",
                    "assumptions",
                    "facets",
                    "chain",
                    "branch_conditions",
                    "component_ids",
                    "counter_ids",
                ],
            ),
            (
                "components",
                vec![
                    "Id",
                    "statement",
                    "mechanism",
                    "scope",
                    "resolve_by",
                    "date",
                    "by",
                    "evidence_note",
                    "source_refs",
                ],
            ),
            (
                "source_evidence",
                vec![
                    "Id",
                    "statement",
                    "evidence_note",
                    "evidence_metadata",
                    "provenance",
                    "claim_type",
                    "resolution",
                ],
            ),
        ] {
            for record in state[group].as_array().unwrap() {
                let original = checkpoint["snapshot"]["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|n| n["Id"] == record["Id"])
                    .unwrap();
                for field in &fields {
                    assert_eq!(record[*field], original[*field], "{group} {field}");
                }
                assert!(record.get("source_session_id").is_none());
                assert!(record.get("Status").is_none());
            }
        }
        assert_eq!(state["baseline"], checkpoint["program"]["baseline"]);
        assert_eq!(state["world_question"], checkpoint["snapshot"]["world"]);
        let p: Value =
            serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap())
                .unwrap();
        assert_eq!(p["cursor"], 1);
        if let Ok(path) = std::env::var("WORLD_SET_PROJECTED_OUTPUT") {
            std::fs::write(path, request.to_string()).unwrap();
        }
        outputs.push(request);
    }
    assert_eq!(
        outputs[0], outputs[1],
        "dropped fields must not influence this structural question"
    );
}
