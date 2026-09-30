//! Actual guest proof: malformed typed replies never become evaluations.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
async fn invoke(
    engine: &WasmEngine,
    hash: &str,
    fields: Value,
    status: u16,
    response: &Value,
) -> Value {
    let host = SimWasmHost::new().with_default_response(status, &response.to_string());
    invoke_host(engine, hash, fields, Arc::new(host), "Recorded").await
}
async fn invoke_host(
    engine: &WasmEngine,
    hash: &str,
    fields: Value,
    host: Arc<dyn WasmHost>,
    expected_action: &str,
) -> Value {
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "retry-fixture".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some("semantic_call".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("typesafe_api_key".into(), "fixture-key".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let r = engine
        .invoke(
            hash,
            &ctx,
            host,
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
        r.callback_action, expected_action,
        "{:?}",
        r.callback_params
    );
    r.callback_params
}
fn fixture() -> Value {
    json!({"snapshot_json":json!({"world":{"Id":"w"},"nodes":[{"Id":"h","kind":"scenario","statement":"A dated event happens in 2027","edges":"[]"}]}).to_string(),"program_json":json!({"cursor":0,"tasks":[{"nodeId":"h","function":"classify_gap"}],"results":{},"evaluations":{}}).to_string(),"trace_json":"[]"})
}
fn response(valid: bool) -> Value {
    json!({"model":"jev-1.13.0","answers":{"q0":{"type":"choice","choice":if valid {"evidence"} else {"timing"},"probabilities":{"evidence":0.8,"timing":0.1,"none":0.1,"prerequisite":0.0,"uncertain":0.0}}}})
}
#[tokio::test]
async fn inconsistent_choice_retries_then_only_valid_answer_advances() {
    let engine = WasmEngine::new().unwrap();
    let path=std::env::var("ARN518_VALIDATION_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/semantic_call/target/wasm32-unknown-unknown/release/semantic_call.wasm",env!("CARGO_MANIFEST_DIR")));
    let hash = engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap();
    let initial = fixture();
    let first = invoke(&engine, &hash, initial.clone(), 200, &response(false)).await;
    let p: Value = serde_json::from_str(first["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["stop_reason"], "validation_retry");
    assert_eq!(p["cursor"], 0);
    assert_eq!(p["evaluations"], json!({}));
    let t: Value = serde_json::from_str(first["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(t[0]["rejectedResponse"], response(false));
    assert!(
        t[0]["error"]
            .as_str()
            .unwrap()
            .contains("selected_probability=0.1")
    );
    let mut fields = initial.clone();
    fields["program_json"] = first["program_json"].clone();
    fields["trace_json"] = first["trace_json"].clone();
    let success = invoke(&engine, &hash, fields.clone(), 200, &response(true)).await;
    let p: Value = serde_json::from_str(success["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["cursor"], 1);
    assert_eq!(p["validation_failures"], 0);
    assert_eq!(p["results"]["h"]["classify_gap"], "evidence");
    let t: Value = serde_json::from_str(success["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(t.as_array().unwrap().len(), 2);
    assert_eq!(t[0]["requestHash"], t[1]["requestHash"]);
    for attempt in 2..=3 {
        let next = invoke(&engine, &hash, fields.clone(), 200, &response(false)).await;
        let p: Value = serde_json::from_str(next["program_json"].as_str().unwrap()).unwrap();
        assert_eq!(p["cursor"], 0);
        assert_eq!(p["evaluations"], json!({}));
        assert_eq!(
            p["stop_reason"],
            if attempt == 2 {
                "validation_retry"
            } else {
                "provider_error"
            }
        );
        fields["program_json"] = next["program_json"].clone();
        fields["trace_json"] = next["trace_json"].clone();
    }
    let denied = invoke(&engine, &hash, initial, 403, &json!({"message":"denied"})).await;
    let p: Value = serde_json::from_str(denied["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["stop_reason"], "provider_error");
    assert!(p["validation_failures"].is_null());
}

#[derive(Default)]
struct Capture(std::sync::Mutex<Vec<Value>>);
#[async_trait::async_trait]
impl WasmHost for Capture {
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Err("not used".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("not used".into())
    }
    async fn http_call(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(body).unwrap());
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":{"q0":{"type":"noul","noul":0.23}}}).to_string(),
        ))
    }
}
#[tokio::test]
async fn world_likelihood_interns_provenance_losslessly_in_actual_provider_input() {
    let ids: Vec<_> = (0..35)
        .map(|i| format!("evidence-with-uuid-length-00000000-{i}"))
        .collect();
    let mut nodes:Vec<Value>=(0..6).map(|i|json!({"Id":format!("h{i}"),"kind":"scenario","statement":format!("Component {i} happens by 2027"),"edges":"[]"})).collect();
    let mut evaluations = json!({});
    for i in 0..6 {
        for f in [
            "classify_gap",
            "estimate_likelihood",
            "evaluate_novelty",
            "decision_value",
        ] {
            evaluations[format!("h{i}")][f] = json!({"type":"noul","probability":0.23,"context":{"round":2,"task":{"nodeId":format!("h{i}"),"function":f},"evidence_ids":ids}});
        }
    }
    nodes.push(json!({"Id":"world","kind":"world","statement":"All six components happen jointly by 2027","component_ids":(0..6).map(|i|format!("h{i}")).collect::<Vec<_>>(),"counter_ids":[],"edges":(0..6).map(|i|json!({"kind":"requires","to_id":format!("h{i}")})).collect::<Vec<_>>().pipe_json()}));
    nodes.push(json!({"Id":ids[0],"kind":"evidence","statement":"Observed baseline","source_quote":"Exact quoted evidence remains here.","edges":"[]"}));
    let snapshot = json!({"world":{"Id":"question"},"nodes":nodes});
    let program = json!({"stage":"worlds","cursor":0,"tasks":[{"nodeId":"world","function":"estimate_likelihood"}],"results":{},"evaluations":evaluations});
    let fields = json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]"});
    let engine = WasmEngine::new().unwrap();
    let path=std::env::var("ARN518_VALIDATION_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/semantic_call/target/wasm32-unknown-unknown/release/semantic_call.wasm",env!("CARGO_MANIFEST_DIR")));
    let hash = engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap();
    let host = Arc::new(Capture::default());
    let out = invoke_host(&engine, &hash, fields, host.clone(), "Recorded").await;
    let request = host.0.lock().unwrap()[0].clone();
    let mut state = request["state"]["cases"]["q0"].clone();
    for (key, value) in request["state"]["common"].as_object().unwrap() {
        state[key] = value.clone();
    }
    let sets = state["evidence_sets"].clone();
    assert_eq!(sets.as_array().unwrap().len(), 1);
    let packed = state.to_string().len();
    for node in state["prerequisites"].as_array_mut().unwrap() {
        for value in node["evaluations"].as_object_mut().unwrap().values_mut() {
            let index = value["context"]["evidence_ids"]["$evidence_set_ref"]
                .as_u64()
                .unwrap() as usize;
            value["context"]["evidence_ids"] = sets[index].clone();
        }
        let mut expected = evaluations[node["id"].as_str().unwrap()].clone();
        expected
            .as_object_mut()
            .unwrap()
            .retain(|k, _| matches!(k.as_str(), "classify_gap" | "estimate_likelihood"));
        assert_eq!(node["evaluations"], expected);
    }
    state.as_object_mut().unwrap().remove("evidence_sets");
    state.as_object_mut().unwrap().remove("context_encoding");
    assert!(packed * 2 < state.to_string().len());
    assert_eq!(
        state["source_evidence"][0]["source_quote"],
        "Exact quoted evidence remains here."
    );
    let trace: Value = serde_json::from_str(out["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(trace[0]["request"]["state_ref"]["evidence_sets"], sets);
    let after: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    for i in 0..6 {
        assert_eq!(
            after["evaluations"][format!("h{i}")],
            evaluations[format!("h{i}")]
        );
    }
}

trait JsonText {
    fn pipe_json(self) -> String;
}
impl JsonText for Vec<Value> {
    fn pipe_json(self) -> String {
        json!(self).to_string()
    }
}

#[derive(Default)]
struct PairCapture(std::sync::Mutex<Vec<Value>>);
#[async_trait::async_trait]
impl WasmHost for PairCapture {
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("unused".into())
    }
    async fn http_call(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        let request: Value = serde_json::from_str(body).unwrap();
        self.0.lock().unwrap().push(request.clone());
        let mut answers = json!({});
        for (key, q) in request["questions"].as_object().unwrap() {
            let mut probabilities = json!({});
            for choice in q["criteria"].as_object().unwrap().keys() {
                probabilities[choice] = json!(if choice == "compatible" { 1.0 } else { 0.0 });
            }
            answers[key] =
                json!({"type":"choice","choice":"compatible","probabilities":probabilities});
        }
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
}
#[tokio::test]
#[ignore = "Optional local captured checkpoint; no private data committed"]
async fn captured_large_pair_program_fits_existing_fuel_budget() {
    let path = std::env::var("ARN518_PAIR_RECORD").unwrap();
    let record: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let fields = record.get("fields").unwrap_or(&record).clone();
    let before: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    let trace: Value = serde_json::from_str(fields["trace_json"].as_str().unwrap()).unwrap();
    let engine = WasmEngine::new().unwrap();
    let path = std::env::var("ARN518_VALIDATION_WASM").unwrap();
    let hash = engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap();
    let host = Arc::new(PairCapture::default());
    let out = invoke_host(&engine, &hash, fields, host.clone(), "Recorded").await;
    let after: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    let nexttrace: Value = serde_json::from_str(out["trace_json"].as_str().unwrap()).unwrap();
    assert!(after["cursor"].as_u64().unwrap() > before["cursor"].as_u64().unwrap());
    assert_eq!(
        &nexttrace.as_array().unwrap()[..trace.as_array().unwrap().len()],
        trace.as_array().unwrap()
    );
    assert_eq!(after["tasks"], before["tasks"]);
    println!(
        "preserved {} checks, advanced cursor {} -> {}, HTTP calls {}",
        trace.as_array().unwrap().len(),
        before["cursor"],
        after["cursor"],
        host.0.lock().unwrap().len()
    );
}

#[tokio::test]
async fn transient_520_retries_same_question_without_losing_receipts_or_clock() {
    let engine = WasmEngine::new().unwrap();
    let path=std::env::var("ARN518_VALIDATION_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/semantic_call/target/wasm32-unknown-unknown/release/semantic_call.wasm",env!("CARGO_MANIFEST_DIR")));
    let hash = engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap();
    let mut fields = fixture();
    fields["started_at_ms"] = json!("9999999999999");
    let failure = invoke(
        &engine,
        &hash,
        fields.clone(),
        520,
        &json!({"error":"temporary upstream failure"}),
    )
    .await;
    let failed: Value = serde_json::from_str(failure["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(failed["stop_reason"], "provider_retry");
    assert_eq!(failed["cursor"], 0);
    assert_eq!(failed["http_calls"], 1);
    assert_eq!(failed["evaluations"], json!({}));
    assert!(failure.get("started_at_ms").is_none());
    fields["program_json"] = failure["program_json"].clone();
    fields["trace_json"] = failure["trace_json"].clone();
    let step_path = format!(
        "{}/../../os-apps/paw-foresight/wasm/semantic_step/target/wasm32-unknown-unknown/release/semantic_step.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    let step_hash = engine
        .compile_and_cache(&std::fs::read(step_path).unwrap())
        .unwrap();
    invoke_host(
        &engine,
        &step_hash,
        fields.clone(),
        Arc::new(SimWasmHost::new()),
        "Evaluate",
    )
    .await;
    let success = invoke(&engine, &hash, fields.clone(), 200, &response(true)).await;
    let p: Value = serde_json::from_str(success["program_json"].as_str().unwrap()).unwrap();
    let before: Value = serde_json::from_str(failure["trace_json"].as_str().unwrap()).unwrap();
    let trace: Value = serde_json::from_str(success["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["cursor"], 1);
    assert_eq!(p["http_calls"], 2);
    assert_eq!(p["transient_provider_failures"], 0);
    assert_eq!(p["stop_reason"], "");
    assert_eq!(trace[0], before[0]);
    assert_eq!(trace[0]["requestHash"], trace[1]["requestHash"]);
    assert!(success.get("started_at_ms").is_none());
    for attempt in 2..=3 {
        let next = invoke(&engine, &hash, fields.clone(), 520, &json!({})).await;
        let p: Value = serde_json::from_str(next["program_json"].as_str().unwrap()).unwrap();
        assert_eq!(p["cursor"], 0);
        assert_eq!(p["http_calls"], attempt);
        assert_eq!(
            p["stop_reason"],
            if attempt == 2 {
                "provider_retry"
            } else {
                "provider_error"
            }
        );
        fields["program_json"] = next["program_json"].clone();
        fields["trace_json"] = next["trace_json"].clone();
    }
    for status in [400, 401, 402, 403] {
        let permanent = invoke(&engine, &hash, fixture(), status, &json!({})).await;
        let p: Value = serde_json::from_str(permanent["program_json"].as_str().unwrap()).unwrap();
        assert_eq!(p["stop_reason"], "provider_error");
        assert_eq!(p["http_calls"], 1);
    }
}
