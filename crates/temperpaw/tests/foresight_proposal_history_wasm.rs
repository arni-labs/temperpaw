//! Actual producer replay using a program reconstructed from an authorized prompt.
//! It compares old/new producer inputs exactly after expanding local references.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};
fn expand(v: &Value, definitions: &Value) -> Value {
    if let Some(obj) = v.as_object() {
        if obj.len() == 1
            && let Some(id) = obj.get("writer_value_ref").and_then(Value::as_str)
        {
            return expand(
                definitions.get(id).expect("missing exact definition"),
                definitions,
            );
        }
        return Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), expand(v, definitions)))
                .collect(),
        );
    }
    if let Some(items) = v.as_array() {
        return json!(
            items
                .iter()
                .map(|v| expand(v, definitions))
                .collect::<Vec<_>>()
        );
    }
    v.clone()
}
async fn invoke(bytes: &[u8], fields: &Value) -> Value {
    let engine = WasmEngine::new().unwrap();
    let hash = engine.compile_and_cache(bytes).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "fixture".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "history-test".into(),
        trigger_action: "Reason".into(),
        wasm_module: Some("semantic_reasoning".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::new(),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let r = engine
        .invoke(
            &hash,
            &ctx,
            Arc::new(SimWasmHost::new().with_default_response(500, "No network expected")),
            &WasmResourceLimits {
                max_memory: 256 * 1024 * 1024,
                max_fuel: 10_000_000_000,
                ..Default::default()
            },
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    assert_eq!(r.callback_action, "LaunchReasoning", "{r:?}");
    r.callback_params
}
#[tokio::test]
#[ignore = "requires authorized captured input and pre-change reasoning WASM"]
async fn captured_proposal_history_is_losslessly_shared_by_native_producer() {
    let capture: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("FORESIGHT_HISTORY_INPUT").unwrap()).unwrap(),
    )
    .unwrap();
    let raw: Value =
        serde_json::from_str(capture["fields"]["user_message"].as_str().unwrap()).unwrap();
    // Captured prompt uses projected refs. Assign synthetic native IDs consistently
    // for replay only; compare both producers against this same reconstruction.
    fn native_ids(v: &Value) -> Value {
        match v {
            Value::String(s) if s.starts_with("ref_") => json!(format!("fixture_node_{}", &s[4..])),
            Value::Array(a) => json!(a.iter().map(native_ids).collect::<Vec<_>>()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| {
                        (
                            if let Some(id) = k.strip_prefix("ref_") {
                                format!("fixture_node_{id}")
                            } else {
                                k.clone()
                            },
                            native_ids(v),
                        )
                    })
                    .collect(),
            ),
            _ => v.clone(),
        }
    }
    let input = native_ids(&raw);
    let mut program = input.clone();
    program["endpoint_proposal_contract"] = json!(2);
    program["endpoint_proposal_history"] = input["proposal_quality_history"].clone();
    program["results"] = input["assessments"].clone();
    program["tasks"] = json!([]);
    let mut nodes = input["source_evidence"].as_array().unwrap().clone();
    nodes.extend(input["catalog"].as_array().unwrap().clone());
    let snapshot = json!({"world":input["world"],"nodes":nodes,"branches":input["branches"]});
    let fields = json!({"phase":"explore","snapshot_json":snapshot.to_string(),"program_json":program.to_string()});
    let old = invoke(
        &std::fs::read(std::env::var("FORESIGHT_HISTORY_OLD_WASM").unwrap()).unwrap(),
        &fields,
    )
    .await;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let new=invoke(&std::fs::read(std::env::var("FORESIGHT_HISTORY_NEW_WASM").unwrap_or_else(|_|root.join("os-apps/paw-foresight/wasm/semantic_reasoning/target/wasm32-unknown-unknown/release/semantic_reasoning.wasm").to_string_lossy().into_owned())).unwrap(),&fields).await;
    let before: Value = serde_json::from_str(old["user_message"].as_str().unwrap()).unwrap();
    let encoded: Value = serde_json::from_str(new["user_message"].as_str().unwrap()).unwrap();
    let mut restored = expand(&encoded, &encoded["writer_shared_values"]);
    restored
        .as_object_mut()
        .unwrap()
        .remove("writer_shared_values");
    restored
        .as_object_mut()
        .unwrap()
        .remove("writer_reference_encoding");
    assert_eq!(
        restored, before,
        "All judgments, originals, evidence and correction values must round-trip exactly"
    );
    let prior = old["user_message"].as_str().unwrap().len();
    let after = new["user_message"].as_str().unwrap().len();
    assert!(
        after < prior / 2,
        "demonstrated repeated context remains: {prior}->{after}"
    );
    eprintln!(
        "Reconstructed captured producer input bytes {prior}->{after}; exact expansion equality passed"
    );
    if let Ok(path) = std::env::var("FORESIGHT_HISTORY_REDUCED_OUTPUT") {
        std::fs::write(format!("{path}.previous.json"),json!({"fields":{"system_prompt":old["system_prompt"],"user_message":old["user_message"]}}).to_string()).unwrap();
        std::fs::write(path,json!({"fields":{"system_prompt":new["system_prompt"],"user_message":new["user_message"]}}).to_string()).unwrap();
    }
}

#[tokio::test]
async fn structural_correction_is_tool_free_and_unknowns_reach_backward_and_writer() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../os-apps/paw-foresight/wasm/semantic_music_frontier_fixture.json"
    ))
    .unwrap();
    let a = &fixture["attempt"];
    let snapshot = json!({"world":a["world"],"nodes":a["source_evidence"],"branches":[]});
    let mut snapshot = snapshot;
    snapshot["world"]["hindcast_mode"] = json!("false");
    let mut program = json!({"endpoint_proposal_contract":2,"proposal_pool":{"stage":"contrast","candidates":a["endpoints"]},"endpoint_proposal_history":[],"baseline":a["baseline"],"world_search_contract":1,"endpoint_search":{"endpoints":a["endpoints"],"routes":[],"amendments":[]},"tasks":[],"results":{}});
    let draft = json!({"proposal_contrasts":a["endpoints"].as_array().unwrap().iter().map(|e|json!({"endpoint_id":e["id"],"contrast":e["contrast"]})).collect::<Vec<_>>()});
    program["response_correction"] = json!({"validation_error":"Keep each frontier comparison receipt within 2400 bytes; share research and use concise comparisons","rejected_draft":draft.to_string()});
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = std::env::var("FORESIGHT_HISTORY_NEW_WASM").unwrap_or_else(|_|root.join("os-apps/paw-foresight/wasm/semantic_reasoning/target/wasm32-unknown-unknown/release/semantic_reasoning.wasm").to_string_lossy().into_owned());
    let bytes = std::fs::read(path).unwrap();
    let fields = |phase: &str, p: &Value| json!({"phase":phase,"snapshot_json":snapshot.to_string(),"program_json":p.to_string()});
    let repaired = invoke(&bytes, &fields("explore", &program)).await;
    assert_eq!(repaired["tools_enabled"], "");
    assert_eq!(repaired["max_turns"], "1");
    assert!(
        repaired["system_prompt"]
            .as_str()
            .unwrap()
            .contains("explicit text-size limit")
    );
    for error in [
        "Unknown source reference",
        "Missing strongest-present comparison; report unknown rather than certify novelty",
    ] {
        program["response_correction"]["validation_error"] = json!(error);
        let substantive = invoke(&bytes, &fields("explore", &program)).await;
        assert_eq!(substantive["max_turns"], "12");
        assert!(
            substantive["tools_enabled"]
                .as_str()
                .unwrap()
                .contains("temper_web_search")
        );
    }
    program["response_correction"] = json!({"validation_error":"baseline.observed.claim: Outlook text must contain 1–400 characters","rejected_draft":json!({"baseline":{"observed":[{"claim":"Long claim ".repeat(50),"evidence_ids":["source"]}]}}).to_string()});
    assert_eq!(
        invoke(&bytes, &fields("explore", &program)).await["max_turns"],
        "1"
    );
    // Empty claims need substantive repair; this error string also covers them.
    program["response_correction"]["rejected_draft"] = json!(
        json!({"baseline":{"observed":[{"claim":"","evidence_ids":["source"]}]}}).to_string()
    );
    assert_eq!(
        invoke(&bytes, &fields("explore", &program)).await["max_turns"],
        "12"
    );
    program["response_correction"] = Value::Null;
    program["endpoint_novelty"] = json!({a["endpoints"][0]["id"].as_str().unwrap():{"status":"provisional","initial_check":a["checks"][0]}});
    // Minimal composed world placeholders exercise the writer's actual input
    // projection; this does not assert these are evaluated live worlds.
    for id in ["writer-world-a", "writer-world-b"] {
        snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":id,"kind":"world","statement":"Writer input fixture","component_ids":[],"edges":"[]"}));
    }
    let fields = |phase: &str, p: &Value| json!({"phase":phase,"snapshot_json":snapshot.to_string(),"program_json":p.to_string()});
    for phase in ["backward", "synthesize"] {
        let actual = invoke(&bytes, &fields(phase, &program)).await;
        let encoded: Value =
            serde_json::from_str(actual["user_message"].as_str().unwrap()).unwrap();
        let restored = expand(&encoded, &encoded["writer_shared_values"]);
        let endpoints = if phase == "backward" {
            &restored["endpoint_search"]["endpoints"]
        } else {
            &restored["endpoint_lineage"]["endpoints"]
        };
        assert_eq!(endpoints.as_array().unwrap().len(), 3);
        for (sent, original) in endpoints
            .as_array()
            .unwrap()
            .iter()
            .zip(a["endpoints"].as_array().unwrap())
        {
            let unknowns = |e: &Value| {
                e["contrast"]["frontier_challenge"]["comparisons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|r| r["result"] == "unknown")
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(unknowns(sent), unknowns(original));
        }
        assert!(
            actual["system_prompt"]
                .as_str()
                .unwrap()
                .contains("unknown")
        );
    }
}
