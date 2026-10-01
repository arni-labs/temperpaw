//! Task-relevant contexts preserve feedback while independent waves remain batchable.
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
        let answers:serde_json::Map<String,Value>=request["questions"].as_object().unwrap().keys().map(|k|(k.clone(),json!({"type":"choice","choice":"none","probabilities":{"prerequisite":0.0,"timing":0.0,"evidence":0.0,"none":1.0,"uncertain":0.0}}))).collect();
        self.0.lock().unwrap().push(request);
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
}

fn expanded_case(request: &Value, key: &str) -> Value {
    let mut state = request["state"]["cases"][key].clone();
    for (k, v) in request["state"]["common"].as_object().unwrap() {
        state[k] = v.clone();
    }
    if let Some(refs) = state.as_object_mut().unwrap().remove("comparison_refs") {
        state["comparisons"] = json!(
            refs.as_array()
                .unwrap()
                .iter()
                .map(
                    |i| request["state"]["comparison_catalog"][i.as_u64().unwrap() as usize]
                        .clone()
                )
                .collect::<Vec<_>>()
        );
    }
    state
}
#[tokio::test]
#[ignore = "Requires reconstructed captured learning wave and prior deployed call artifact"]
async fn gap_wave_keeps_sources_prior_feedback_and_fits_more_independent_questions() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("TASK_CONTEXT_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let fields = json!({"snapshot_json":fixture["snapshot"].to_string(),"program_json":fixture["program"].to_string(),"trace_json":fixture["trace"].to_string(),"started_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string()});
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(Capture(std::sync::Mutex::new(vec![])));
    let result = run(&engine, "semantic_call", fields, host.clone()).await;
    assert_eq!(result["callback_action"], "Recorded", "{result}");
    let request = {
        let r = host.0.lock().unwrap();
        assert_eq!(r.len(), 1);
        r[0].clone()
    };
    assert!(
        request["questions"].as_object().unwrap().len() >= 8,
        "old generic context only fit two independent questions"
    );
    assert!(request.to_string().len() <= 58901);
    let p: Value =
        serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        p["cursor"].as_u64().unwrap(),
        request["questions"].as_object().unwrap().len() as u64
    );
    let trace: Value =
        serde_json::from_str(result["callback_params"]["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        &trace.as_array().unwrap()[..fixture["trace"].as_array().unwrap().len()],
        fixture["trace"].as_array().unwrap()
    );
    let original: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("TASK_CONTEXT_ORIGINAL_BATCH").unwrap()).unwrap(),
    )
    .unwrap();
    for key in original["questions"].as_object().unwrap().keys() {
        let mut before = expanded_case(&original, key);
        let after = expanded_case(&request, key);
        assert!(!before["comparisons"].as_array().unwrap().is_empty());
        before["comparisons"] = json!([]);
        assert_eq!(
            after, before,
            "every other field, source and typed feedback stays exact"
        );
    }
    if let Ok(path) = std::env::var("TASK_CONTEXT_OUTPUT") {
        std::fs::write(path, request.to_string()).unwrap();
    }
    // Same snapshot and fixed task wave through old/new artifacts: novelty's
    // comparison sample and every other outbound field must remain identical.
    let mut novelty_program = fixture["program"].clone();
    for task in novelty_program["tasks"].as_array_mut().unwrap() {
        task["function"] = json!("evaluate_novelty");
    }
    let fields = json!({"snapshot_json":fixture["snapshot"].to_string(),"program_json":novelty_program.to_string(),"trace_json":fixture["trace"].to_string(),"started_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string()});
    let host = Arc::new(Capture(std::sync::Mutex::new(vec![])));
    let _ = run(&engine, "semantic_call", fields, host.clone()).await;
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("TASK_CONTEXT_ORIGINAL_NOVELTY_BATCH").unwrap())
            .unwrap(),
    )
    .unwrap();
    let requests = host.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0], expected,
        "relative novelty keeps its full identical comparison sample"
    );
}
