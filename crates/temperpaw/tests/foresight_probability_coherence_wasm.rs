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

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let sorted: std::collections::BTreeMap<_, _> =
                m.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        _ => v.clone(),
    }
}
fn fingerprint(v: &Value) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(canonical(v).to_string().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn proposition(n: &Value) -> Value {
    json!({"id":n["Id"],"statement":n["statement"],"resolve_by":n["resolve_by"],"component_ids":n["component_ids"],"branch_conditions":n["branch_conditions"]})
}
#[tokio::test]
async fn final_answer_preserves_inconsistent_raw_odds_and_reports_them() {
    let engine = WasmEngine::new().unwrap();
    let h = json!({"Id":"h","kind":"scenario","statement":"The required change occurs by 2036","resolve_by":"2036-10-01","edges":"[]"});
    let mut worlds = vec![];
    for id in ["w1", "w2"] {
        worlds.push(json!({"Id":id,"kind":"world","statement":"The changes occur together by 2036","title":"A whole future","component_ids":["h","h2","h3"],"counter_ids":[],"branch_conditions":[],"edges":"[]","scene":"A future day","narrative":"A whole future with uncertain consequences","signals":["A signal"],"falsifiers":["A contrary event"],"what_you_can_do":[],"chain":[],"facets":[{"id":"f","title":"A facet","description":"A consequence","component_ids":["h","h2","h3"]}],"assumptions":[]}));
    }
    let mut snapshot = json!({"world":{"last_ingest_date":"2026-10-01","target_date":"2036-10-01"},"nodes":[h.clone(),worlds[0].clone(),worlds[1].clone()]});
    for id in ["h2", "h3"] {
        let mut node = h.clone();
        node["Id"] = json!(id);
        snapshot["nodes"].as_array_mut().unwrap().push(node);
    }
    let baseline = json!({"as_of":"2026-10-01","observed":[],"assumptions":[],"unknowns":["No source verification in this synthetic test"]});
    let context =
        fingerprint(&json!({"world":snapshot["world"],"baseline":baseline,"source_evidence":[]}));
    let mut program = json!({"baseline":baseline,"active_world_ids":["w1","w2"],"results":{"h":{"estimate_likelihood":"0.42"},"w1":{"estimate_likelihood":"0.49"},"w2":{"estimate_likelihood":"0.40"}},"evaluations":{}});
    for (node, p) in [(&h, 0.42), (&worlds[0], 0.49), (&worlds[1], 0.4)] {
        program["evaluations"][node["Id"].as_str().unwrap()]["estimate_likelihood"] = json!({"type":"noul","probability":p,"context":{"probability_comparison":{"version":1,"context_hash":context,"proposition_hash":fingerprint(&proposition(node)),"component_hashes":{"h":fingerprint(&proposition(&h))}}}});
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
