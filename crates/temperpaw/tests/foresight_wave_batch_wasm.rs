//! Actual guest proof of bounded independent batching and preserved dependencies.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
#[allow(dead_code)]
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
        entity_state: json!({"fields":fields,"counters":{"transition_count":fields["_transition_count"].as_u64().unwrap_or(0)}}),
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

#[allow(dead_code)]
mod core {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../os-apps/paw-foresight/wasm/semantic_core.rs"
    ));
}
struct WaveHost(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl WasmHost for WaveHost {
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
        let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 2 {
            return Ok((520, "temporary upstream error".into()));
        }
        let mut answers = json!({});
        for (key, q) in request["questions"].as_object().unwrap() {
            let case = &request["state"]["cases"][key];
            answers[key] = match q["type"].as_str().unwrap() {
                "choice" => {
                    let keys = q["criteria"].as_object().unwrap();
                    let choice = if keys.contains_key("future_change") {
                        "future_change"
                    } else if keys.contains_key("compatible") {
                        "compatible"
                    } else if keys.contains_key("coherent") {
                        "coherent"
                    } else if keys.contains_key("none") {
                        "none"
                    } else {
                        keys.keys().next().unwrap()
                    };
                    let mut probabilities = json!({});
                    for k in keys.keys() {
                        probabilities[k] = json!(if k == choice { 1.0 } else { 0.0 });
                    }
                    json!({"type":"choice","choice":choice,"probabilities":probabilities})
                }
                "noul" => {
                    if case["node"]["kind"] == "scenario" {
                        assert_eq!(case["assessment"]["classify_temporal"], "future_change");
                        assert!(case["assessment"]["classify_gap"].is_string());
                    } else if case["node"]["kind"] == "world" {
                        assert!(
                            case["world_audit"]["checks"]
                                .as_array()
                                .is_some_and(|checks| !checks.is_empty()),
                            "world likelihood must see completed audits: {case}"
                        );
                    }
                    json!({"type":"noul","noul":0.37})
                }
                "score" => {
                    assert_eq!(case["assessment"]["estimate_likelihood"], "0.37");
                    let mut probabilities = json!({});
                    for i in 0..q["criteria"].as_array().unwrap().len() {
                        probabilities[i.to_string()] = json!(if i == 0 { 1.0 } else { 0.0 })
                    }
                    json!({"type":"score","score":0.0,"probabilities":probabilities})
                }
                other => panic!("unexpected {other}"),
            };
        }
        if n == 5 {
            answers = json!({});
        }
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
}
#[tokio::test]
async fn thousand_judgments_keep_prior_scores_and_fit_reserved_transition_budget() {
    let engine = WasmEngine::new().unwrap();
    let path=std::env::var("ARN518_WAVE_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/semantic_call/target/wasm32-unknown-unknown/release/semantic_call.wasm",env!("CARGO_MANIFEST_DIR")));
    let hash = engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap();
    let nodes:Vec<_>=(0..220).map(|i|json!({"Id":format!("h{i}"),"kind":"scenario","statement":format!("Distinct future event {i} occurs by2030"),"edges":"[]"})).collect();
    let mut tasks = vec![];
    for f in [
        "classify_temporal",
        "classify_gap",
        "estimate_likelihood",
        "evaluate_novelty",
        "decision_value",
    ] {
        for n in &nodes {
            tasks.push(json!({"nodeId":n["Id"],"function":f,"depth":0}));
        }
    }
    let mut fields = json!({"snapshot_json":json!({"world":{"Id":"w","last_ingest_date":"2026-09-30"},"nodes":nodes}).to_string(),"program_json":json!({"cursor":0,"tasks":tasks,"stage":"exploration","baseline_status":"established","results":{},"evaluations":{}}).to_string(),"trace_json":"[]","started_at_ms":"9999999999999"});
    let host = Arc::new(WaveHost(std::sync::atomic::AtomicUsize::new(0)));
    let mut invocations = 0;
    loop {
        let before: Value = serde_json::from_str(fields["trace_json"].as_str().unwrap()).unwrap();
        let out = invoke_host(&engine, &hash, fields.clone(), host.clone(), "Recorded").await;
        invocations += 1;
        assert!(invocations < 200, "fixture must progress");
        let p: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
        assert_ne!(p["stop_reason"], "provider_error", "{}", p["last_error"]);
        let trace: Value = serde_json::from_str(out["trace_json"].as_str().unwrap()).unwrap();
        assert_eq!(
            &trace.as_array().unwrap()[..before.as_array().unwrap().len()],
            before.as_array().unwrap()
        );
        if invocations == 1 {
            assert!(
                p["cursor"].as_u64().unwrap() > 8,
                "old sequential artifact cannot batch independent wave"
            );
        }
        fields["program_json"] = out["program_json"].clone();
        fields["trace_json"] = out["trace_json"].clone();
        assert!(
            invocations * 2 < 192,
            "exploration consumed completion reserve"
        );
        if p["cursor"] == 1100 {
            assert_eq!(trace.as_array().unwrap().len(), 1102);
            assert_eq!(p["evaluations"].as_object().unwrap().len(), 220);
            assert_eq!(p["stop_reason"], "");
            println!(
                "1100valid judgments+2failed receipts, {}HTTP/invocations, {}conservative transitions",
                host.0.load(std::sync::atomic::Ordering::SeqCst),
                invocations * 2
            );
            break;
        }
    }
    let mut snapshot: Value =
        serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    let world = json!({"Id":"joint-world","kind":"world","statement":"All three distinct future events happen together by2030","component_ids":["h0","h1","h2"],"counter_ids":[],"chain":[],"resolve_by":"2030-12-31","edges":"[]"});
    snapshot["nodes"]
        .as_array_mut()
        .unwrap()
        .push(world.clone());
    let mut p: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    p["stage"] = json!("worlds");
    p["cursor"] = json!(0);
    p["tasks"] = json!(core::search::world_tasks(&world));
    let world_count = p["tasks"].as_array().unwrap().len();
    fields["snapshot_json"] = json!(snapshot.to_string());
    fields["program_json"] = json!(p.to_string());
    loop {
        let out = invoke_host(&engine, &hash, fields.clone(), host.clone(), "Recorded").await;
        invocations += 1;
        assert!(invocations < 200, "fixture must progress");
        p = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
        assert_ne!(p["stop_reason"], "provider_error", "{}", p["last_error"]);
        fields["program_json"] = out["program_json"].clone();
        fields["trace_json"] = out["trace_json"].clone();
        assert!(invocations * 2 < 416);
        if p["cursor"].as_u64().unwrap() as usize == world_count {
            break;
        }
    }
    assert_eq!(p["results"]["joint-world"]["estimate_likelihood"], "0.37");
    assert!(p["results"]["joint-world"]["check_world_consistency"].is_string());
    let trace: Value = serde_json::from_str(fields["trace_json"].as_str().unwrap()).unwrap();
    let world_checks = trace
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["task"]["world_id"] == "joint-world")
        .count();
    assert_eq!(world_checks, world_count - 1);
    println!(
        "fresh world estimate0.37 and {world_checks} actual audit receipts; total {}transitions (<512)",
        invocations * 2
    );
}

#[tokio::test]
async fn actual_step_reserves_writer_before_native_hop_exhaustion() {
    let engine = WasmEngine::new().unwrap();
    let hash=engine.compile_and_cache(&std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"),"/../../os-apps/paw-foresight/wasm/semantic_step/target/wasm32-unknown-unknown/release/semantic_step.wasm")).unwrap()).unwrap();
    let world =
        json!({"Id":"w","kind":"world","statement":"Joint future","component_ids":[],"chain":[]});
    let p = json!({"stage":"worlds","cursor":0,"tasks":[{"nodeId":"w","function":"estimate_likelihood"}],"active_world_ids":["w"],"results":{"w":{"estimate_likelihood":"0.37"}},"evaluations":{}});
    let fields = json!({"_transition_count":416,"snapshot_json":json!({"nodes":[world]}).to_string(),"program_json":p.to_string(),"trace_json":"[]","started_at_ms":"9999999999999"});
    let out = invoke_host(
        &engine,
        &hash,
        fields,
        Arc::new(SimWasmHost::new()),
        "Reason",
    )
    .await;
    assert_eq!(out["phase"], "synthesize");
    let p: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["stop_reason"], "transition_budget");
    assert_eq!(p["results"]["w"]["estimate_likelihood"], "0.37");
    assert_eq!(out["reasoning_phase_polls"], 0);
}

#[tokio::test]
async fn accepted_candidate_stamps_exact_prerequisite_context_and_invalidates_changed_parent() {
    let engine = WasmEngine::new().unwrap();
    let path=std::env::var("ARN518_WAVE_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/semantic_call/target/wasm32-unknown-unknown/release/semantic_call.wasm",env!("CARGO_MANIFEST_DIR")));
    let hash = engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap();
    let snapshot = json!({"world":{"description":"Question"},"nodes":[{"Id":"parent","kind":"scenario","statement":"Prior event","edges":"[]"},{"Id":"child","kind":"scenario","statement":"Dependent event","edges":"[{\"kind\":\"requires\",\"to_id\":\"parent\"}]"}]});
    let task = json!({"nodeId":"child","function":"estimate_likelihood","depth":1});
    let mut program = json!({"world_search_contract":1,"stage":"exploration","cursor":0,"tasks":[task],"baseline":{},"results":{},"evaluations":{}});
    let before = program.clone();
    core::endpoints::invalidate_changed_candidates(&snapshot, &mut program, &before);
    program["results"]["parent"]["estimate_likelihood"] = json!("0.2");
    program["results"]["child"]["classify_temporal"] = json!("future_change");
    program["results"]["child"]["classify_gap"] = json!("none");
    let expected =
        core::evaluation::candidate_prerequisite_fingerprint(&snapshot, &program, "child")
            .unwrap()
            .unwrap();
    let fields = json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","started_at_ms":"9999999999999"});
    let out = invoke_host(
        &engine,
        &hash,
        fields,
        Arc::new(WaveHost(std::sync::atomic::AtomicUsize::new(0))),
        "Recorded",
    )
    .await;
    let mut recorded: Value = serde_json::from_str(out["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        recorded["evaluations"]["child"]["estimate_likelihood"]["context"]["prerequisite_input_fingerprint"],
        expected
    );
    let saved = recorded.clone();
    core::endpoints::invalidate_changed_candidates(&snapshot, &mut recorded, &saved);
    assert_eq!(recorded["results"]["child"]["estimate_likelihood"], "0.37");
    recorded["results"]["parent"]["classify_temporal"] = json!("future_change");
    recorded["results"]["parent"]["classify_gap"] = json!("none");
    recorded["tasks"] = json!([{"nodeId":"parent","function":"estimate_likelihood","depth":0}]);
    recorded["cursor"] = json!(0);
    let fields = json!({"snapshot_json":snapshot.to_string(),"program_json":recorded.to_string(),"trace_json":out["trace_json"],"started_at_ms":"9999999999999"});
    let refreshed = invoke_host(
        &engine,
        &hash,
        fields,
        Arc::new(WaveHost(std::sync::atomic::AtomicUsize::new(0))),
        "Recorded",
    )
    .await;
    let refreshed: Value =
        serde_json::from_str(refreshed["program_json"].as_str().unwrap()).unwrap();
    assert!(refreshed["results"]["child"]["estimate_likelihood"].is_null());
    assert_eq!(refreshed["cursor"], 1);
    assert_eq!(refreshed["tasks"][1]["nodeId"], "child");
    assert_eq!(refreshed["tasks"][1]["function"], "estimate_likelihood");
    recorded["results"]["parent"]["estimate_likelihood"] = json!("0.8");
    core::endpoints::invalidate_changed_candidates(&snapshot, &mut recorded, &saved);
    assert!(recorded["results"]["child"]["estimate_likelihood"].is_null());
}
