//! Branch generation crosses actual reasoning, expansion and Jev guest boundaries.
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

struct Capture(f64);
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
        let answers: serde_json::Map<String, Value> = request["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|key| (key.clone(), json!({"type":"noul","noul":self.0})))
            .collect();
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
}
#[tokio::test]
async fn final_answer_preserves_inconsistent_raw_odds_and_reports_them() {
    let engine = WasmEngine::new().unwrap();
    let h = json!({"Id":"h","kind":"scenario","statement":"The required change occurs by 2036","resolve_by":"2036-10-01","edges":"[]"});
    let mut worlds = vec![];
    for id in ["w1", "w2"] {
        worlds.push(json!({"Id":id,"kind":"world","statement":"The changes occur together by 2036","title":"A whole future","component_ids":["h","h2","h3"],"counter_ids":[],"branch_conditions":[],"edges":"[]","scene":"A future day","narrative":"A whole future with uncertain consequences","signals":["A signal"],"falsifiers":["A contrary event"],"what_you_can_do":[],"chain":[],"facets":[{"id":"f","title":"A facet","description":"A consequence","component_ids":["h","h2","h3"]}],"assumptions":[]}));
    }
    for world in &mut worlds {
        world["facets"]=json!(["h","h2","h3"].iter().enumerate().map(|(i,id)|json!({"id":format!("f{i}"),"title":format!("Consequence {}",i+1),"description":"A defining consequence in this synthetic world","component_ids":[id]})).collect::<Vec<_>>());
        world["edges"]=json!(json!([{"kind":"requires","to_id":"h"},{"kind":"requires","to_id":"h2"},{"kind":"requires","to_id":"h3"}]).to_string());
    }
    let mut snapshot = json!({"world":{"last_ingest_date":"2026-10-01","target_date":"2036-10-01"},"nodes":[h.clone(),worlds[0].clone(),worlds[1].clone()]});
    for id in ["h2", "h3"] {
        let mut node = h.clone();
        node["Id"] = json!(id);
        snapshot["nodes"].as_array_mut().unwrap().push(node);
    }
    let baseline = json!({"as_of":"2026-10-01","observed":[],"assumptions":[],"unknowns":["No source verification in this synthetic test"]});
    let mut program = json!({"stage":"worlds","world_revision":1,"baseline":baseline,"active_world_ids":["w1","w2"],"results":{},"evaluations":{},"http_calls":0});
    let mut trace = "[]".to_owned();
    for (id, p) in [("h", 0.42), ("w1", 0.49), ("w2", 0.4)] {
        program["tasks"] = json!([{"nodeId":id,"function":"estimate_likelihood","depth":0}]);
        program["cursor"] = json!(0);
        let called=run(&engine,"semantic_call",json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":trace}),Arc::new(Capture(p))).await;
        assert_eq!(called["callback_action"], "Recorded", "{called}");
        program = serde_json::from_str(called["callback_params"]["program_json"].as_str().unwrap())
            .unwrap();
        trace = called["callback_params"]["trace_json"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            program["evaluations"][id]["estimate_likelihood"]["probability"],
            p
        );
    }
    let answer = json!({"schema":"foresight-worlds-v3","headline":"Contrasting futures","summary":"Independent estimates need consistency checks","horizon":"2036-10-01","probability_basis":"model_implied_world_estimate","probability_model":"overlapping_worlds","calibrated":false,"evidence_limits":["Synthetic fixture"],"research_questions":[],"outcomes":worlds.iter().map(|w|json!({"id":w["Id"],"world_id":w["Id"],"title":w["title"],"definition":w["statement"],"component_ids":w["component_ids"],"counter_ids":[],"scene":w["scene"],"narrative":w["narrative"],"what_you_can_do":[],"signals":w["signals"],"falsifiers":w["falsifiers"]})).collect::<Vec<_>>()});
    let complete=run(&engine,"semantic_expand",json!({"phase":"synthesize","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","reasoning_result":answer.to_string()}),Arc::new(SimWasmHost::new())).await;
    assert_eq!(complete["callback_action"], "Complete", "{complete}");
    let result: Value =
        serde_json::from_str(complete["callback_params"]["answer"].as_str().unwrap()).unwrap();
    if let Ok(path) = std::env::var("ARN518_COHERENCE_FIXTURE") {
        std::fs::write(
            path,
            json!({"synthetic":true,"snapshot":snapshot,"program":program,"answer":result})
                .to_string(),
        )
        .unwrap();
    }
    assert_eq!(result["outcomes"][0]["probability"], 0.49);
    assert_eq!(result["outcomes"][1]["probability"], 0.4);
    assert_eq!(
        result["outcomes"][0]["audit"]["probability_coherence"]["findings"][0]["component_probability"],
        0.42
    );
    assert!(
        result["evaluation_note"]
            .as_str()
            .unwrap()
            .contains("49.0% exceeds a required event at 42.0%")
    );
    assert!(
        result["evidence_limits"]
            .to_string()
            .contains("inconsistent")
    );
}
