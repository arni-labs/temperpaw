//! Actual guest proof of inherited hypothetical state and temporal forecast gating.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
};
use temper_wasm::{
    StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
#[derive(Default)]
struct Provider(Mutex<Vec<Value>>);
#[async_trait::async_trait]
impl WasmHost for Provider {
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
        url: &str,
        _: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        assert_eq!(url, "https://api.typesafe.ai/v1/systemone");
        let r: Value = serde_json::from_str(body).unwrap();
        self.0.lock().unwrap().push(r.clone());
        let mut answers = json!({});
        for (id, q) in r["questions"].as_object().unwrap() {
            answers[id] = match q["type"].as_str().unwrap() {
                "noul" => json!({"type":"noul","noul":0.3}),
                "score" => {
                    let ps: serde_json::Map<String, Value> =
                        (0..q["criteria"].as_array().unwrap().len())
                            .map(|i| (i.to_string(), json!(if i == 0 { 1.0 } else { 0.0 })))
                            .collect();
                    json!({"type":"score","score":0.0,"probabilities":ps})
                }
                _ => {
                    let keys = q["criteria"].as_object().unwrap();
                    let selected = if keys.contains_key("already_observed") {
                        if r["state"]["cases"][id]["node"]["Id"] == "observed" {
                            "already_observed"
                        } else {
                            "future_change"
                        }
                    } else if keys.contains_key("evidence") {
                        "evidence"
                    } else {
                        keys.keys().next().unwrap()
                    };
                    let ps: serde_json::Map<String, Value> = keys
                        .keys()
                        .map(|k| (k.clone(), json!(if k == selected { 1.0 } else { 0.0 })))
                        .collect();
                    json!({"type":"choice","choice":selected,"probabilities":ps})
                }
            };
        }
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
}
async fn invoke(snapshot: Value, program: Value) -> (Value, Vec<Value>) {
    let path=std::env::var("FORESIGHT_CALL_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/semantic_call/target/wasm32-unknown-unknown/release/semantic_call.wasm",env!("CARGO_MANIFEST_DIR")));
    let engine = WasmEngine::new().unwrap();
    let hash = engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap();
    let host = Arc::new(Provider::default());
    let mut ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "branch-test".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some("semantic_call".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":{"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]"}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("typesafe_api_key".into(), "fixture-only".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let out = loop {
        let out = engine
            .invoke(
                &hash,
                &ctx,
                host.clone(),
                &WasmResourceLimits {
                    max_memory: 256 * 1024 * 1024,
                    max_fuel: 10_000_000_000,
                    ..Default::default()
                },
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap();
        assert_eq!(out.callback_action, "Recorded", "{}", out.callback_params);
        let p: Value =
            serde_json::from_str(out.callback_params["program_json"].as_str().unwrap()).unwrap();
        assert_ne!(p["stop_reason"], "provider_error");
        if p["cursor"].as_u64().unwrap() as usize >= p["tasks"].as_array().unwrap().len() {
            break out;
        }
        ctx.entity_state["fields"]["program_json"] = out.callback_params["program_json"].clone();
        ctx.entity_state["fields"]["trace_json"] = out.callback_params["trace_json"].clone();
    };
    let requests = host.0.lock().unwrap().clone();
    (out.callback_params, requests)
}
#[tokio::test]
async fn actual_conditional_inputs_inherit_ancestors_but_never_target_or_failed_parent() {
    let world = json!({"Id":"w","kind":"world","component_ids":["a","b","c","d"],"assumptions":[],"chain":[{"id":"ab","from_ids":["a"],"to_id":"b","by":"2027-01-01","mechanism":"A enables B"},{"id":"bc","from_ids":["b"],"to_id":"c","by":"2027-02-01","mechanism":"B enables C"},{"id":"cd","from_ids":["c"],"to_id":"d","by":"2027-03-01","mechanism":"C enables D"}]});
    let snapshot = json!({"world":{"last_ingest_date":"2026-09-29"},"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"},{"Id":"d","kind":"scenario"},world]});
    let tasks: Vec<_> = ["conditional_on", "conditional_off"]
        .iter()
        .map(|f| json!({"nodeId":"w/link/bc","world_id":"w","link_id":"bc","function":f,"depth":0}))
        .collect();
    let (out,requests)=invoke(snapshot,json!({"tasks":tasks,"cursor":0,"results":{},"evaluations":{},"baseline":{"as_of":"2026-09-29"}})).await;
    assert_eq!(requests.len(), 1);
    let cases = &requests[0]["state"]["cases"];
    let on = &cases["q0"]["branch_state"];
    let off = &cases["q1"]["branch_state"];
    assert_eq!(on["assignments"].as_array().unwrap().len(), 2);
    assert_eq!(off["assignments"].as_array().unwrap().len(), 1);
    assert_eq!(off["assignments"][0]["node_id"], "a");
    assert_eq!(off["unassigned_parent_ids"], json!(["b"]));
    assert_eq!(off["history"], json!([]));
    assert_eq!(off["parent_state_ids"], json!([]));
    assert_eq!(off["condition"]["kind"], "not_all_occurring");
    assert_eq!(on["as_of"], "2026-09-29");
    for branch in [on, off] {
        assert!(
            branch["assignments"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["node_id"] != "c" && a["node_id"] != "d")
        );
    }
    let p: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["cursor"], 2);
    let trace: Value = serde_json::from_str(out["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(trace[0]["request"]["state_ref"]["branch_state"], *on);
    assert_eq!(trace[1]["request"]["state_ref"]["branch_state"], *off);
}
#[tokio::test]
async fn temporal_classification_precedes_forecasting_and_skips_observed_stale_odds() {
    let nodes: Vec<_> = ["observed", "future"]
        .iter()
        .map(|id| json!({"Id":id,"kind":"scenario","statement":format!("Claim {id}"),"edges":"[]"}))
        .collect();
    let mut tasks = vec![];
    for id in ["observed", "future"] {
        for f in [
            "classify_temporal",
            "classify_gap",
            "estimate_likelihood",
            "evaluate_novelty",
            "decision_value",
        ] {
            tasks.push(json!({"nodeId":id,"function":f,"depth":0}));
        }
    }
    let (out,requests)=invoke(json!({"world":{"last_ingest_date":"2026-09-29"},"nodes":nodes}),json!({"baseline_status":"established","tasks":tasks,"cursor":0,"results":{"observed":{"estimate_likelihood":"0.99"}},"evaluations":{"observed":{"estimate_likelihood":{"probability":0.99}}},"baseline":{"as_of":"2026-09-29"}})).await;
    assert_eq!(requests.len(), 7);
    assert!(
        requests[0]["questions"]["q0"]["criteria"]
            .get("already_observed")
            .is_some()
    );
    assert!(
        requests
            .iter()
            .filter(|r| r["state"]["cases"]["q0"]["node"]["Id"] == "observed")
            .all(|r| r["questions"]["q0"]["type"] == "choice")
    );
    let p: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["cursor"], 10);
    assert_eq!(
        p["results"]["observed"]["classify_temporal"],
        "already_observed"
    );
    assert!(
        p["results"]["observed"]
            .get("estimate_likelihood")
            .is_none()
    );
    assert!(
        p["evaluations"]["observed"]
            .get("estimate_likelihood")
            .is_none()
    );
    assert_eq!(p["results"]["future"]["estimate_likelihood"], "0.3");
    let trace: Value = serde_json::from_str(out["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(trace.as_array().unwrap().len(), 7);
}
