//! Real guest boundary tests for evidence chronology and baseline eligibility.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
fn artifact(engine: &WasmEngine, module: &str) -> String {
    let path=std::env::var(format!("ARN518_EVIDENCE_{}",module.to_uppercase())).unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm",env!("CARGO_MANIFEST_DIR")));
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
        entity_state: json!({"fields":fields}),
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
fn metadata(kind: &str) -> Value {
    json!({"kind":kind,"publication_date":null,"observation_period":{"start":null,"end":null},"retrieved_at":null})
}
#[derive(Default)]
struct Capture {
    requests: Mutex<Vec<Value>>,
}
#[async_trait::async_trait]
impl WasmHost for Capture {
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("Unexpected binary IO".into())
    }
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Ok("fixture-only".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_call(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(body).unwrap());
        Ok((503, "fixture stops after capture".into()))
    }
}
#[tokio::test]
async fn chronology_and_lead_eligibility_cross_real_guest_boundaries() {
    let engine = WasmEngine::new().unwrap();
    let historical = json!({"kind":"finding","publication_date":"2025","observation_period":{"start":"2020","end":"2021"},"retrieved_at":"2026-09-30"});
    let unknown = metadata("finding");
    let lead = metadata("lead");
    let rows = json!({"value":[
 {"Id":"historical","world_id":"w","provenance":"observed","statement":"Support-worker study found productivity gains during its study period","source_refs":"[\"https://example.org/study\"]","evidence_json":historical.to_string()},
 {"Id":"unknown","world_id":"w","provenance":"observed","statement":"Scoped measured finding with unknown dates","source_refs":"[\"https://example.org/finding\"]","evidence_json":unknown.to_string()},
 {"Id":"lead","world_id":"w","provenance":"observed","statement":"Title of an inaccessible paper","source_refs":"[\"https://example.org/paper\"]","evidence_json":lead.to_string()},
 {"Id":"legacy","world_id":"w","provenance":"observed","statement":"Old untyped statement","source_refs":"[]"}]});
    let host=SimWasmHost::new().with_default_response(500,"unexpected IO")
 .with_response("http://fixture/tdata/Worlds('w')",200,&json!({"Id":"w","Status":"Active","hindcast_mode":"false","research_session_id":"research","agent_model":"fixture","agent_provider":"fixture","last_ingest_date":"2026-09-30","target_date":"2031-09-30"}).to_string())
 .with_response("http://fixture/tdata/EventNodes?$filter=world_id%20eq%20'w'&$top=513",200,&rows.to_string())
 .with_response("http://fixture/tdata/Sessions('research')",200,"{\"agent_id\":\"researcher\"}");
    let prepared = run(
        &engine,
        "semantic_prepare",
        json!({"world_id":"w"}),
        Arc::new(host),
    )
    .await;
    assert_eq!(prepared["callback_action"], "Prepared", "{prepared}");
    let mut fields = prepared["callback_params"].clone();
    let snapshot: Value = serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    assert_eq!(snapshot["world"]["evidence_contract"], "v1");
    assert_eq!(snapshot["nodes"][0]["evidence_metadata"], historical);
    assert_eq!(snapshot["nodes"][1]["evidence_metadata"], unknown);
    assert_eq!(snapshot["nodes"][2]["evidence_metadata"], lead);
    assert_eq!(
        snapshot["nodes"][3]["evidence_metadata"]["kind"],
        "legacy_unverified"
    );
    for id in ["lead", "legacy"] {
        fields["reasoning_result"]=json!(json!({"baseline":{"as_of":"2026-09-30","observed":[{"claim":"Substantive claim","evidence_ids":[id]}],"assumptions":[],"unknowns":[]}}).to_string());
        let rejected = run(
            &engine,
            "semantic_expand",
            fields.clone(),
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert_eq!(
            rejected["callback_action"], "CompositionRejected",
            "{rejected}"
        );
        assert!(rejected["callback_params"].get("snapshot_json").is_none());
    }
    fields["reasoning_result"]=json!(json!({"baseline":{"as_of":"2026-09-30","observed":[{"claim":"Historical measured result; dates unknown for the second finding","evidence_ids":["historical","unknown"]}],"assumptions":[],"unknowns":[]}}).to_string());
    let accepted = run(
        &engine,
        "semantic_expand",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(accepted["callback_action"], "Expanded", "{accepted}");
    for date_field in ["publication", "observation"] {
        let mut future_snapshot = snapshot.clone();
        if date_field == "publication" {
            future_snapshot["nodes"][0]["evidence_metadata"]["publication_date"] = json!("2027");
        } else {
            future_snapshot["nodes"][0]["evidence_metadata"]["observation_period"] =
                json!({"start":"2027","end":"2027"});
        }
        let mut future_fields = fields.clone();
        future_fields["snapshot_json"] = json!(future_snapshot.to_string());
        let future = run(
            &engine,
            "semantic_expand",
            future_fields,
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert_eq!(
            future["callback_action"], "CompositionRejected",
            "{date_field}: {future}"
        );
    }
    fields["program_json"] = accepted["callback_params"]["program_json"].clone();
    let capture = Arc::new(Capture::default());
    let _ = run(&engine, "semantic_call", fields.clone(), capture.clone()).await;
    let requests = capture.requests.lock().unwrap().clone();
    assert!(!requests.is_empty());
    fn find_node<'a>(value: &'a Value, id: &str) -> Option<&'a Value> {
        if value["Id"] == id && value.get("evidence_metadata").is_some() {
            return Some(value);
        }
        match value {
            Value::Object(map) => map.values().find_map(|v| find_node(v, id)),
            Value::Array(items) => items.iter().find_map(|v| find_node(v, id)),
            _ => None,
        }
    }
    for (id, expected) in [
        ("historical", historical.clone()),
        ("unknown", unknown.clone()),
        ("lead", lead.clone()),
    ] {
        assert_eq!(
            find_node(&requests[0], id).unwrap()["evidence_metadata"],
            expected
        );
    }
    assert_eq!(
        find_node(&requests[0], "legacy").unwrap()["evidence_metadata"]["kind"],
        "legacy_unverified"
    );
    fields["phase"] = json!("explore");
    let report = json!({"id":"later","statement":"A later retrieved scoped finding","url":"https://example.org/later","quote":"A scoped finding","provenance":"observed","observed_at":"2026-09-30","evidence_metadata":historical});
    let mut response = json!({"hypotheses":[],"research_evidence":[report],"continue_exploring":false,"exploration_note":"A dated finding was added"});
    fields["reasoning_result"] = json!(response.to_string());
    let later = run(
        &engine,
        "semantic_expand",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(later["callback_action"], "Expanded", "{later}");
    let later_snapshot: Value =
        serde_json::from_str(later["callback_params"]["snapshot_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        later_snapshot["nodes"].as_array().unwrap().last().unwrap()["evidence_metadata"],
        historical
    );
    response["research_evidence"][0]
        .as_object_mut()
        .unwrap()
        .remove("evidence_metadata");
    fields["reasoning_result"] = json!(response.to_string());
    let missing = run(
        &engine,
        "semantic_expand",
        fields,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(
        missing["callback_action"], "CompositionRejected",
        "{missing}"
    );
    assert_eq!(
        snapshot["nodes"][1]["evidence_metadata"]["publication_date"],
        Value::Null
    );
}
